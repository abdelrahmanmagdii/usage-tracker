use serde_json::Value;
use tauri::{
    image::Image,
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    App, AppHandle, Emitter, Manager,
};
use tauri_plugin_autostart::ManagerExt;

use crate::claude::ClaudeManager;
use crate::codex::process::{CodexManager, ConnectionState};
use crate::prefs::PrefsStore;

/// The combined menu-bar item carrying both providers. One narrow item resists
/// macOS hiding it when the bar is crowded (or collides with a notch), which is
/// why it is the default; the per-provider items below are the opt-in layout.
pub const TRAY_ID: &str = "usagebar";
/// Per-provider items, used only in the "two separate icons" layout.
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

/// How old a meter's numbers may get before the menu bar admits they are old.
/// Refreshes are attempted every few minutes, so this is several missed cycles
/// rather than a single hiccup.
pub const TRAY_STALE_AFTER_SECS: u64 = 15 * 60;

/// Age of a meter's numbers once they count as stale, or `None` while they are
/// still current (or while the meter has never had any).
pub fn stale_age(updated_at: Option<u64>, now_unix: u64) -> Option<u64> {
    updated_at
        .map(|at| now_unix.saturating_sub(at))
        .filter(|age| *age >= TRAY_STALE_AFTER_SECS)
}

/// Marks a title whose numbers stopped refreshing. Without it the menu bar goes
/// on presenting a frozen percentage as if it were current — the failure mode
/// where an expired session quietly pinned the meter at an old number for
/// hours. The `~` reads as "about", and the tooltip spells out the age.
pub fn with_stale_marker(title: String, stale: bool) -> String {
    if !stale || title.is_empty() {
        return title;
    }
    format!("~{title}")
}

/// Coarse "how long ago" for tooltips.
pub fn format_age(seconds: u64) -> String {
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h {}m", minutes % 60);
    }
    format!("{}d {}h", hours / 24, hours % 24)
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
            #[cfg(target_os = "macos")]
            crate::activate_app();
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

/// Recomputes and repaints the single tray item from the current app state.
fn refresh_tray(app: &AppHandle) {
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        refresh_unified_tray(&handle).await;
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Codex,
    Claude,
}

