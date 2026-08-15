use serde_json::Value;
use tauri::{
    image::Image,
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    App, AppHandle, Manager,
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

pub fn setup(app: &App) -> tauri::Result<()> {
    let prefs = app.state::<PrefsStore>().get();
    let autostart_enabled = app.autolaunch().is_enabled().unwrap_or(false);

    let refresh = MenuItem::with_id(app, "refresh", "Refresh", true, None::<&str>)?;
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
        autostart_enabled,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit UsageBar", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &refresh,
            &PredefinedMenuItem::separator(app)?,
            &compact,
            &alerts,
            &autostart,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;
    TrayIconBuilder::with_id(CODEX_TRAY_ID)
        .tooltip("UsageBar")
        .icon(codex_tray_icon())
        .icon_as_template(false)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "refresh" => {
                let manager = app.state::<CodexManager>().inner().clone();
                tauri::async_runtime::spawn(async move {
                    let _ = manager.refresh_or_start().await;
                });
            }
            "toggle-compact" => {
                app.state::<PrefsStore>()
                    .update(|prefs| prefs.compact_tray = !prefs.compact_tray);
                refresh_all_tray_titles(app);
            }
            "toggle-alerts" => {
                app.state::<PrefsStore>()
                    .update(|prefs| prefs.usage_alerts = !prefs.usage_alerts);
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
            }
            "quit" => quit_from_menu(app),
            _ => {}
        })
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
    let refresh = MenuItem::with_id(app, "claude-refresh", "Refresh", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "claude-quit", "Quit UsageBar", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&refresh, &quit])?;
    TrayIconBuilder::with_id(CLAUDE_TRAY_ID)
        .tooltip("Claude Code usage")
        .icon(claude_tray_icon())
        .icon_as_template(false)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "claude-refresh" => {
                let manager = app.state::<ClaudeManager>().inner().clone();
                tauri::async_runtime::spawn(async move {
                    let _ = manager.refresh().await;
                });
            }
            "claude-quit" => quit_from_menu(app),
            _ => {}
        })
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CookedWindow {
    pub used_percent: f64,
    pub resets_at: Option<u64>,
}

pub fn most_cooked_window(payload: Option<&Value>) -> Option<CookedWindow> {
    fn visit(value: &Value, best: &mut Option<CookedWindow>) {
        match value {
            Value::Object(map) => {
                // Windows can opt out of the tray (e.g. Claude's model-scoped
                // weekly limits) while still appearing in the popover.
                let excluded = map
                    .get("excludeFromTray")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if !excluded {
                    if let Some(used) = map.get("usedPercent").and_then(Value::as_f64) {
                        let used_percent = used.clamp(0.0, 100.0);
                        let resets_at = map
                            .get("resetsAt")
                            .and_then(Value::as_f64)
                            .map(|value| value.max(0.0) as u64);
                        let replace = match best {
                            Some(current) => used_percent > current.used_percent,
                            None => true,
                        };
                        if replace {
                            *best = Some(CookedWindow {
                                used_percent,
                                resets_at,
                            });
                        }
                    }
                }
                for child in map.values() {
                    visit(child, best);
                }
            }
            Value::Array(array) => {
                for child in array {
                    visit(child, best);
                }
            }
            _ => {}
        }
    }
    let mut best = None;
    visit(payload?, &mut best);
    best
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

    #[test]
    fn finds_most_used_window_and_skips_tray_excluded_ones() {
        let payload = serde_json::json!({
            "rateLimits": { "primary": { "usedPercent": 20, "resetsAt": 1_000 }, "secondary": null },
            "rateLimitsByLimitId": {
                "other": { "primary": { "usedPercent": 82, "resetsAt": 2_000 } },
                "scoped": { "secondary": { "usedPercent": 95, "resetsAt": 3_000, "excludeFromTray": true } }
            }
        });
        let cooked = most_cooked_window(Some(&payload));
        assert_eq!(
            cooked,
            Some(CookedWindow {
                used_percent: 82.0,
                resets_at: Some(2_000)
            })
        );
        assert_eq!(most_cooked_window(None), None);
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
