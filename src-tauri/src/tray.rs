use serde_json::Value;
use tauri::{
    image::Image,
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    App, AppHandle, Emitter, Manager,
};
use tauri_plugin_autostart::ManagerExt;

use crate::claude::ClaudeManager;
use crate::codex::process::CodexManager;
use crate::prefs::PrefsStore;

pub const CODEX_TRAY_ID: &str = "provider-codex";
pub const CLAUDE_TRAY_ID: &str = "provider-claude";

/// Unix timestamp (seconds) until which an announced-but-not-yet-landed reset
/// is pending. While pending, the Codex tray title carries a ⚡ prefix so the
/// burn window is visible at menu-bar level without opening the popover.
#[derive(Default)]
pub struct ResetRadar(pub std::sync::atomic::AtomicU64);

impl ResetRadar {
    pub fn incoming_at(&self, now_unix: u64) -> bool {
        let until = self.0.load(std::sync::atomic::Ordering::Relaxed);
        until > now_unix
    }
}

pub fn with_incoming_prefix(title: String, incoming: bool) -> String {
    if !incoming {
        return title;
    }
    if title.is_empty() {
        "⚡".to_owned()
    } else {
        format!("⚡ {title}")
    }
}

fn toggle_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        if window.is_visible().unwrap_or(false) {
            let _ = window.hide();
        } else {
            let _ = window.show();
            let _ = window.set_focus();
        }
    }
}

fn quit_from_menu(app: &AppHandle) {
    let handle = app.clone();
    let manager = app.state::<CodexManager>().inner().clone();
    tauri::async_runtime::spawn(async move {
        manager.shutdown().await;
        handle.exit(0);
    });
}

