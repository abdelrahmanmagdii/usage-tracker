use std::collections::BTreeMap;

use tauri::{AppHandle, Manager, State};

use crate::antigravity::AntigravityManager;
use crate::claude::{ClaudeManager, ClaudeState};
use crate::codex::process::{CodexManager, CodexState};
use crate::cursor::CursorManager;
use crate::devin::DevinManager;
use crate::gemini::GeminiManager;
use crate::opencode::OpenCodeManager;
use crate::provider::ProviderState;
use crate::registry::{Meters, Provider};

#[tauri::command]
pub async fn get_codex_state(manager: State<'_, CodexManager>) -> Result<CodexState, String> {
    Ok(manager.snapshot().await)
}

#[tauri::command]
pub async fn refresh_codex(manager: State<'_, CodexManager>) -> Result<CodexState, String> {
    manager.refresh_or_start().await
}

#[tauri::command]
pub async fn get_claude_state(manager: State<'_, ClaudeManager>) -> Result<ClaudeState, String> {
    Ok(manager.snapshot().await)
}

#[tauri::command]
pub async fn refresh_claude(manager: State<'_, ClaudeManager>) -> Result<ClaudeState, String> {
    manager.refresh_from_user().await
}

#[tauri::command]
pub async fn get_cursor_state(manager: State<'_, CursorManager>) -> Result<ProviderState, String> {
    Ok(manager.snapshot().await)
}

#[tauri::command]
pub async fn refresh_cursor(manager: State<'_, CursorManager>) -> Result<ProviderState, String> {
    manager.refresh().await
}

#[tauri::command]
pub async fn get_opencode_state(
    manager: State<'_, OpenCodeManager>,
) -> Result<ProviderState, String> {
    Ok(manager.snapshot().await)
}

#[tauri::command]
pub async fn refresh_opencode(
    manager: State<'_, OpenCodeManager>,
) -> Result<ProviderState, String> {
    manager.refresh().await
}

#[tauri::command]
pub async fn get_devin_state(manager: State<'_, DevinManager>) -> Result<ProviderState, String> {
    Ok(manager.snapshot().await)
}

#[tauri::command]
pub async fn refresh_devin(manager: State<'_, DevinManager>) -> Result<ProviderState, String> {
    manager.refresh().await
}

#[tauri::command]
pub async fn get_antigravity_state(
    manager: State<'_, AntigravityManager>,
) -> Result<ProviderState, String> {
    Ok(manager.snapshot().await)
}

#[tauri::command]
pub async fn refresh_antigravity(
    manager: State<'_, AntigravityManager>,
) -> Result<ProviderState, String> {
    manager.refresh_from_user().await
}

#[tauri::command]
pub async fn get_gemini_state(manager: State<'_, GeminiManager>) -> Result<ProviderState, String> {
    Ok(manager.snapshot().await)
}

#[tauri::command]
pub async fn refresh_gemini(manager: State<'_, GeminiManager>) -> Result<ProviderState, String> {
    manager.refresh().await
}

/// Any provider's snapshot by id. The per-provider commands above stay for
/// the renderer that already calls them; this one needs no new code when a
/// provider is added.
#[tauri::command]
pub async fn get_provider_state(
    meters: State<'_, Meters>,
    provider: String,
) -> Result<serde_json::Value, String> {
    meters.get(Provider::parse(&provider)?).state_json().await
}

/// The Retry button for any provider by id, with the same semantics as the
/// matching `refresh_*` command.
#[tauri::command]
pub async fn refresh_provider(
    meters: State<'_, Meters>,
    provider: String,
) -> Result<serde_json::Value, String> {
    meters.get(Provider::parse(&provider)?).user_refresh().await
}

#[tauri::command]
pub fn get_app_prefs(prefs: State<'_, crate::prefs::PrefsStore>) -> crate::prefs::AppPrefs {
    let mut prefs = prefs.get();
    if crate::demo::enabled() {
        // Session-only: the walkthrough would cover the sample meters. This
        // copy is not written, so a later launch without --demo is unchanged.
        prefs.onboarding_complete = true;
    }
    prefs
}

/// True only for an explicit `--demo` / `USAGEBAR_DEMO=1` launch.
#[tauri::command]
pub fn is_demo_mode() -> bool {
    crate::demo::enabled()
}

/// Windows each provider currently reports, for the in-app menu-bar picker,
/// keyed by provider id (`{ "codex": [...], "claude": [...], ... }`).
#[tauri::command]
pub async fn get_tray_windows(
    meters: State<'_, Meters>,
) -> Result<BTreeMap<&'static str, Vec<crate::tray::TrayWindow>>, String> {
    let mut windows = BTreeMap::new();
    for meter in meters.iter() {
        let reading = meter.reading().await;
        windows.insert(
            meter.provider().key(),
            crate::tray::collect_windows(reading.rate_limits.as_ref()),
        );
    }
    Ok(windows)
}

