//! Self-update checks against the signed GitHub Releases manifest.
//!
//! A background loop checks once the app has settled and then every six
//! hours; a "Check for Updates…" tray item runs the same check on demand. A
//! downloaded update parks its version on `UpdateStatus`, which swaps the
//! menu item for "Restart to Update — vX.Y.Z" on the next tray repaint.
//!
//! macOS downloads and installs in the background, so the restart only
//! relaunches. Windows downloads and signature-checks the installer but keeps
//! it in memory: nothing installs, and the app never exits, until the user
//! chooses "Install Update" in the tray menu.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_updater::UpdaterExt;

/// The first check waits until launch settles — startup is already busy
/// finding CLIs, reading keychains, and painting the tray.
const FIRST_CHECK_SECS: u64 = 45;
const CHECK_EVERY_SECS: u64 = 6 * 60 * 60;
/// How often the background loop compares the wall clock with the last
/// check. Tokio timers pause while the Mac sleeps, so a plain six-hour sleep
/// stretched across every nap and a laptop that sleeps overnight could go
/// days between checks.
const CLOCK_TICK_SECS: u64 = 10 * 60;

/// What a Windows update keeps in memory until the user installs it: the
/// updater's handle and the downloaded, signature-verified installer bytes.
#[cfg(windows)]
type Payload = (tauri_plugin_updater::Update, Vec<u8>);
#[cfg(not(windows))]
type Payload = ();

struct Pending<T> {
    version: String,
    payload: Option<T>,
}

/// The single staged update. The payload can be taken once, so a double click
/// cannot start two installers, and is put back if the installer returns.
struct Staged<T> {
    slot: Mutex<Option<Pending<T>>>,
}

impl<T> Default for Staged<T> {
    fn default() -> Self {
        Self {
            slot: Mutex::new(None),
        }
    }
}