fn refresh_all_tray_titles(app: &AppHandle) {
    let codex = app.state::<CodexManager>().inner().clone();
    let claude = app.state::<ClaudeManager>().inner().clone();
    tauri::async_runtime::spawn(async move {
        codex.update_tray().await;
        claude.update_tray().await;
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Codex,
    Claude,
}

impl Provider {
    pub fn tray_id(self) -> &'static str {
        match self {
            Provider::Codex => CODEX_TRAY_ID,
            Provider::Claude => CLAUDE_TRAY_ID,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Provider::Codex => "codex",
            Provider::Claude => "claude",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Provider::Codex => "Codex",
            Provider::Claude => "Claude Code",
        }
    }

    pub fn window_preference(self, prefs: &crate::prefs::AppPrefs) -> String {
        match self {
            Provider::Codex => prefs.codex_tray_window.clone(),
            Provider::Claude => prefs.claude_tray_window.clone(),
        }
    }
}

/// Remembers the menu each tray currently displays so the once-per-second
/// title tick does not rebuild native menus continuously.
#[derive(Default)]
pub struct TrayMenuState(std::sync::Mutex<std::collections::HashMap<&'static str, String>>);

impl TrayMenuState {
    fn changed(&self, key: &'static str, signature: &str) -> bool {
        let mut map = self.0.lock().expect("tray menu state poisoned");
        if map.get(key).map(String::as_str) == Some(signature) {
            return false;
        }
        map.insert(key, signature.to_owned());
        true
    }

    fn invalidate(&self) {
        self.0.lock().expect("tray menu state poisoned").clear();
    }
}

/// Re-syncs both trays after a preference changes anywhere (tray menu or the
/// in-app settings panel), so titles and menu checkmarks stay in agreement.
pub fn apply_preference_change(app: &AppHandle) {
    app.state::<TrayMenuState>().invalidate();
    refresh_all_tray_titles(app);
}

fn window_menu_id(provider: Provider, window_id: &str) -> String {
    format!("win|{}|{}", provider.key(), window_id)
}

fn build_menu(
    app: &AppHandle,
    provider: Provider,
    windows: &[TrayWindow],
    selected: &str,
) -> tauri::Result<Menu<tauri::Wry>> {
    use tauri::menu::{IsMenuItem, Submenu};

    let prefs = app.state::<PrefsStore>().get();
    let refresh = MenuItem::with_id(
        app,
        format!("{}-refresh", provider.key()),
        "Refresh",
        true,
        None::<&str>,
    )?;

    // Menu-bar window picker: "Most used" plus one entry per reported window.
    let mut picker: Vec<Box<dyn IsMenuItem<tauri::Wry>>> = vec![Box::new(CheckMenuItem::with_id(
        app,
        window_menu_id(provider, crate::prefs::TRAY_WINDOW_AUTO),
        "Most used",
        true,
        selected == crate::prefs::TRAY_WINDOW_AUTO,
        None::<&str>,
    )?)];
    for window in windows {
        picker.push(Box::new(CheckMenuItem::with_id(
            app,
            window_menu_id(provider, &window.id),
            &window.label,
            true,
            selected == window.id,
            None::<&str>,
        )?));
    }
    let picker_refs: Vec<&dyn IsMenuItem<tauri::Wry>> =
        picker.iter().map(|item| item.as_ref()).collect();
    let picker_menu = Submenu::with_items(app, "Menu Bar Shows", true, &picker_refs)?;

    let quit = MenuItem::with_id(
        app,
        format!("{}-quit", provider.key()),
        "Quit UsageBar",
        true,
        None::<&str>,
    )?;
    let separator = PredefinedMenuItem::separator(app)?;

    // Preferences that apply app-wide live on the Codex (primary) tray only.
    if provider == Provider::Claude {
        return Menu::with_items(app, &[&refresh, &separator, &picker_menu, &separator, &quit]);
    }

    let compact = CheckMenuItem::with_id(
        app,
        "toggle-compact",
        "Compact Meter",
        true,
        prefs.compact_tray,
        None::<&str>,
    )?;
    let alerts = CheckMenuItem::with_id(
        app,
        "toggle-alerts",
        "Usage Alerts",
        true,
        prefs.usage_alerts,
        None::<&str>,
    )?;
    let autostart = CheckMenuItem::with_id(
        app,
        "toggle-autostart",
        "Launch at Login",
        true,
        app.autolaunch().is_enabled().unwrap_or(false),
        None::<&str>,
    )?;
    let walkthrough = MenuItem::with_id(app, "show-onboarding", "Setup Guide…", true, None::<&str>)?;
    Menu::with_items(
        app,
        &[
            &refresh,
            &separator,
            &picker_menu,
            &separator,
            &compact,
            &alerts,
            &autostart,
            &separator,
            &walkthrough,
            &quit,
        ],
    )
}

/// Rebuilds a tray's menu when its window list or preferences change. Native
/// menu objects must be created on the macOS main thread.
pub fn sync_tray_menu(app: &AppHandle, provider: Provider, windows: &[TrayWindow]) {
    let Some(menu_state) = app.try_state::<TrayMenuState>() else {
        return;
    };
    let prefs = app.state::<PrefsStore>().get();
    let selected = provider.window_preference(&prefs);
    let signature = format!(
        "{selected}|{}|{}|{}",
        prefs.compact_tray,
        prefs.usage_alerts,
        windows
            .iter()
            .map(|window| format!("{}={}", window.id, window.label))
            .collect::<Vec<_>>()
            .join(",")
    );
    if !menu_state.changed(provider.key(), &signature) {
        return;
    }
    let handle = app.clone();
    let windows = windows.to_vec();
    let _ = app.run_on_main_thread(move || {
        let Some(tray) = handle.tray_by_id(provider.tray_id()) else {
            return;
        };
        match build_menu(&handle, provider, &windows, &selected) {
            Ok(menu) => {
                let _ = tray.set_menu(Some(menu));
            }
            Err(_error) => {
                #[cfg(debug_assertions)]
                eprintln!("usagebar: tray menu rebuild failed: {_error}");
            }
        }
    });
}

fn handle_menu_event(app: &AppHandle, id: &str) {
    if let Some(rest) = id.strip_prefix("win|") {
        let mut parts = rest.splitn(2, '|');
        let (Some(key), Some(window_id)) = (parts.next(), parts.next()) else {
            return;
        };
        let provider = match key {
            "codex" => Provider::Codex,
            "claude" => Provider::Claude,
            _ => return,
        };
        app.state::<PrefsStore>().update(|prefs| match provider {
            Provider::Codex => prefs.codex_tray_window = window_id.to_owned(),
            Provider::Claude => prefs.claude_tray_window = window_id.to_owned(),
        });
        app.state::<TrayMenuState>().invalidate();
        refresh_all_tray_titles(app);
        return;
    }

    match id {
        "codex-refresh" => {
            let manager = app.state::<CodexManager>().inner().clone();
            tauri::async_runtime::spawn(async move {
                let _ = manager.refresh_or_start().await;
            });
        }
        "claude-refresh" => {
            let manager = app.state::<ClaudeManager>().inner().clone();
            tauri::async_runtime::spawn(async move {
                let _ = manager.refresh().await;
            });
        }
        "toggle-compact" => {
            app.state::<PrefsStore>()
                .update(|prefs| prefs.compact_tray = !prefs.compact_tray);
            app.state::<TrayMenuState>().invalidate();
            refresh_all_tray_titles(app);
        }
        "toggle-alerts" => {
            app.state::<PrefsStore>()
                .update(|prefs| prefs.usage_alerts = !prefs.usage_alerts);
            app.state::<TrayMenuState>().invalidate();
        }
        "toggle-autostart" => {
            let autolaunch = app.autolaunch();
            let result = if autolaunch.is_enabled().unwrap_or(false) {
                autolaunch.disable()
            } else {
                autolaunch.enable()
            };
            if let Err(error) = result {
                eprintln!("usagebar: launch-at-login toggle failed: {error}");
            }
            app.state::<TrayMenuState>().invalidate();
        }
        "show-onboarding" => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
            let _ = app.emit("usagebar://show-onboarding", ());
        }
        "codex-quit" | "claude-quit" => quit_from_menu(app),
        _ => {}
    }
}

pub fn setup(app: &App) -> tauri::Result<()> {
    let handle = app.handle().clone();
    let menu = build_menu(
        &handle,
        Provider::Codex,
        &[],
        &app.state::<PrefsStore>().get().codex_tray_window,
    )?;
    TrayIconBuilder::with_id(CODEX_TRAY_ID)
        .tooltip("UsageBar")
        .icon(codex_tray_icon())
        .icon_as_template(false)
        .menu(&menu)
        .show_menu_on_left_click(false)
        // Menu-event handlers are GLOBAL in Tauri: this one handler receives
        // events from every tray menu, including Claude's. Registering a
        // second handler there would run each click twice and turn the
        // toggles into no-ops, so this is deliberately the only one.
        .on_menu_event(|app, event| handle_menu_event(app, event.id.as_ref()))
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle_main_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// Builds the Claude tray item on first successful credential detection so
/// Macs without Claude Code never grow an empty second icon.
pub fn ensure_claude_tray(app: &AppHandle) -> tauri::Result<()> {
    if let Some(tray) = app.tray_by_id(CLAUDE_TRAY_ID) {
        let _ = tray.set_visible(true);
        return Ok(());
    }
    let menu = build_menu(
        app,
        Provider::Claude,
        &[],
        &app.state::<PrefsStore>().get().claude_tray_window,
    )?;
    TrayIconBuilder::with_id(CLAUDE_TRAY_ID)
        .tooltip("Claude Code usage")
        .icon(claude_tray_icon())
        .icon_as_template(false)
        .menu(&menu)
        .show_menu_on_left_click(false)
        // No on_menu_event here on purpose — see the note on the Codex tray.
        // Tray icon (click) handlers, unlike menu handlers, are per-tray.
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle_main_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// A compact Codex-purple version of the cloud/terminal mark. Keeping one tray
/// item per provider lets Claude/Gemini add their own neighboring, distinctly
/// colored menu-bar meters later without combining brand identities.
pub fn codex_tray_icon() -> Image<'static> {
    const WIDTH: u32 = 22;
    const HEIGHT: u32 = 18;
    const SAMPLES: u32 = 4;
    let mut rgba = vec![0_u8; (WIDTH * HEIGHT * 4) as usize];

    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let mut cloud_coverage = 0_u32;
            let mut terminal_coverage = 0_u32;
            for sample_y in 0..SAMPLES {
                for sample_x in 0..SAMPLES {
                    let px = x as f64 + (sample_x as f64 + 0.5) / SAMPLES as f64;
                    let py = y as f64 + (sample_y as f64 + 0.5) / SAMPLES as f64;
                    if inside_codex_cloud(px, py) {
                        cloud_coverage += 1;
                        if inside_terminal_glyph(px, py) {
                            terminal_coverage += 1;
                        }
                    }
                }
            }
            if cloud_coverage == 0 {
                continue;
            }
            let alpha = ((cloud_coverage * 255) / (SAMPLES * SAMPLES)) as u8;
            let blend = y as f64 / (HEIGHT - 1) as f64;
            let glyph_mix = terminal_coverage as f64 / cloud_coverage as f64;
            let red = ((194.0 + (139.0 - 194.0) * blend) * (1.0 - glyph_mix) + 255.0 * glyph_mix)
                .round() as u8;
            let green = ((79.0 + (55.0 - 79.0) * blend) * (1.0 - glyph_mix) + 255.0 * glyph_mix)
                .round() as u8;
            let blue = ((255.0 + (235.0 - 255.0) * blend) * (1.0 - glyph_mix) + 255.0 * glyph_mix)
                .round() as u8;
            let index = ((y * WIDTH + x) * 4) as usize;
            rgba[index..index + 4].copy_from_slice(&[red, green, blue, alpha]);
        }
    }
    Image::new_owned(rgba, WIDTH, HEIGHT)
}

