//! Self-update checks against the signed GitHub Releases manifest.
//!
//! A background loop checks once the app has settled and then every six
//! hours; a "Check for Updates…" tray item runs the same check on demand. A
//! downloaded update parks its version on `UpdateStatus`, which swaps the
//! menu item for "Restart to Update — vX.Y.Z" on the next tray repaint.

use std::sync::Mutex;
use std::time::Duration;

use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_updater::UpdaterExt;

/// The first check waits until launch settles — startup is already busy
/// finding CLIs, reading keychains, and painting the tray.
const FIRST_CHECK_SECS: u64 = 45;
const CHECK_EVERY_SECS: u64 = 6 * 60 * 60;

/// The version of an update that downloaded and is waiting on a relaunch.
/// The tray reads this to offer the restart, and `sync_menus` folds it into
/// the menu signature so a landing update rebuilds the menus once.
#[derive(Default)]
pub struct UpdateStatus(Mutex<Option<String>>);

impl UpdateStatus {
    pub fn pending_version(&self) -> Option<String> {
        self.0.lock().expect("update status poisoned").clone()
    }

    fn set_pending(&self, version: String) {
        *self.0.lock().expect("update status poisoned") = Some(version);
    }
}

fn notify(app: &AppHandle, title: &str, body: &str) {
    let _ = app
        .notification()
        .builder()
        .title(title)
        .body(body)
        .show();
}

/// One check → silent download+install → pending-restart flag plus a macOS
/// notification. Manual checks also report "up to date" and failures; the
/// background loop only speaks when there is something to act on.
pub async fn check_for_update(app: AppHandle, manual: bool) {
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
            app.state::<UpdateStatus>().set_pending(version.clone());
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
    if tauri::is_dev() {
        return;
    }
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(FIRST_CHECK_SECS)).await;
        loop {
            check_for_update(handle.clone(), false).await;
            tokio::time::sleep(Duration::from_secs(CHECK_EVERY_SECS)).await;
        }
    });
}
