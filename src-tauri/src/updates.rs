//! Self-update checks against the signed GitHub Releases manifest.
//!
//! A background loop checks once the app has settled and then every six
//! hours; a "Check for Updates…" tray item runs the same check on demand. A
//! downloaded update parks its version on `UpdateStatus`, which swaps the
//! menu item for "Restart to Update — vX.Y.Z" on the next tray repaint.

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

/// The version of an update that downloaded and is waiting on a relaunch.
/// The tray reads this to offer the restart, and `sync_menus` folds it into
/// the menu signature so a landing update rebuilds the menus once.
#[derive(Default)]
pub struct UpdateStatus {
    pending: Mutex<Option<String>>,
    /// Set while a check (and its download) runs, so a manual check during
    /// the background one does not download and install the same build twice
    /// at once.
    checking: AtomicBool,
}

impl UpdateStatus {
    pub fn pending_version(&self) -> Option<String> {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn set_pending(&self, version: String) {
        *self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(version);
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
    // This Windows port is distributed manually; the upstream feed ships macOS builds.
    if cfg!(target_os = "windows") {
        if manual {
            notify(&app, "Windows updates", "Install a newer Windows installer to update UsageBar. Automatic updates are not configured for this port.");
        }
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
            notify(
                &app,
                &format!("UsageBar {version} is ready"),
                "Restart UsageBar from the menu-bar menu to finish updating.",
            );
        }
        return;
    }
    let updater = match app.updater() {
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
    let download = update
        .download_and_install(|_received, _total| {}, || {})
        .await;
    match download {
        Ok(()) => {
            status.set_pending(version.clone());
            notify(
                &app,
                &format!("UsageBar {version} is ready"),
                "Restart UsageBar from the menu-bar menu to finish updating.",
            );
            // Repaint so the menu item flips to "Restart to Update".
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

/// Periodic background checks. Skipped in dev: a `cargo tauri dev` build has
/// no `.app` bundle for the updater to replace.
pub fn spawn_periodic_checks(app: &AppHandle) {
    if tauri::is_dev() || cfg!(target_os = "windows") {
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
        status.set_pending("1.2.3".to_owned());
        assert_eq!(status.pending_version().as_deref(), Some("1.2.3"));
    }
}