fn inside_codex_cloud(x: f64, y: f64) -> bool {
    [
        (6.2, 9.2, 4.0),
        (9.1, 6.0, 4.4),
        (13.6, 6.7, 4.1),
        (16.0, 9.7, 4.0),
        (13.2, 12.1, 4.3),
        (8.3, 12.0, 4.1),
    ]
    .into_iter()
    .any(|(cx, cy, radius)| (x - cx).powi(2) + (y - cy).powi(2) <= radius * radius)
}

fn inside_terminal_glyph(x: f64, y: f64) -> bool {
    let chevron = distance_to_segment(x, y, 6.8, 7.0, 8.6, 9.2) <= 0.72
        || distance_to_segment(x, y, 8.6, 9.2, 6.8, 11.5) <= 0.72;
    let underscore = distance_to_segment(x, y, 11.1, 11.2, 14.4, 11.2) <= 0.72;
    chevron || underscore
}

/// Claude's coral starburst, drawn with the same supersampled rasterizer as the
/// Codex cloud so the two provider icons sit together at matching weights.
pub fn claude_tray_icon() -> Image<'static> {
    const WIDTH: u32 = 22;
    const HEIGHT: u32 = 18;
    const SAMPLES: u32 = 4;
    const CENTER_X: f64 = 11.0;
    const CENTER_Y: f64 = 9.0;
    let mut rgba = vec![0_u8; (WIDTH * HEIGHT * 4) as usize];

    let rays: Vec<(f64, f64)> = (0..8)
        .map(|index| {
            let angle = std::f64::consts::FRAC_PI_4 * index as f64;
            // Cardinal rays reach a little farther than diagonals, echoing the
            // uneven spark of the Claude mark.
            let length = if index % 2 == 0 { 7.0 } else { 5.4 };
            (
                CENTER_X + angle.cos() * length,
                CENTER_Y + angle.sin() * length,
            )
        })
        .collect();

    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let mut coverage = 0_u32;
            for sample_y in 0..SAMPLES {
                for sample_x in 0..SAMPLES {
                    let px = x as f64 + (sample_x as f64 + 0.5) / SAMPLES as f64;
                    let py = y as f64 + (sample_y as f64 + 0.5) / SAMPLES as f64;
                    let inside = rays.iter().any(|(tip_x, tip_y)| {
                        distance_to_segment(px, py, CENTER_X, CENTER_Y, *tip_x, *tip_y) <= 0.78
                    });
                    if inside {
                        coverage += 1;
                    }
                }
            }
            if coverage == 0 {
                continue;
            }
            let alpha = ((coverage * 255) / (SAMPLES * SAMPLES)) as u8;
            let blend = y as f64 / (HEIGHT - 1) as f64;
            let red = (217.0 + (191.0 - 217.0) * blend).round() as u8;
            let green = (119.0 + (94.0 - 119.0) * blend).round() as u8;
            let blue = (87.0 + (62.0 - 87.0) * blend).round() as u8;
            let index = ((y * WIDTH + x) * 4) as usize;
            rgba[index..index + 4].copy_from_slice(&[red, green, blue, alpha]);
        }
    }
    Image::new_owned(rgba, WIDTH, HEIGHT)
}