#[tauri::command]
pub fn set_tray_window(app: AppHandle, provider: String, window: String) -> Result<(), String> {
    let provider = Provider::parse(&provider)?;
    app.state::<crate::prefs::PrefsStore>()
        .update(|prefs| prefs.set_tray_window(provider.key(), window));
    crate::tray::apply_preference_change(&app);
    Ok(())
}

#[tauri::command]
pub fn set_provider_visible(app: AppHandle, provider: String, visible: bool) -> Result<(), String> {
    let provider = Provider::parse(&provider)?;
    app.state::<crate::prefs::PrefsStore>()
        .update(|prefs| prefs.set_visible(provider.key(), visible));
    crate::tray::apply_preference_change(&app);
    if visible {
        crate::tray::refresh_provider(&app, provider);
    }
    Ok(())
}

#[tauri::command]
pub fn set_usage_alerts(app: AppHandle, enabled: bool) {
    app.state::<crate::prefs::PrefsStore>()
        .update(|prefs| prefs.usage_alerts = enabled);
    crate::tray::apply_preference_change(&app);
}

/// "Runs out before it resets at this pace" notifications. They also need
/// `usage_alerts` on; this switch only narrows what that one allows.
#[tauri::command]
pub fn set_pace_alerts(app: AppHandle, enabled: bool) {
    app.state::<crate::prefs::PrefsStore>()
        .update(|prefs| prefs.pace_alerts = enabled);
    crate::tray::apply_preference_change(&app);
}

/// Only post links from the reset feed may be handed to `open`, so the
/// webview cannot launch arbitrary URLs or files.
fn is_openable_post_url(url: &str) -> bool {
    const ALLOWED: [&str; 3] = [
        "https://bsky.app/",
        "https://x.com/",
        "https://twitter.com/",
    ];
    ALLOWED.iter().any(|prefix| url.starts_with(prefix))
        && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}

#[tauri::command]
pub fn open_post_url(url: String) -> Result<(), String> {
    if !is_openable_post_url(&url) {
        return Err("Only reset post links can be opened.".to_owned());
    }
    open_in_browser(&url)
}

/// Hands a vetted https link to the default browser.
pub fn open_in_browser(url: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        #[link(name = "shell32")]
        extern "system" {
            fn ShellExecuteW(
                hwnd: *mut std::ffi::c_void,
                operation: *const u16,
                file: *const u16,
                parameters: *const u16,
                directory: *const u16,
                show: i32,
            ) -> isize;
        }
        let url: Vec<u16> = url.encode_utf16().chain(Some(0)).collect();
        let result = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                std::ptr::null(),
                url.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1,
            )
        };
        if result <= 32 {
            return Err(format!("Could not open browser (Windows error {result})"));
        }
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let opener = if cfg!(target_os = "macos") {
            "/usr/bin/open"
        } else {
            "xdg-open"
        };
        std::process::Command::new(opener)
            .arg(url)
            .spawn()
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

#[tauri::command]
pub fn set_reset_alerts(app: AppHandle, enabled: bool) {
    app.state::<crate::prefs::PrefsStore>()
        .update(|prefs| prefs.reset_alerts = enabled);
    crate::tray::apply_preference_change(&app);
}

#[tauri::command]
pub fn set_dense_layout(app: AppHandle, enabled: bool) {
    app.state::<crate::prefs::PrefsStore>()
        .update(|prefs| prefs.dense_layout = enabled);
    crate::tray::apply_preference_change(&app);
}

/// The percent-used levels that fire a usage alert. Stored ascending; the
/// engine reads them descending so a jump past two levels fires only the
/// higher one.
#[tauri::command]
pub fn set_usage_alert_thresholds(app: AppHandle, thresholds: Vec<u32>) -> Result<(), String> {
    let normalized = crate::prefs::normalize_alert_thresholds(&thresholds)?;
    app.state::<crate::prefs::PrefsStore>()
        .update(|prefs| prefs.usage_alert_thresholds = normalized);
    crate::tray::apply_preference_change(&app);
    Ok(())
}

/// `true` = one combined menu-bar icon; `false` = one icon per provider.
#[tauri::command]
pub fn set_combined_tray(app: AppHandle, enabled: bool) {
    app.state::<crate::prefs::PrefsStore>()
        .update(|prefs| prefs.combined_tray = enabled);
    crate::tray::apply_preference_change(&app);
}

