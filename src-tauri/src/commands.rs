use std::path::PathBuf;

use tauri::{AppHandle, Manager, State};

use crate::claude::{ClaudeManager, ClaudeState};
use crate::codex::process::{CodexManager, CodexState};

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

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrayWindowOptions {
    pub codex: Vec<crate::tray::TrayWindow>,
    pub claude: Vec<crate::tray::TrayWindow>,
}

/// Windows each provider currently reports, for the in-app menu-bar picker.
#[tauri::command]
pub async fn get_tray_windows(
    codex: State<'_, CodexManager>,
    claude: State<'_, ClaudeManager>,
) -> Result<TrayWindowOptions, String> {
    let codex_state = codex.snapshot().await;
    let claude_state = claude.snapshot().await;
    Ok(TrayWindowOptions {
        codex: crate::tray::collect_windows(codex_state.rate_limits.as_ref()),
        claude: crate::tray::collect_windows(claude_state.rate_limits.as_ref()),
    })
}

#[tauri::command]
pub fn set_tray_window(app: AppHandle, provider: String, window: String) -> Result<(), String> {
    match provider.as_str() {
        "codex" => app
            .state::<crate::prefs::PrefsStore>()
            .update(|prefs| prefs.codex_tray_window = window),
        "claude" => app
            .state::<crate::prefs::PrefsStore>()
            .update(|prefs| prefs.claude_tray_window = window),
        other => return Err(format!("Unknown provider: {other}")),
    };
    crate::tray::apply_preference_change(&app);
    Ok(())
}

#[tauri::command]
pub fn set_usage_alerts(app: AppHandle, enabled: bool) {
    app.state::<crate::prefs::PrefsStore>()
        .update(|prefs| prefs.usage_alerts = enabled);
    crate::tray::apply_preference_change(&app);
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