fn distance_to_segment(x: f64, y: f64, x0: f64, y0: f64, x1: f64, y1: f64) -> f64 {
    let dx = x1 - x0;
    let dy = y1 - y0;
    let length_squared = dx * dx + dy * dy;
    let t = (((x - x0) * dx + (y - y0) * dy) / length_squared).clamp(0.0, 1.0);
    ((x - (x0 + t * dx)).powi(2) + (y - (y0 + t * dy)).powi(2)).sqrt()
}

/// One quota window a menu-bar meter can follow.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrayWindow {
    /// Stable identifier used by the menu-bar picker preference.
    pub id: String,
    /// Human label for the menu and tooltip ("5-hour", "Weekly", "Fable").
    pub label: String,
    pub used_percent: f64,
    pub resets_at: Option<u64>,
    pub duration_mins: Option<f64>,
}

pub fn window_duration_label(minutes: Option<f64>) -> String {
    match minutes {
        Some(m) if m == 300.0 => "5-hour".to_owned(),
        Some(m) if m == 1_440.0 => "Daily".to_owned(),
        Some(m) if m == 10_080.0 => "Weekly".to_owned(),
        Some(m) if m > 0.0 && m % 10_080.0 == 0.0 => format!("{}-week", (m / 10_080.0) as u64),
        Some(m) if m > 0.0 && m % 1_440.0 == 0.0 => format!("{}-day", (m / 1_440.0) as u64),
        Some(m) if m > 0.0 && m % 60.0 == 0.0 => format!("{}-hour", (m / 60.0) as u64),
        _ => "Limit".to_owned(),
    }
}