impl Provider {
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

/// Caches the last state applied to each tray so the once-per-second tick only
/// reaches into AppKit when a title, tooltip, or visibility actually changed —
/// and always does so on the main thread, which is the only thread an
/// `NSStatusItem` may be mutated from. Skipping the visibility calls in steady
/// state is what stops the menu-bar items from flickering or dropping out.
#[derive(Default)]
pub struct TrayRenderCache {
    visible: std::sync::Mutex<std::collections::HashMap<&'static str, bool>>,
    labels: std::sync::Mutex<std::collections::HashMap<&'static str, (String, String)>>,
}

/// Re-syncs the tray after a preference changes anywhere (tray menu or the
/// in-app settings panel), so the title and menu checkmarks stay in agreement.
pub fn apply_preference_change(app: &AppHandle) {
    app.state::<TrayMenuState>().invalidate();
    refresh_tray(app);
}

fn window_menu_id(provider: Provider, window_id: &str) -> String {
    format!("win|{}|{}", provider.key(), window_id)
}

/// The Compact/Extended layout picker, shown on the shared (primary) menu.
fn layout_submenu(app: &AppHandle, combined: bool) -> tauri::Result<tauri::menu::Submenu<tauri::Wry>> {
    use tauri::menu::{IsMenuItem, Submenu};
    let compact = CheckMenuItem::with_id(app, "layout-compact", "Compact (one icon)", true, combined, None::<&str>)?;
    let extended = CheckMenuItem::with_id(app, "layout-extended", "Extended (two icons)", true, !combined, None::<&str>)?;
    let refs: Vec<&dyn IsMenuItem<tauri::Wry>> = vec![&compact, &extended];
    Submenu::with_items(app, "Menu Bar Layout", true, &refs)
}

/// A provider's "Menu Bar Shows" submenu: "Most used" plus one entry per window.
fn window_picker(
    app: &AppHandle,
    provider: Provider,
    windows: &[TrayWindow],
    selected: &str,
) -> tauri::Result<tauri::menu::Submenu<tauri::Wry>> {
    use tauri::menu::{IsMenuItem, Submenu};

    let mut items: Vec<Box<dyn IsMenuItem<tauri::Wry>>> = vec![Box::new(CheckMenuItem::with_id(
        app,
        window_menu_id(provider, crate::prefs::TRAY_WINDOW_AUTO),
        "Most used",
        true,
        selected == crate::prefs::TRAY_WINDOW_AUTO,
        None::<&str>,
    )?)];
    for window in windows {
        items.push(Box::new(CheckMenuItem::with_id(
            app,
            window_menu_id(provider, &window.id),
            &window.label,
            true,
            selected == window.id,
            None::<&str>,
        )?));
    }
    let refs: Vec<&dyn IsMenuItem<tauri::Wry>> = items.iter().map(|item| item.as_ref()).collect();
    let title = format!("{} shows", provider.display_name());
    Submenu::with_items(app, title, true, &refs)
}

/// Builds the one shared tray menu: a "Menu Bar Shows" picker per available
/// provider, the app-wide toggles, the setup guide, and quit.
fn build_unified_menu(
    app: &AppHandle,
    codex_windows: &[TrayWindow],
    claude_windows: &[TrayWindow],
    claude_present: bool,
) -> tauri::Result<Menu<tauri::Wry>> {
    let prefs = app.state::<PrefsStore>().get();
    let refresh = MenuItem::with_id(app, "refresh-all", "Refresh", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;

    let codex_picker = window_picker(app, Provider::Codex, codex_windows, &prefs.codex_tray_window)?;
    let claude_picker =
        window_picker(app, Provider::Claude, claude_windows, &prefs.claude_tray_window)?;

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
    let layout = layout_submenu(app, prefs.combined_tray)?;
    let walkthrough = MenuItem::with_id(app, "show-onboarding", "Setup Guide…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit-app", "Quit UsageBar", true, None::<&str>)?;

    use tauri::menu::IsMenuItem;
    let mut items: Vec<&dyn IsMenuItem<tauri::Wry>> = vec![&refresh, &separator, &codex_picker];
    // The Claude picker only appears once a Claude login has been found.
    if claude_present {
        items.push(&claude_picker);
    }
    items.extend([
        &layout as &dyn IsMenuItem<tauri::Wry>,
        &separator,
        &alerts,
        &autostart,
        &separator,
        &walkthrough,
        &quit,
    ]);
    Menu::with_items(app, &items)
}

/// A single provider's menu for the two-icon layout: its window picker, plus
/// the app-wide toggles on the primary (Codex) item only.
fn build_provider_menu(
    app: &AppHandle,
    provider: Provider,
    windows: &[TrayWindow],
    include_shared: bool,
) -> tauri::Result<Menu<tauri::Wry>> {
    use tauri::menu::IsMenuItem;

    let prefs = app.state::<PrefsStore>().get();
    let selected = match provider {
        Provider::Codex => prefs.codex_tray_window.clone(),
        Provider::Claude => prefs.claude_tray_window.clone(),
    };
    let refresh = MenuItem::with_id(app, "refresh-all", "Refresh", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let picker = window_picker(app, provider, windows, &selected)?;
    let quit = MenuItem::with_id(app, "quit-app", "Quit UsageBar", true, None::<&str>)?;

    if !include_shared {
        return Menu::with_items(app, &[&refresh, &separator, &picker, &separator, &quit]);
    }

    let layout = layout_submenu(app, prefs.combined_tray)?;
    let alerts = CheckMenuItem::with_id(app, "toggle-alerts", "Usage Alerts", true, prefs.usage_alerts, None::<&str>)?;
    let autostart = CheckMenuItem::with_id(app, "toggle-autostart", "Launch at Login", true, app.autolaunch().is_enabled().unwrap_or(false), None::<&str>)?;
    let walkthrough = MenuItem::with_id(app, "show-onboarding", "Setup Guide…", true, None::<&str>)?;
    let items: Vec<&dyn IsMenuItem<tauri::Wry>> = vec![
        &refresh, &separator, &picker, &layout, &separator, &alerts, &autostart, &separator, &walkthrough, &quit,
    ];
    Menu::with_items(app, &items)
}

/// A provider's rendered menu-bar state for one refresh.
struct ProviderView {
    seg: MeterSegment,
    windows: Vec<TrayWindow>,
    selected: Option<TrayWindow>,
    updated_at: Option<u64>,
    present: bool,
}

async fn codex_view(app: &AppHandle, prefs: &crate::prefs::AppPrefs, now: u64) -> ProviderView {
    let state = app.state::<CodexManager>().inner().snapshot().await;
    let windows = collect_windows(state.rate_limits.as_ref());
    let selected = select_window(&windows, &prefs.codex_tray_window).cloned();
    let incoming = app
        .try_state::<ResetRadar>()
        .is_some_and(|radar| radar.incoming_at(now));
    let seg = MeterSegment {
        present: selected.is_some(),
        remaining: selected.as_ref().map(remaining_percent).unwrap_or(0.0),
        resets_at: selected.as_ref().and_then(|window| window.resets_at),
        incoming,
        stale: stale_age(state.updated_at, now).is_some(),
    };
    ProviderView { seg, windows, selected, updated_at: state.updated_at, present: true }
}

async fn claude_view(app: &AppHandle, prefs: &crate::prefs::AppPrefs, now: u64) -> ProviderView {
    let state = app.state::<ClaudeManager>().inner().snapshot().await;
    let present = state.connection != ConnectionState::CliNotFound;
    let windows = collect_windows(state.rate_limits.as_ref());
    let selected = select_window(&windows, &prefs.claude_tray_window).cloned();
    let seg = MeterSegment {
        present: present && selected.is_some(),
        remaining: selected.as_ref().map(remaining_percent).unwrap_or(0.0),
        resets_at: selected.as_ref().and_then(|window| window.resets_at),
        incoming: false,
        stale: stale_age(state.updated_at, now).is_some(),
    };
    ProviderView { seg, windows, selected, updated_at: state.updated_at, present }
}

/// Repaints the menu bar in whichever layout the user chose, toggling the
/// visibility of the combined item versus the per-provider items so only one
/// layout is on screen at a time.
pub async fn refresh_unified_tray(app: &AppHandle) {
    let prefs = app.state::<PrefsStore>().get();
    let now = now_unix_seconds();
    let codex = codex_view(app, &prefs, now).await;
    let claude = claude_view(app, &prefs, now).await;

    // Only the active layout's items are visible; the others stay created but
    // hidden so their menu handlers and click targets persist across toggles.
    set_visible(app, TRAY_ID, prefs.combined_tray);
    set_visible(app, CODEX_TRAY_ID, !prefs.combined_tray);
    set_visible(app, CLAUDE_TRAY_ID, !prefs.combined_tray && claude.present);

    let absent = MeterSegment::default();
    if prefs.combined_tray {
        let title = combined_title(codex.seg, claude.seg, now);
        let tooltip = unified_tooltip(
            &codex.seg,
            codex.selected.as_ref(),
            codex.updated_at,
            &claude.seg,
            claude.selected.as_ref(),
            claude.updated_at,
            now,
        );
        paint(app, TRAY_ID, &title, &tooltip);
    } else {
        let codex_title = combined_title(codex.seg, absent, now);
        let codex_tip = unified_tooltip(&codex.seg, codex.selected.as_ref(), codex.updated_at, &absent, None, None, now);
        paint(app, CODEX_TRAY_ID, &codex_title, &codex_tip);
        if claude.present {
            let claude_title = combined_title(absent, claude.seg, now);
            let claude_tip = unified_tooltip(&absent, None, None, &claude.seg, claude.selected.as_ref(), claude.updated_at, now);
            paint(app, CLAUDE_TRAY_ID, &claude_title, &claude_tip);
        }
    }

    sync_menus(app, &prefs, &codex, &claude);
}

fn set_visible(app: &AppHandle, id: &'static str, visible: bool) {
    {
        let cache = app.state::<TrayRenderCache>();
        let mut map = cache.visible.lock().expect("tray render cache poisoned");
        if map.get(id) == Some(&visible) {
            return;
        }
        map.insert(id, visible);
    }
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(tray) = handle.tray_by_id(id) {
            let _ = tray.set_visible(visible);
        }
    });
}

fn paint(app: &AppHandle, id: &'static str, title: &str, tooltip: &str) {
    {
        let cache = app.state::<TrayRenderCache>();
        let mut map = cache.labels.lock().expect("tray render cache poisoned");
        if map
            .get(id)
            .is_some_and(|(last_title, last_tip)| last_title == title && last_tip == tooltip)
        {
            return;
        }
        map.insert(id, (title.to_owned(), tooltip.to_owned()));
    }
    let handle = app.clone();
    let title = title.to_owned();
    let tooltip = tooltip.to_owned();
    let _ = app.run_on_main_thread(move || {
        if let Some(tray) = handle.tray_by_id(id) {
            let _ = tray.set_title(Some(title));
            let _ = tray.set_tooltip(Some(tooltip));
        }
    });
}

/// Rebuilds whichever menus the active layout needs, but only when their window
/// lists or preferences actually changed (native menu builds are not free).
fn sync_menus(app: &AppHandle, prefs: &crate::prefs::AppPrefs, codex: &ProviderView, claude: &ProviderView) {
    let windows_sig = |windows: &[TrayWindow]| {
        windows
            .iter()
            .map(|window| format!("{}={}", window.id, window.label))
            .collect::<Vec<_>>()
            .join(",")
    };
    let shared = format!("{}|{}", prefs.usage_alerts, prefs.combined_tray);

    if prefs.combined_tray {
        let sig = format!(
            "combined|{}|{}|{shared}|{}|{}|{}",
            prefs.codex_tray_window,
            prefs.claude_tray_window,
            windows_sig(&codex.windows),
            windows_sig(&claude.windows),
            claude.present,
        );
        rebuild_menu(app, TRAY_ID, sig, {
            let codex_windows = codex.windows.clone();
            let claude_windows = claude.windows.clone();
            let claude_present = claude.present;
            move |app| build_unified_menu(app, &codex_windows, &claude_windows, claude_present)
        });
    } else {
        let codex_sig = format!("codex|{}|{shared}|{}", prefs.codex_tray_window, windows_sig(&codex.windows));
        rebuild_menu(app, CODEX_TRAY_ID, codex_sig, {
            let windows = codex.windows.clone();
            move |app| build_provider_menu(app, Provider::Codex, &windows, true)
        });
        if claude.present {
            let claude_sig = format!("claude|{}|{}", prefs.claude_tray_window, windows_sig(&claude.windows));
            rebuild_menu(app, CLAUDE_TRAY_ID, claude_sig, {
                let windows = claude.windows.clone();
                move |app| build_provider_menu(app, Provider::Claude, &windows, false)
            });
        }
    }
}

fn rebuild_menu<F>(app: &AppHandle, id: &'static str, signature: String, build: F)
where
    F: FnOnce(&AppHandle) -> tauri::Result<Menu<tauri::Wry>> + Send + 'static,
{
    if !app.state::<TrayMenuState>().changed(id, &signature) {
        return;
    }
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let Some(tray) = handle.tray_by_id(id) else {
            return;
        };
        match build(&handle) {
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

fn window_tooltip_line(
    name: &str,
    seg: &MeterSegment,
    window: Option<&TrayWindow>,
    updated_at: Option<u64>,
    now: u64,
) -> String {
    let percent = seg.remaining.clamp(0.0, 100.0).round() as u32;
    let label = window.map(|window| window.label.as_str()).unwrap_or("usage");
    let mut line = format!("{name}: {percent}% left ({label})");
    if let Some(age) = stale_age(updated_at, now) {
        line.push_str(&format!(" · last updated {} ago", format_age(age)));
    } else if let Some(target) = seg.resets_at.filter(|target| *target > now) {
        line.push_str(&format!(" · resets in {}", format_countdown(target - now)));
    }
    line
}

fn unified_tooltip(
    codex: &MeterSegment,
    codex_window: Option<&TrayWindow>,
    codex_updated_at: Option<u64>,
    claude: &MeterSegment,
    claude_window: Option<&TrayWindow>,
    claude_updated_at: Option<u64>,
    now: u64,
) -> String {
    let mut lines = Vec::new();
    if codex.present {
        lines.push(window_tooltip_line("Codex", codex, codex_window, codex_updated_at, now));
    }
    if claude.present {
        lines.push(window_tooltip_line(
            "Claude Code",
            claude,
            claude_window,
            claude_updated_at,
            now,
        ));
    }
    if lines.is_empty() {
        return "UsageBar".to_owned();
    }
    lines.join("\n")
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
        refresh_tray(app);
        return;
    }

    match id {
        "refresh-all" => {
            let codex = app.state::<CodexManager>().inner().clone();
            let claude = app.state::<ClaudeManager>().inner().clone();
            tauri::async_runtime::spawn(async move {
                let _ = codex.refresh_or_start().await;
                let _ = claude.refresh().await;
            });
        }
        "layout-compact" | "layout-extended" => {
            let combined = id == "layout-compact";
            app.state::<PrefsStore>()
                .update(|prefs| prefs.combined_tray = combined);
            apply_preference_change(app);
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
                #[cfg(target_os = "macos")]
                crate::activate_app();
                let _ = window.set_focus();
            }
            let _ = app.emit("usagebar://show-onboarding", ());
        }
        "quit-app" => quit_from_menu(app),
        _ => {}
    }
}

fn now_unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Every tray item toggles the popover on a left click.
fn on_left_click(tray: &tauri::tray::TrayIcon, event: TrayIconEvent) {
    if let TrayIconEvent::Click {
        button: MouseButton::Left,
        button_state: MouseButtonState::Up,
        ..
    } = event
    {
        toggle_main_window(tray.app_handle());
    }
}

pub fn setup(app: &App) -> tauri::Result<()> {
    let handle = app.handle().clone();
    let combined = app.state::<PrefsStore>().get().combined_tray;

    // The combined item also owns the single global menu-event handler that
    // serves every item's menu, so it is always created (just hidden when the
    // two-icon layout is active).
    let unified_menu = build_unified_menu(&handle, &[], &[], false)?;
    TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("UsageBar")
        .icon(usagebar_tray_icon())
        .icon_as_template(false)
        .menu(&unified_menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| handle_menu_event(app, event.id.as_ref()))
        .on_tray_icon_event(on_left_click)
        .build(app)?;

    // Codex always exists, so its per-provider item is built up front (hidden
    // unless the two-icon layout is active). Claude's is built on first login.
    let codex_menu = build_provider_menu(&handle, Provider::Codex, &[], true)?;
    TrayIconBuilder::with_id(CODEX_TRAY_ID)
        .tooltip("Codex usage")
        .icon(codex_tray_icon())
        .icon_as_template(false)
        .menu(&codex_menu)
        .show_menu_on_left_click(false)
        .on_tray_icon_event(on_left_click)
        .build(app)?;
    set_visible(&handle, TRAY_ID, combined);
    set_visible(&handle, CODEX_TRAY_ID, !combined);
    Ok(())
}

/// Builds the Claude per-provider item on first login so the two-icon layout
/// has one to show. In the combined layout it stays created but hidden.
pub fn ensure_claude_tray(app: &AppHandle) -> tauri::Result<()> {
    if app.tray_by_id(CLAUDE_TRAY_ID).is_none() {
        let menu = build_provider_menu(app, Provider::Claude, &[], false)?;
        TrayIconBuilder::with_id(CLAUDE_TRAY_ID)
            .tooltip("Claude Code usage")
            .icon(claude_tray_icon())
            .icon_as_template(false)
            .menu(&menu)
            .show_menu_on_left_click(false)
            .on_tray_icon_event(on_left_click)
            .build(app)?;
    }
    app.state::<TrayMenuState>().invalidate();
    refresh_tray(app);
    Ok(())
}

/// The combined menu-bar mark: two rounded bars — Codex purple, Claude coral —
/// reading as one "usage bars" glyph for both providers under a single icon.
pub fn usagebar_tray_icon() -> Image<'static> {
    const WIDTH: u32 = 20;
    const HEIGHT: u32 = 18;
    const SAMPLES: u32 = 4;
    // (x0, x1, top, bottom, [r,g,b]) for each bar, in icon pixels.
    let bars = [
        (4.0_f64, 8.6_f64, 4.5_f64, 15.0_f64, [140.0_f64, 92.0, 240.0]),
        (11.4_f64, 16.0_f64, 8.0_f64, 15.0_f64, [217.0_f64, 119.0, 87.0]),
    ];
    let mut rgba = vec![0_u8; (WIDTH * HEIGHT * 4) as usize];

    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            for (x0, x1, top, bottom, color) in bars {
                let mut coverage = 0_u32;
                for sample_y in 0..SAMPLES {
                    for sample_x in 0..SAMPLES {
                        let px = x as f64 + (sample_x as f64 + 0.5) / SAMPLES as f64;
                        let py = y as f64 + (sample_y as f64 + 0.5) / SAMPLES as f64;
                        // Rounded ends: clamp the sample toward the bar's core
                        // and test a capsule radius.
                        let radius = (x1 - x0) / 2.0;
                        let cx = (x0 + x1) / 2.0;
                        let cy = py.clamp(top + radius, bottom - radius);
                        if (px - cx).powi(2) + (py - cy).powi(2) <= radius * radius {
                            coverage += 1;
                        }
                    }
                }
                if coverage == 0 {
                    continue;
                }
                let alpha = ((coverage * 255) / (SAMPLES * SAMPLES)) as u8;
                let index = ((y * WIDTH + x) * 4) as usize;
                // First bar to cover this pixel wins (they do not overlap).
                if rgba[index + 3] == 0 {
                    rgba[index] = color[0] as u8;
                    rgba[index + 1] = color[1] as u8;
                    rgba[index + 2] = color[2] as u8;
                    rgba[index + 3] = alpha;
                }
            }
        }
    }
    Image::new_owned(rgba, WIDTH, HEIGHT)
}