impl<T> Staged<T> {
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Pending<T>>> {
        self.slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn version(&self) -> Option<String> {
        self.lock().as_ref().map(|pending| pending.version.clone())
    }

    fn set(&self, version: String, payload: Option<T>) {
        *self.lock() = Some(Pending { version, payload });
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    fn take_payload(&self) -> Option<(String, T)> {
        let mut slot = self.lock();
        let pending = slot.as_mut()?;
        let payload = pending.payload.take()?;
        Some((pending.version.clone(), payload))
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    fn restore(&self, payload: T) {
        if let Some(pending) = self.lock().as_mut() {
            pending.payload = Some(payload);
        }
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    fn clear(&self) {
        *self.lock() = None;
    }
}

/// The update that downloaded and is waiting on a relaunch (or, on Windows,
/// on the user's go-ahead). The tray reads its version to offer the action,
/// and `sync_menus` folds it into the menu signature so a landing update
/// rebuilds the menus once.
#[derive(Default)]
pub struct UpdateStatus {
    staged: Staged<Payload>,
    /// Set while a check (and its download) runs, so a manual check during
    /// the background one does not download the same build twice at once.
    checking: AtomicBool,
}

impl UpdateStatus {
    pub fn pending_version(&self) -> Option<String> {
        self.staged.version()
    }

    /// Claims the single check slot, or `None` while another check runs. The
    /// slot frees itself when the guard drops, on every return path.
    fn try_begin_check(&self) -> Option<CheckGuard<'_>> {
        self.checking
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| CheckGuard(&self.checking))
    }
}

struct CheckGuard<'a>(&'a AtomicBool);

impl Drop for CheckGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// Whether the background loop should check now, by the wall clock.
fn periodic_check_due(now: u64, last_check: u64) -> bool {
    now.saturating_sub(last_check) >= CHECK_EVERY_SECS
}

fn notify(app: &AppHandle, title: &str, body: &str) {
    let _ = app.notification().builder().title(title).body(body).show();
}

/// One check → silent download+install → pending-restart flag plus a macOS
/// notification. Manual checks also report "up to date" and failures; the
/// background loop only speaks when there is something to act on.
pub async fn check_for_update(app: AppHandle, manual: bool) {
    if tauri::is_dev() || crate::demo::enabled() {
        return;
    }
    // Claimed before the pending check below, so a check that finishes in
    // between cannot be followed by a second download of the same build.
    let status = app.state::<UpdateStatus>();
    let Some(_check) = status.try_begin_check() else {
        if manual {
            notify(
                &app,
                "Checking for updates",
                "An update check is already running.",
            );
        }
        return;
    };
    // An update already downloaded and installed only needs a relaunch.
    // Checking again would download the same build every six hours and
    // announce it as ready each time.
    if let Some(version) = status.pending_version() {
        if manual {
            notify(&app, &format!("UsageBar {version} is ready"), READY_BODY);
        }
        return;
    }
    // On Windows the plugin tears the app down before it launches the
    // installer; if the launch fails the tray would be gone. The hook is
    // emptied, and the explicit install action shuts the children down itself.
    #[cfg(windows)]
    let updater = app.updater_builder().on_before_exit(|| {}).build();
    #[cfg(not(windows))]
    let updater = app.updater();
    let updater = match updater {
        Ok(updater) => updater,
        Err(error) => {
            if manual {
                notify(&app, "Updates unavailable", &error.to_string());
            }
            return;
        }
    };
    let update = match updater.check().await {
        Ok(update) => update,
        Err(error) => {
            if manual {
                notify(&app, "Update check failed", &error.to_string());
            }
            return;
        }
    };
    let Some(update) = update else {
        if manual {
            let current = app.package_info().version.to_string();
            notify(
                &app,
                "UsageBar is up to date",
                &format!("You're running the latest version ({current})."),
            );
        }
        return;
    };

    let version = update.version.clone();
    // Windows keeps the verified installer for an explicit install; macOS
    // installs now and only needs a relaunch.
    #[cfg(windows)]
    let staged = update
        .download(|_received, _total| {}, || {})
        .await
        .map(|bytes| Some((update, bytes)));
    #[cfg(not(windows))]
    let staged = update
        .download_and_install(|_received, _total| {}, || {})
        .await
        .map(|()| None);
    match staged {
        Ok(payload) => {
            status.staged.set(version.clone(), payload);
            notify(&app, &format!("UsageBar {version} is ready"), READY_BODY);
            // Repaint so the menu item flips to the restart/install action.
            app.state::<crate::tray::TrayMenuState>().invalidate();
            crate::tray::refresh_unified_tray(&app).await;
        }
        Err(error) => {
            if manual {
                notify(
                    &app,
                    &format!("UsageBar {version} could not be installed"),
                    &error.to_string(),
                );
            }
        }
    }
}

#[cfg(windows)]
const READY_BODY: &str = "Choose Install Update in the tray menu to finish updating.";
#[cfg(not(windows))]
const READY_BODY: &str = "Restart UsageBar from the menu-bar menu to finish updating.";

/// The tray's "restart-update" action. macOS relaunches into the update it
/// already installed. Windows runs the staged, verified installer once, off
/// the GUI thread; without staged bytes it does nothing, so the app never
/// restarts without an update to apply.
pub fn restart_to_update(app: &AppHandle) {
    #[cfg(not(windows))]
    tauri::process::restart(&app.env());
    #[cfg(windows)]
    install_staged(app);
}

#[cfg(windows)]
fn install_staged(app: &AppHandle) {
    let status = app.state::<UpdateStatus>();
    let Some((version, (update, bytes))) = status.staged.take_payload() else {
        return;
    };
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        // Children stop first, as for a normal quit, outside the installer call.
        if let Some(codex) = handle.try_state::<crate::codex::process::CodexManager>() {
            let codex = codex.inner().clone();
            // If the installer cannot start, keep the watchdog available.
            codex.stop_child().await;
        }
        if let Some(antigravity) = handle.try_state::<crate::antigravity::AntigravityManager>() {
            let antigravity = antigravity.inner().clone();
            antigravity.shutdown().await;
        }
        // A successful install exits the process from inside `install`.
        let outcome = tokio::task::spawn_blocking(move || {
            let result = update.install(&bytes);
            (result, update, bytes)
        })
        .await;
        let status = handle.state::<UpdateStatus>();
        match outcome {
            Ok((result, update, bytes)) => {
                status.staged.restore((update, bytes));
                let message = match result {
                    Err(error) => error.to_string(),
                    Ok(()) => "The installer returned without updating.".to_owned(),
                };
                notify(
                    &handle,
                    &format!("UsageBar {version} could not be installed"),
                    &format!("{message} Choose Install Update to try again."),
                );
            }
            Err(_) => {
                // The package is gone with the task; a fresh check downloads it again.
                status.staged.clear();
                notify(
                    &handle,
                    &format!("UsageBar {version} could not be installed"),
                    "The update will be downloaded again at the next check.",
                );
            }
        }
        handle.state::<crate::tray::TrayMenuState>().invalidate();
        crate::tray::refresh_unified_tray(&handle).await;
    });
}