fn collect_snapshot(limit_id: &str, snapshot: &Value, out: &mut Vec<TrayWindow>) {
    for kind in ["primary", "secondary"] {
        let Some(window) = snapshot.get(kind) else {
            continue;
        };
        let Some(used) = window.get("usedPercent").and_then(Value::as_f64) else {
            continue;
        };
        let duration = window.get("windowDurationMins").and_then(Value::as_f64);
        let label = window
            .get("windowLabel")
            .and_then(Value::as_str)
            .or_else(|| snapshot.get("windowLabel").and_then(Value::as_str))
            .map(str::to_owned)
            .unwrap_or_else(|| window_duration_label(duration));
        out.push(TrayWindow {
            id: format!("{limit_id}:{kind}"),
            label,
            used_percent: used.clamp(0.0, 100.0),
            resets_at: window
                .get("resetsAt")
                .and_then(Value::as_f64)
                .map(|value| value.max(0.0) as u64),
            duration_mins: duration,
        });
    }
}

/// Lists every window a provider reports, shortest window first so the picker
/// reads 5-hour → weekly regardless of map ordering.
pub fn collect_windows(payload: Option<&Value>) -> Vec<TrayWindow> {
    let Some(payload) = payload else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(by_id) = payload.get("rateLimitsByLimitId").and_then(Value::as_object) {
        for (limit_id, snapshot) in by_id {
            collect_snapshot(limit_id, snapshot, &mut out);
        }
    }
    if out.is_empty() {
        if let Some(snapshot) = payload.get("rateLimits") {
            collect_snapshot("codex", snapshot, &mut out);
        }
    }
    out.sort_by(|left, right| {
        let a = left.duration_mins.unwrap_or(f64::MAX);
        let b = right.duration_mins.unwrap_or(f64::MAX);
        a.partial_cmp(&b)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.id.cmp(&right.id))
    });
    out
}