/// A compact Codex-purple version of the cloud/terminal mark. Retained for the
/// popover brand mark and tests even though the tray now uses a combined glyph.
#[allow(dead_code)]
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

/// Claude's coral starburst. Retained for reference and tests even though the
/// tray now uses the combined glyph.
#[allow(dead_code)]
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

/// Titles show percent REMAINING, mirroring what the Codex and Claude apps
/// display, so the menu bar never disagrees with the app it mirrors.
pub fn tray_title(remaining_percent: Option<f64>, resets_at: Option<u64>, now_unix: u64) -> String {
    let Some(remaining) = remaining_percent else {
        return String::new();
    };
    let percent = remaining.clamp(0.0, 100.0).round() as u32;
    match resets_at {
        Some(target) if target > now_unix => {
            format!("{percent}% · {}", format_countdown(target - now_unix))
        }
        _ => format!("{percent}%"),
    }
}

/// Percent of a window still available.
pub fn remaining_percent(window: &TrayWindow) -> f64 {
    (100.0 - window.used_percent).clamp(0.0, 100.0)
}

/// One provider's contribution to the single combined menu-bar title.
#[derive(Debug, Clone, Copy, Default)]
pub struct MeterSegment {
    /// Whether this provider has usable data to show at all.
    pub present: bool,
    pub remaining: f64,
    pub resets_at: Option<u64>,
    pub incoming: bool,
    pub stale: bool,
}

