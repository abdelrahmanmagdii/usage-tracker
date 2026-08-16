use std::path::PathBuf;

use tauri::{AppHandle, Manager, State};

use crate::claude::{ClaudeManager, ClaudeState};
use crate::codex::process::{CodexManager, CodexState};
use crate::notch::{self, NotchController, NotchMode, NotchStatus};

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
    manager.refresh().await
}

#[tauri::command]
pub fn get_app_prefs(prefs: State<'_, crate::prefs::PrefsStore>) -> crate::prefs::AppPrefs {
    prefs.get()
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
pub fn get_notch_status(controller: State<'_, NotchController>) -> NotchStatus {
    controller.status()
}

#[tauri::command]
pub fn set_notch_mode(
    app: AppHandle,
    controller: State<'_, NotchController>,
    mode: NotchMode,
) -> Result<NotchStatus, String> {
    controller.set_mode(mode)?;
    notch::schedule_sync(app)?;
    Ok(controller.status())
}

#[tauri::command]
pub fn show_main_window(app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "The UsageBar window is unavailable".to_owned())?;
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())
}

#[tauri::command]
pub fn set_notch_expanded(app: AppHandle, expanded: bool) -> Result<(), String> {
    let window = app
        .get_webview_window(notch::NOTCH_WINDOW_LABEL)
        .ok_or_else(|| "The notch companion is unavailable".to_owned())?;
    let height = if expanded {
        notch::NOTCH_EXPANDED_HEIGHT
    } else {
        notch::NOTCH_COLLAPSED_HEIGHT
    };
    window
        .set_size(tauri::LogicalSize::new(notch::NOTCH_WIDTH, height))
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn write_share_card(path: String, bytes: Vec<u8>) -> Result<(), String> {
    let destination = PathBuf::from(path);
    if destination.extension().and_then(|value| value.to_str()) != Some("png") {
        return Err("Share cards must be saved as PNG files".into());
    }
    tokio::fs::write(destination, bytes)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn quit_app(app: AppHandle) {
    let manager = app.state::<CodexManager>().inner().clone();
    manager.shutdown().await;
    app.exit(0);
}