/// Periodic background checks. Skipped in dev: a `cargo tauri dev` build has
/// no `.app` bundle for the updater to replace.
pub fn spawn_periodic_checks(app: &AppHandle) {
    if tauri::is_dev() {
        return;
    }
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(FIRST_CHECK_SECS)).await;
        let mut last_check = 0;
        let mut interval = tokio::time::interval(Duration::from_secs(CLOCK_TICK_SECS));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let now = crate::provider::now_unix_seconds();
            if !periodic_check_due(now, last_check) {
                continue;
            }
            last_check = now;
            check_for_update(handle.clone(), false).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_one_update_check_runs_at_a_time() {
        let status = UpdateStatus::default();
        let first = status
            .try_begin_check()
            .expect("first check claims the slot");
        assert!(status.try_begin_check().is_none());
        drop(first);
        // Finishing (or bailing out of) a check frees the slot again.
        assert!(status.try_begin_check().is_some());
    }

    #[test]
    fn background_checks_follow_the_wall_clock() {
        let now = 1_000_000;
        // The first tick after launch always checks.
        assert!(periodic_check_due(now, 0));
        assert!(!periodic_check_due(now, now - 60));
        assert!(!periodic_check_due(now, now - CHECK_EVERY_SECS + 1));
        // Six wall-clock hours, however many of them the Mac slept through.
        assert!(periodic_check_due(now, now - CHECK_EVERY_SECS));
        // A clock set backwards waits rather than checking on every tick.
        assert!(!periodic_check_due(now, now + 3_600));
    }

    #[test]
    fn a_pending_update_is_remembered() {
        let status = UpdateStatus::default();
        assert_eq!(status.pending_version(), None);
        status.staged.set("1.2.3".to_owned(), None);
        assert_eq!(status.pending_version().as_deref(), Some("1.2.3"));
    }

    #[test]
    fn a_staged_package_can_be_taken_once_and_comes_back_after_a_failure() {
        let staged: Staged<Vec<u8>> = Staged::default();
        assert!(staged.take_payload().is_none());
        staged.set("1.2.3".to_owned(), Some(vec![1, 2, 3]));
        let (version, bytes) = staged.take_payload().expect("first click gets it");
        assert_eq!(
            (version.as_str(), bytes.as_slice()),
            ("1.2.3", &[1, 2, 3][..])
        );
        // A second click while the installer runs gets nothing, and the
        // update stays announced.
        assert!(staged.take_payload().is_none());
        assert_eq!(staged.version().as_deref(), Some("1.2.3"));
        // The installer returned an error: the same verified bytes return.
        staged.restore(bytes);
        assert_eq!(
            staged.take_payload().map(|(_, bytes)| bytes),
            Some(vec![1, 2, 3])
        );
    }

    #[test]
    fn restoring_or_clearing_leaves_no_phantom_update() {
        let staged: Staged<u8> = Staged::default();
        staged.restore(9);
        assert!(staged.version().is_none() && staged.take_payload().is_none());
        staged.set("2.0.0".to_owned(), Some(1));
        staged.clear();
        assert!(staged.version().is_none());
    }
}
