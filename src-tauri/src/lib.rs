mod alerts;
mod claude;
mod codex;
mod commands;
mod prefs;
mod tray;

use claude::ClaudeManager;
use codex::process::CodexManager;
use tauri::{Manager, WindowEvent};

/// Meters refresh when their data is older than this.
const STALE_AFTER_SECS: u64 = 5 * 60;
/// Steady-state spacing between refresh attempts, so a healthy meter cannot
/// hammer its backend.
const MIN_RETRY_SECS: u64 = 4 * 60;
/// Spacing after the first failed attempt. A denied keychain prompt or a
/// dropped network call recovers on the next watchdog tick instead of leaving
/// the meter wrong (or hidden) for minutes.
const FIRST_RETRY_SECS: u64 = 30;

fn now_unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// How long to wait before the next attempt: healthy meters keep the steady
/// cadence, while a run of failures backs off from `FIRST_RETRY_SECS` up to the
/// same ceiling, so a persistent outage (an expired session that only Claude
/// Code itself can renew) is not retried any harder than before.
fn retry_after_secs(consecutive_failures: u32) -> u64 {
    if consecutive_failures == 0 {
        return MIN_RETRY_SECS;
    }
    let doublings = consecutive_failures.saturating_sub(1).min(16);
    FIRST_RETRY_SECS
        .saturating_mul(1_u64 << doublings)
        .min(MIN_RETRY_SECS)
}

/// Whether a meter is due for a refresh. Data younger than `STALE_AFTER_SECS`
/// is left alone; anything older waits only for the retry spacing.
fn should_refresh(
    now: u64,
    updated_at: Option<u64>,
    last_attempt: u64,
    consecutive_failures: u32,
) -> bool {
    let fresh = updated_at.is_some_and(|at| now.saturating_sub(at) < STALE_AFTER_SECS);
    if fresh {
        return false;
    }
    now.saturating_sub(last_attempt) >= retry_after_secs(consecutive_failures)
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
            app.manage(tray::TrayRenderCache::default());
            let manager = CodexManager::new(app.handle().clone());
            app.manage(manager.clone());
            let claude_manager = ClaudeManager::new(app.handle().clone());
            app.manage(claude_manager.clone());
            tray::setup(app)?;

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

            // Both providers share one tray coordinator, so a single repaint per
            // tick keeps the menu-bar countdown live. Refreshing once (rather
            // than once per manager, as before) halves the per-second work and
            // removes a redundant second pass over the same AppKit items.
            let tray_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    interval.tick().await;
                    tray::refresh_unified_tray(&tray_handle).await;
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
            // tick of waking, while the retry spacing keeps failures from
            // hammering the backends.
            //
            // Success is measured by `updated_at` moving rather than by the
            // returned Result: both providers report "signed out" as an Ok
            // state, and that still leaves the meter frozen.
            let refresh_manager = app.state::<CodexManager>().inner().clone();
            tauri::async_runtime::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                let mut last_attempt = now_unix_seconds();
                // Start in the failure lane: if the startup refresh did not
                // land, the meter recovers on the next tick instead of waiting
                // out the steady-state interval. A refresh that did land is
                // fresh, so the watchdog skips anyway.
                let mut failures: u32 = 1;
                loop {
                    interval.tick().await;
                    let now = now_unix_seconds();
                    let before = refresh_manager.snapshot().await.updated_at;
                    if !should_refresh(now, before, last_attempt, failures) {
                        continue;
                    }
                    last_attempt = now;
                    let _ = refresh_manager.refresh_or_start().await;
                    let after = refresh_manager.snapshot().await.updated_at;
                    failures = if after == before {
                        failures.saturating_add(1)
                    } else {
                        0
                    };
                }
            });
            let claude_refresher = claude_manager.clone();
            tauri::async_runtime::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                let mut last_attempt = now_unix_seconds();
                // Start in the failure lane: if the startup refresh did not
                // land, the meter recovers on the next tick instead of waiting
                // out the steady-state interval. A refresh that did land is
                // fresh, so the watchdog skips anyway.
                let mut failures: u32 = 1;
                loop {
                    interval.tick().await;
                    let now = now_unix_seconds();
                    let before = claude_refresher.snapshot().await.updated_at;
                    if !should_refresh(now, before, last_attempt, failures) {
                        continue;
                    }
                    last_attempt = now;
                    let _ = claude_refresher.refresh().await;
                    let after = claude_refresher.snapshot().await.updated_at;
                    failures = if after == before {
                        failures.saturating_add(1)
                    } else {
                        0
                    };
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
            commands::get_tray_windows,
            commands::set_tray_window,
            commands::set_usage_alerts,
            commands::set_combined_tray,
            commands::get_autostart,
            commands::set_autostart,
            commands::write_share_card,
            commands::quit_app
        ])
        .run(tauri::generate_context!())
        .expect("error while running UsageBar");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_retry_quickly_then_back_off_to_the_steady_cadence() {
        assert_eq!(retry_after_secs(0), MIN_RETRY_SECS);
        assert_eq!(retry_after_secs(1), FIRST_RETRY_SECS);
        assert_eq!(retry_after_secs(2), 60);
        assert_eq!(retry_after_secs(3), 120);
        // The backoff never exceeds the healthy cadence, so a session only
        // Claude Code can renew is not polled harder than before.
        assert_eq!(retry_after_secs(4), MIN_RETRY_SECS);
        assert_eq!(retry_after_secs(u32::MAX), MIN_RETRY_SECS);
    }

    #[test]
    fn fresh_data_is_left_alone_and_stale_data_retries_on_schedule() {
        let now = 1_000_000;
        // Data younger than the staleness window is never re-fetched.
        assert!(!should_refresh(now, Some(now - 60), now - 3_600, 0));
        // Stale data waits out the steady cadence after a healthy run...
        assert!(!should_refresh(now, Some(now - STALE_AFTER_SECS), now - 60, 0));
        assert!(should_refresh(now, Some(now - STALE_AFTER_SECS), now - MIN_RETRY_SECS, 0));
        // ...but a failed attempt is retried on the next watchdog tick, which
        // is what pulls a hidden or frozen Claude meter back within seconds.
        assert!(should_refresh(now, Some(now - STALE_AFTER_SECS), now - FIRST_RETRY_SECS, 1));
        assert!(!should_refresh(now, Some(now - STALE_AFTER_SECS), now - 10, 1));
        // A meter that never got data at all is due as soon as spacing allows.
        assert!(should_refresh(now, None, now - FIRST_RETRY_SECS, 1));
        assert!(!should_refresh(now, None, now, 1));
    }
}