#[tauri::command]
pub fn get_autostart(app: AppHandle) -> bool {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
pub fn set_autostart(app: AppHandle, enabled: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    let autolaunch = app.autolaunch();
    let result = if enabled {
        autolaunch.enable()
    } else {
        autolaunch.disable()
    };
    result.map_err(|error| error.to_string())?;
    crate::tray::apply_preference_change(&app);
    Ok(())
}

/// Records that the first-run walkthrough finished so it does not reappear.
#[tauri::command]
pub fn complete_onboarding(prefs: State<'_, crate::prefs::PrefsStore>) {
    prefs.update(|prefs| prefs.onboarding_complete = true);
}

/// The renderer reports when an announced reset is pending (`until` = unix
/// seconds it lands, `None` to clear); the Codex tray shows ⚡ while pending.
#[tauri::command]
pub fn set_reset_incoming(
    app: AppHandle,
    radar: State<'_, crate::tray::ResetRadar>,
    until: Option<u64>,
) {
    radar
        .0
        .store(until.unwrap_or(0), std::sync::atomic::Ordering::Relaxed);
    let manager = app.state::<CodexManager>().inner().clone();
    tauri::async_runtime::spawn(async move {
        manager.update_tray().await;
    });
}

#[tauri::command]
pub async fn quit_app(app: AppHandle) {
    let manager = app.state::<CodexManager>().inner().clone();
    manager.shutdown().await;
    app.exit(0);
}

/// Dismisses the popover from the UI: the window goes away and UsageBar stops
/// being the active app, so focus returns to whatever the user was in.
#[tauri::command]
pub fn hide_window(app: AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
    #[cfg(target_os = "macos")]
    crate::hide_app();
}

#[tauri::command]
pub fn set_widget_expanded(
    app: AppHandle,
    expanded: bool,
    width: Option<f64>,
    height: Option<f64>,
) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    if let Some(window) = app.get_webview_window("codex-widget") {
        let default_width = if expanded { 354.0 } else { 328.0 };
        let default_height = if expanded { 160.0 } else { 26.0 };
        let target_width = width.unwrap_or(default_width);
        let target_height = height.unwrap_or(default_height);
        window
            .set_size(tauri::LogicalSize::new(target_width, target_height))
            .map_err(|error| error.to_string())?;
        if crate::demo::enabled() && expanded {
            crate::demo::place_demo_windows(&app);
        } else {
            crate::platform::position_codex_widget(&app).map_err(|error| error.to_string())?;
        }
    }
    #[cfg(not(target_os = "windows"))]
    let _ = (app, expanded, width, height);
    Ok(())
}

#[tauri::command]
pub fn drag_codex_widget(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("codex-widget") {
        app.state::<crate::platform::WidgetPlacement>()
            .manually_moved
            .store(true, std::sync::atomic::Ordering::Relaxed);
        #[cfg(target_os = "windows")]
        {
            #[repr(C)]
            struct Point {
                x: i32,
                y: i32,
            }
            #[link(name = "user32")]
            extern "system" {
                fn GetCursorPos(point: *mut Point) -> i32;
                fn ReleaseCapture() -> i32;
                fn SendMessageW(
                    window: *mut std::ffi::c_void,
                    message: u32,
                    wparam: usize,
                    lparam: isize,
                ) -> isize;
            }
            let handle = window.hwnd().map_err(|error| error.to_string())?;
            let mut point = Point { x: 0, y: 0 };
            unsafe {
                if GetCursorPos(&mut point) == 0 {
                    return Err("Could not locate the pointer".into());
                }
                ReleaseCapture();
                // WM_NCLBUTTONDOWN takes packed screen coordinates. The native
                // move loop returns on release, keeping hover expansion stable.
                let coordinates =
                    ((point.x as u32 & 0xffff) | ((point.y as u32 & 0xffff) << 16)) as i32 as isize;
                SendMessageW(handle.0, 0x00A1, 2, coordinates);
            }
        }
        #[cfg(not(target_os = "windows"))]
        window.start_dragging().map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn hide_codex_widget(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("codex-widget") {
        window.hide().map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn reset_widget_position(app: AppHandle) -> Result<(), String> {
    app.state::<crate::platform::WidgetPlacement>()
        .manually_moved
        .store(false, std::sync::atomic::Ordering::Relaxed);
    #[cfg(target_os = "windows")]
    crate::platform::position_codex_widget(&app).map_err(|error| error.to_string())?;
    Ok(())
}

pub fn show_codex_widget(app: &AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("codex-widget") {
        let _ = window.set_always_on_top(true);
        window.show().map_err(|error| error.to_string())?;
        let _ = window.unminimize();
    }
    #[cfg(target_os = "windows")]
    let _ = crate::platform::position_codex_widget(app);
    Ok(())
}

#[tauri::command]
pub fn show_dashboard(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("main") {
        #[cfg(target_os = "macos")]
        crate::activate_app();
        if crate::demo::enabled() {
            crate::demo::place_popover(&window);
            let _ = window.set_always_on_top(true);
        }
        window.show().map_err(|error| error.to_string())?;
        window.unminimize().map_err(|error| error.to_string())?;
        window.set_focus().map_err(|error| error.to_string())?;
        crate::claude::refresh_on_open(&app);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::is_openable_post_url;

    #[test]
    fn only_reset_post_links_are_openable() {
        assert!(is_openable_post_url(
            "https://bsky.app/profile/a.social/post/3mw"
        ));
        assert!(is_openable_post_url("https://x.com/thsottiaux/status/1"));
        assert!(!is_openable_post_url("file:///etc/passwd"));
        assert!(!is_openable_post_url("https://evil.example/x.com/"));
        assert!(!is_openable_post_url("https://x.com/a b"));
    }
}