fn segment_percent(seg: &MeterSegment) -> String {
    let percent = seg.remaining.clamp(0.0, 100.0).round() as u32;
    with_incoming_prefix(with_stale_marker(format!("{percent}%"), seg.stale), seg.incoming)
}

/// The combined (compact-layout) menu-bar title, Codex first then Claude. A
/// lone provider keeps its countdown since there is room; two providers show
/// both percentages without countdowns so they fit in one narrow item.
pub fn combined_title(codex: MeterSegment, claude: MeterSegment, now: u64) -> String {
    let present: Vec<&MeterSegment> = [&codex, &claude]
        .into_iter()
        .filter(|seg| seg.present)
        .collect();
    match present.as_slice() {
        [] => String::new(),
        [only] => {
            let base = tray_title(Some(only.remaining), only.resets_at, now);
            with_incoming_prefix(with_stale_marker(base, only.stale), only.incoming)
        }
        segments => segments
            .iter()
            .map(|seg| segment_percent(seg))
            .collect::<Vec<_>>()
            .join(" · "),
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
    fn title_shows_remaining_percent_and_countdown() {
        assert_eq!(tray_title(Some(42.0), Some(4_661), 1_000), "42% · 1:01:01");
        assert_eq!(tray_title(Some(42.0), Some(1_545), 1_000), "42% · 9:05");
        assert_eq!(tray_title(Some(42.0), Some(200_000), 1_000), "42% · 2d 7h");
        assert_eq!(tray_title(Some(42.0), None, 1_000), "42%");
        assert_eq!(tray_title(Some(42.0), Some(900), 1_000), "42%");
        assert_eq!(tray_title(None, Some(4_661), 1_000), "");
    }

    #[test]
    fn remaining_percent_complements_used() {
        let window = TrayWindow {
            id: "w".into(),
            label: "Weekly".into(),
            used_percent: 38.0,
            resets_at: None,
            duration_mins: Some(10_080.0),
        };
        assert!((remaining_percent(&window) - 62.0).abs() < 1e-9);
    }

    #[test]
    fn combined_title_shows_both_providers_without_countdowns() {
        let codex = MeterSegment { present: true, remaining: 62.0, resets_at: Some(9_000), incoming: false, stale: false };
        let claude = MeterSegment { present: true, remaining: 8.0, resets_at: Some(9_000), incoming: false, stale: false };
        // Two providers: percentages only, joined, no countdowns.
        assert_eq!(combined_title(codex, claude, 1_000), "62% · 8%");
        // Markers still attach to the right segment.
        let stale_claude = MeterSegment { stale: true, ..claude };
        assert_eq!(combined_title(codex, stale_claude, 1_000), "62% · ~8%");
        let incoming_codex = MeterSegment { incoming: true, ..codex };
        assert_eq!(combined_title(incoming_codex, claude, 1_000), "⚡ 62% · 8%");
    }

    #[test]
    fn combined_title_keeps_the_countdown_for_a_lone_provider() {
        let codex = MeterSegment { present: true, remaining: 62.0, resets_at: Some(4_661), incoming: false, stale: false };
        let absent = MeterSegment::default();
        // A lone provider has room for the countdown.
        assert_eq!(combined_title(codex, absent, 1_000), "62% · 1:01:01");
        // Nothing present yields an empty title.
        assert_eq!(combined_title(absent, absent, 1_000), "");
        // Claude-only (Codex still loading) shows just Claude.
        let claude = MeterSegment { present: true, remaining: 8.0, resets_at: None, incoming: false, stale: false };
        assert_eq!(combined_title(absent, claude, 1_000), "8%");
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
    fn stale_numbers_are_marked_in_the_title() {
        let now = 100_000;
        // Fresh data, and data that has never arrived, are left alone.
        assert_eq!(stale_age(Some(now - 60), now), None);
        assert_eq!(stale_age(None, now), None);
        assert_eq!(stale_age(Some(now - TRAY_STALE_AFTER_SECS), now), Some(TRAY_STALE_AFTER_SECS));
        // A clock that jumped backwards must not read as stale.
        assert_eq!(stale_age(Some(now + 500), now), None);

        assert_eq!(with_stale_marker("42% · 9:05".into(), true), "~42% · 9:05");
        assert_eq!(with_stale_marker("42%".into(), false), "42%");
        // An empty title has no number to qualify, so it stays empty.
        assert_eq!(with_stale_marker(String::new(), true), "");
        // Stale and incoming compose without fighting over the prefix.
        assert_eq!(
            with_incoming_prefix(with_stale_marker("42%".into(), true), true),
            "⚡ ~42%"
        );
    }

    #[test]
    fn age_reads_in_the_largest_useful_unit() {
        assert_eq!(format_age(59), "0m");
        assert_eq!(format_age(15 * 60), "15m");
        assert_eq!(format_age(3 * 3_600 + 25 * 60), "3h 25m");
        assert_eq!(format_age(50 * 3_600), "2d 2h");
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
