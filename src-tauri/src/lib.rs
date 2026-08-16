mod alerts;
mod claude;
mod codex;
mod commands;
mod notch;
mod prefs;
mod tray;

use claude::ClaudeManager;
use codex::process::CodexManager;
use tauri::{Manager, WindowEvent};

/// Meters refresh when their data is older than this.
const STALE_AFTER_SECS: u64 = 5 * 60;
/// Minimum spacing between refresh attempts, so failures cannot hammer.
const MIN_RETRY_SECS: u64 = 4 * 60;

fn now_unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Opts the process out of App Nap. Without this, macOS throttles the process
/// timers as soon as the popover hides, which froze both the 1-second tray
/// countdown and the periodic refresh until the user clicked the menu bar.
#[cfg(target_os = "macos")]
fn disable_app_nap() {
    use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};
    let reason = NSString::from_str("UsageBar keeps menu-bar meters live");
    let activity = NSProcessInfo::processInfo().beginActivityWithOptions_reason(
        NSActivityOptions::UserInitiatedAllowingIdleSystemSleep,
        &reason,
    );
    // The assertion must outlive the whole process.
    std::mem::forget(activity);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .setup(|app| {
            #[cfg(target_os = "macos")]
            {
                // Regular keeps the Dock icon around for development; releases
                // behave like a proper menu-bar accessory app.
                #[cfg(debug_assertions)]
                app.set_activation_policy(tauri::ActivationPolicy::Regular);
                #[cfg(not(debug_assertions))]
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);
                disable_app_nap();
            }

            let prefs_path = app.path().app_config_dir()?.join("preferences.json");
            app.manage(prefs::PrefsStore::load(prefs_path));
            app.manage(alerts::UsageAlerts::default());
            app.manage(tray::ResetRadar::default());
            app.manage(tray::TrayMenuState::default());
            let manager = CodexManager::new(app.handle().clone());
            app.manage(manager.clone());
            let claude_manager = ClaudeManager::new(app.handle().clone());
            app.manage(claude_manager.clone());
            tray::setup(app)?;
            notch::setup(app)?;

            #[cfg(target_os = "macos")]
            if let Some(window) = app.get_webview_window("main") {
                use window_vibrancy::{
                    apply_vibrancy, NSVisualEffectMaterial, NSVisualEffectState,
                };
                let _ = window.set_minimizable(true);
                let _ = window.set_closable(true);
                let _ = window.set_always_on_top(false);
                #[cfg(debug_assertions)]
                if let Err(error) = apply_vibrancy(
                    &window,
                    NSVisualEffectMaterial::Popover,
                    Some(NSVisualEffectState::Active),
                    Some(16.0),
                ) {
                    eprintln!("usagebar: vibrancy unavailable: {error}");
                }
                #[cfg(not(debug_assertions))]
                let _ = apply_vibrancy(
                    &window,
                    NSVisualEffectMaterial::Popover,
                    Some(NSVisualEffectState::Active),
                    Some(16.0),
                );
            }

            #[cfg(debug_assertions)]
            if let Some(window) = app.get_webview_window("main") {
                window.show()?;
                window.set_focus()?;
            }

            let tray_ticker = manager.clone();
            let claude_ticker = claude_manager.clone();
            tauri::async_runtime::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    interval.tick().await;
                    tray_ticker.update_tray().await;
                    claude_ticker.update_tray().await;
                }
            });

            tauri::async_runtime::spawn(async move {
                let _ = manager.start().await;
            });
            let claude_starter = claude_manager.clone();
            tauri::async_runtime::spawn(async move {
                let _ = claude_starter.refresh().await;
            });

            // Wall-clock staleness watchdogs instead of a plain sleep loop:
            // tokio timers pause during system sleep, so a fixed 5-minute
            // sleep silently stretched across naps. Checking `updated_at`
            // against the wall clock every 30 seconds catches up within one
            // tick of waking, while MIN_RETRY_SECS keeps failures from
            // hammering the backends.
            let refresh_manager = app.state::<CodexManager>().inner().clone();
            tauri::async_runtime::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                let mut last_attempt = now_unix_seconds();
                loop {
                    interval.tick().await;
                    let now = now_unix_seconds();
                    let snapshot = refresh_manager.snapshot().await;
                    let fresh = snapshot
                        .updated_at
                        .is_some_and(|at| now.saturating_sub(at) < STALE_AFTER_SECS);
                    if fresh || now.saturating_sub(last_attempt) < MIN_RETRY_SECS {
                        continue;
                    }
                    last_attempt = now;
                    let _ = refresh_manager.refresh_or_start().await;
                }
            });
            let claude_refresher = claude_manager.clone();
            tauri::async_runtime::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                let mut last_attempt = now_unix_seconds();
                loop {
                    interval.tick().await;
                    let now = now_unix_seconds();
                    let snapshot = claude_refresher.snapshot().await;
                    let fresh = snapshot
                        .updated_at
                        .is_some_and(|at| now.saturating_sub(at) < STALE_AFTER_SECS);
                    if fresh || now.saturating_sub(last_attempt) < MIN_RETRY_SECS {
                        continue;
                    }
                    last_attempt = now;
                    let _ = claude_refresher.refresh().await;
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_codex_state,
            commands::refresh_codex,
            commands::get_claude_state,
            commands::refresh_claude,
            commands::set_reset_incoming,
            commands::get_app_prefs,
            commands::complete_onboarding,
            commands::get_notch_status,
            commands::set_notch_mode,
            commands::set_notch_expanded,
            commands::show_main_window,
            commands::write_share_card,
            commands::quit_app
        ])
        .run(tauri::generate_context!())
        .expect("error while running UsageBar");
}
