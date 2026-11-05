mod alerts;
mod antigravity;
mod claude;
mod codex;
mod commands;
mod cursor;
mod demo;
mod devin;
mod gemini;
mod opencode;
mod platform;
mod prefs;
mod provider;
mod registry;
mod tray;
mod updates;

use tauri::{Manager, WindowEvent};

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

/// Brings the app to the foreground so a just-shown window becomes key.
///
/// An `Accessory`-policy (menu-bar) app is not activated when a window is
/// merely shown, so on modern macOS the window stays non-key: its traffic-light
/// buttons render inactive and the first click on them is swallowed as an
/// activation click rather than a press. Tauri's `set_focus` tries to fix this
/// with `activateIgnoringOtherApps:`, which is deprecated and unreliable on
/// recent macOS. Activating the running application directly is dependable back
/// to 10.6 and makes the window controls live immediately.
#[cfg(target_os = "macos")]
pub(crate) fn activate_app() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;

    // Window activation must happen on the main thread.
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    // Dismissing the popover hides the whole application (see `hide_app`), and
    // a hidden app stays hidden through `activate` alone.
    app.unhide(None);
    // macOS 14 replaced `activateIgnoringOtherApps:` (now a no-op) with the
    // cooperative `activate`, which the system grants for user-initiated events
    // like the status-item click that shows this window.
    if objc2::available!(macos = 14.0) {
        app.activate();
    } else {
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
    }
}

/// Removes the native window buttons entirely; the toolbar's own dismiss
/// control replaces them.
///
/// The popover's titlebar strip is transparent — the glass card starts below
/// it — so the traffic lights floated over a see-through area where a click a
/// few pixels wide of a button passed through the window to whatever app sat
/// behind, activating it. That read as "the window slipped behind". Minimize
/// and zoom had nothing to do here anyway: an `Accessory` app has no Dock icon
/// to restore a miniaturized window from, and the window is fixed-size.
#[cfg(target_os = "macos")]
fn hide_native_window_buttons(window: &tauri::WebviewWindow) {
    use objc2_app_kit::{NSWindow, NSWindowButton};

    let Ok(ptr) = window.ns_window() else {
        return;
    };
    if ptr.is_null() {
        return;
    }
    // Tauri hands back the `NSWindow` backing this webview window, and setup
    // runs on the main thread, where AppKit views may be touched.
    let ns_window: &NSWindow = unsafe { &*(ptr as *const NSWindow) };
    for kind in [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ] {
        if let Some(button) = ns_window.standardWindowButton(kind) {
            button.setHidden(true);
        }
    }
}

/// Dismisses the popover the way the menu bar item does: the window goes away
/// and UsageBar stops being the active app.
///
/// `hide()` on its own only orders the window out, leaving UsageBar frontmost
/// with the menu bar and keyboard focus still its own, so the window the user
/// came from never comes back forward. Hiding the application returns both.
#[cfg(target_os = "macos")]
pub(crate) fn hide_app() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    NSApplication::sharedApplication(mtm).hide(None);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            if cfg!(target_os = "windows") {
                Some(vec!["--startup"])
            } else {
                None
            },
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
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
            app.manage(updates::UpdateStatus::default());
            app.manage(platform::WidgetPlacement::default());
            // Every provider's manager, plus the list the tray and watchdogs
            // iterate. Before the tray, which reads it on every repaint.
            registry::install(app);
            tray::setup(app)?;
            #[cfg(target_os = "windows")]
            {
                if let Some(widget) = app.get_webview_window("codex-widget") {
                    let _ = widget.set_always_on_top(true);
                    let _ = widget.set_size(tauri::LogicalSize::new(328.0, 26.0));
                    let _ = widget.show();
                    let _ = widget.unminimize();
                    let _ = platform::position_codex_widget(app.handle());
                }

                // Windows shell/taskbar may still be initializing work area at boot.
                // Re-dock periodically over the first few seconds so it doesn't get buried.
                let startup_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    for delay_ms in [300, 800, 1500, 3000, 5000, 8000] {
                        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                        let _ = platform::position_codex_widget(&startup_handle);
                    }
                });
            }
            if demo::enabled() {
                // Sample quotas only. The updater would call the network, and
                // the provider tasks below no-op inside their refresh paths.
                demo::log_enablement();
                demo::spawn_control(app.handle().clone());
            } else {
                updates::spawn_periodic_checks(app.handle());
            }

            #[cfg(target_os = "macos")]
            if let Some(window) = app.get_webview_window("main") {
                use window_vibrancy::{
                    apply_vibrancy, NSVisualEffectMaterial, NSVisualEffectState,
                };
                // Minimize/zoom/closable are all set in tauri.conf.json, which
                // applies them when the window is created. They are deliberately
                // not re-set at runtime here.
                hide_native_window_buttons(&window);
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

            #[cfg(any(debug_assertions, target_os = "windows"))]
            if !cfg!(target_os = "windows") || !std::env::args_os().any(|arg| arg == "--startup") {
                if let Some(window) = app.get_webview_window("main") {
                    if demo::enabled() {
                        demo::place_popover(&window);
                        let _ = window.set_always_on_top(true);
                    }
                    window.show()?;
                    window.set_focus()?;
                }
            }

            // Every provider shares one tray coordinator, so a single repaint per
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
                    #[cfg(target_os = "windows")]
                    if provider::now_unix_seconds() % 5 == 0 {
                        let handle = tray_handle.clone();
                        let _ = tray_handle.run_on_main_thread(move || {
                            let _ = platform::position_codex_widget(&handle);
                        });
                    }
                }
            });

            // Each provider's first refresh, then its staleness watchdog.
            // The timing per provider lives with its `Meter` impl.
            registry::spawn_refreshers(app.handle());
            Ok(())
        })
        .on_window_event(|window, event| {
            // Cmd+W and any programmatic close still route through here; the
            // window is a popover, so it hides instead of being destroyed.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
                #[cfg(target_os = "macos")]
                hide_app();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_codex_state,
            commands::refresh_codex,
            commands::get_claude_state,
            commands::refresh_claude,
            commands::get_cursor_state,
            commands::refresh_cursor,
            commands::get_opencode_state,
            commands::refresh_opencode,
            commands::get_devin_state,
            commands::refresh_devin,
            commands::get_antigravity_state,
            commands::refresh_antigravity,
            commands::get_gemini_state,
            commands::refresh_gemini,
            commands::get_provider_state,
            commands::refresh_provider,
            commands::set_reset_incoming,
            commands::get_app_prefs,
            commands::complete_onboarding,
            commands::get_tray_windows,
            commands::set_tray_window,
            commands::set_provider_visible,
            commands::set_usage_alerts,
            commands::set_reset_alerts,
            commands::open_post_url,
            commands::set_dense_layout,
            commands::set_usage_alert_thresholds,
            commands::set_combined_tray,
            commands::get_autostart,
            commands::set_autostart,
            commands::hide_window,
            commands::show_dashboard,
            commands::set_widget_expanded,
            commands::drag_codex_widget,
            commands::hide_codex_widget,
            commands::reset_widget_position,
            commands::is_demo_mode,
            commands::quit_app
        ])
        .run(tauri::generate_context!())
        .expect("error while running UsageBar");
}