/// Resolves the preference to a window: an explicit choice when it still
/// exists, otherwise the most-used window.
pub fn select_window<'a>(windows: &'a [TrayWindow], preference: &str) -> Option<&'a TrayWindow> {
    if preference != crate::prefs::TRAY_WINDOW_AUTO {
        if let Some(chosen) = windows.iter().find(|window| window.id == preference) {
            return Some(chosen);
        }
    }
    windows.iter().max_by(|left, right| {
        left.used_percent
            .partial_cmp(&right.used_percent)
            .unwrap_or(std::cmp::Ordering::Equal)
    })
}

/// Titles show percent USED, mirroring how Codex and Claude Code both report
/// usage, so the menu bar never disagrees with the tools themselves.
pub fn tray_title(
    used_percent: Option<f64>,
    resets_at: Option<u64>,
    now_unix: u64,
    compact: bool,
) -> String {
    let Some(used) = used_percent else {
        return String::new();
    };
    let percent = used.clamp(0.0, 100.0).round() as u32;
    match resets_at {
        Some(target) if !compact && target > now_unix => {
            format!("{percent}% · {}", format_countdown(target - now_unix))
        }
        _ => format!("{percent}%"),
    }
}

pub fn format_countdown(total_seconds: u64) -> String {
    let days = total_seconds / 86_400;
    if days > 0 {
        return format!("{days}d {}h", (total_seconds % 86_400) / 3_600);
    }
    let hours = total_seconds / 3_600;
    let minutes = (total_seconds % 3_600) / 60;
    let seconds = total_seconds % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_payload() -> Value {
        serde_json::json!({
            "rateLimitsByLimitId": {
                "session": { "primary": { "usedPercent": 8, "windowDurationMins": 300, "resetsAt": 1_000 } },
                "weekly-all": { "secondary": { "usedPercent": 57, "windowDurationMins": 10_080, "resetsAt": 2_000 } },
                "weekly-scoped-fable": { "secondary": { "usedPercent": 92, "windowDurationMins": 10_080, "resetsAt": 3_000, "excludeFromTray": true, "windowLabel": "Fable" } }
            }
        })
    }

    #[test]
    fn collects_windows_shortest_first_with_labels() {
        let windows = collect_windows(Some(&sample_payload()));
        let labeled: Vec<(&str, &str)> = windows
            .iter()
            .map(|window| (window.id.as_str(), window.label.as_str()))
            .collect();
        assert_eq!(
            labeled,
            vec![
                ("session:primary", "5-hour"),
                ("weekly-all:secondary", "Weekly"),
                ("weekly-scoped-fable:secondary", "Fable"),
            ]
        );
        assert!(collect_windows(None).is_empty());
    }

    #[test]
    fn falls_back_to_the_flat_codex_shape() {
        let payload = serde_json::json!({
            "rateLimits": {
                "primary": { "usedPercent": 20, "windowDurationMins": 300 },
                "secondary": { "usedPercent": 36, "windowDurationMins": 10_080 }
            }
        });
        let windows = collect_windows(Some(&payload));
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].id, "codex:primary");
        assert_eq!(windows[1].label, "Weekly");
    }

    #[test]
    fn selection_honors_the_preference_and_falls_back_to_most_used() {
        let windows = collect_windows(Some(&sample_payload()));
        // Auto tracks the most-used window, scoped limits included.
        let auto = select_window(&windows, crate::prefs::TRAY_WINDOW_AUTO).expect("auto window");
        assert_eq!(auto.label, "Fable");
        // An explicit pick wins even when another window is more used.
        let pinned = select_window(&windows, "session:primary").expect("pinned window");
        assert_eq!(pinned.label, "5-hour");
        assert_eq!(pinned.used_percent, 8.0);
        // A window that disappeared (plan change) degrades to most-used.
        let stale = select_window(&windows, "weekly-scoped-gone:secondary").expect("fallback");
        assert_eq!(stale.label, "Fable");
        assert!(select_window(&[], "session:primary").is_none());
    }

    #[test]
    fn title_shows_used_percent_and_countdown() {
        assert_eq!(tray_title(Some(42.0), Some(4_661), 1_000, false), "42% · 1:01:01");
        assert_eq!(tray_title(Some(42.0), Some(1_545), 1_000, false), "42% · 9:05");
        assert_eq!(tray_title(Some(42.0), Some(200_000), 1_000, false), "42% · 2d 7h");
        assert_eq!(tray_title(Some(42.0), None, 1_000, false), "42%");
        assert_eq!(tray_title(Some(42.0), Some(900), 1_000, false), "42%");
        assert_eq!(tray_title(None, Some(4_661), 1_000, false), "");
    }

    #[test]
    fn incoming_reset_prefixes_the_title() {
        assert_eq!(with_incoming_prefix("42% · 9:05".into(), true), "⚡ 42% · 9:05");
        assert_eq!(with_incoming_prefix("42%".into(), false), "42%");
        assert_eq!(with_incoming_prefix(String::new(), true), "⚡");
        let radar = ResetRadar::default();
        assert!(!radar.incoming_at(1_000));
        radar.0.store(2_000, std::sync::atomic::Ordering::Relaxed);
        assert!(radar.incoming_at(1_000));
        assert!(!radar.incoming_at(2_000));
    }

    #[test]
    fn compact_title_drops_the_countdown() {
        assert_eq!(tray_title(Some(42.0), Some(4_661), 1_000, true), "42%");
        assert_eq!(tray_title(None, Some(4_661), 1_000, true), "");
    }

    #[test]
    fn claude_icon_is_a_coral_starburst() {
        let icon = claude_tray_icon();
        let rgba = icon.rgba();
        let pixel_at = |x: usize, y: usize| &rgba[(y * 22 + x) * 4..(y * 22 + x) * 4 + 4];
        let center = pixel_at(11, 9);
        assert_eq!(center[3], 255);
        assert!(center[0] > center[1] && center[1] > center[2]);
        assert_eq!(pixel_at(0, 0)[3], 0);
        assert!(pixel_at(16, 9)[3] > 0);
    }

    #[test]
    fn codex_icon_has_an_orchid_cloud_and_white_terminal_glyph() {
        let icon = codex_tray_icon();
        let rgba = icon.rgba();
        let pixel_at = |x: usize, y: usize| &rgba[(y * 22 + x) * 4..(y * 22 + x) * 4 + 4];
        assert_eq!(pixel_at(11, 7)[3], 255);
        assert!(pixel_at(8, 9)[0] > 220);
        assert!(pixel_at(12, 11)[1] > 220);
        assert_eq!(pixel_at(0, 0)[3], 0);
    }
}
