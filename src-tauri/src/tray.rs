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
use crate::cursor::CursorManager;
use crate::opencode::OpenCodeManager;
use crate::prefs::PrefsStore;
use crate::provider::ProviderState;

/// The combined menu-bar item carrying both providers. One narrow item resists
/// macOS hiding it when the bar is crowded (or collides with a notch), which is
/// why it is the default; the per-provider items below are the opt-in layout.
pub const TRAY_ID: &str = "usagebar";
/// Per-provider items, used only in the "one icon per tool" layout.
pub const CODEX_TRAY_ID: &str = "provider-codex";
pub const CLAUDE_TRAY_ID: &str = "provider-claude";
pub const CURSOR_TRAY_ID: &str = "provider-cursor";
pub const OPENCODE_TRAY_ID: &str = "provider-opencode";

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
            // Same dismissal as the close button: give the menu bar and the
            // keyboard back to whatever the user was working in.
            #[cfg(target_os = "macos")]
            crate::hide_app();
        } else {
            // Activate first: dismissal hides the whole application, and a
            // window ordered in while the app is hidden stays off-screen.
            #[cfg(target_os = "macos")]
            crate::activate_app();
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
    Cursor,
    OpenCode,
}

impl Provider {
    pub const ALL: [Provider; 4] = [
        Provider::Codex,
        Provider::Claude,
        Provider::Cursor,
        Provider::OpenCode,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Provider::Codex => crate::prefs::PROVIDER_CODEX,
            Provider::Claude => crate::prefs::PROVIDER_CLAUDE,
            Provider::Cursor => crate::prefs::PROVIDER_CURSOR,
            Provider::OpenCode => crate::prefs::PROVIDER_OPENCODE,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Provider::Codex => "Codex",
            Provider::Claude => "Claude Code",
            Provider::Cursor => "Cursor",
            Provider::OpenCode => "OpenCode Go",
        }
    }

    pub     fn tray_id(self) -> &'static str {
        match self {
            Provider::Codex => CODEX_TRAY_ID,
            Provider::Claude => CLAUDE_TRAY_ID,
            Provider::Cursor => CURSOR_TRAY_ID,
            Provider::OpenCode => OPENCODE_TRAY_ID,
        }
    }

    /// Menu-bar ink for this tool. Compact layout uses these as left-to-right
    /// bars next to the percentages; extended layout paints the same colors
    /// into each tool's logo. Distinct on purpose: purple Codex, coral Claude,
    /// teal Cursor, indigo OpenCode.
    fn color(self) -> [f64; 3] {
        match self {
            Provider::Codex => [140.0, 92.0, 240.0],
            Provider::Claude => [217.0, 119.0, 87.0],
            Provider::Cursor => [15.0, 157.0, 142.0],
            Provider::OpenCode => [79.0, 70.0, 229.0],
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|provider| provider.key() == key)
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
    icons: std::sync::Mutex<std::collections::HashMap<&'static str, String>>,
}

/// Re-syncs the tray after a preference changes anywhere (tray menu or the
/// in-app settings panel), so the title and menu checkmarks stay in agreement.
pub fn apply_preference_change(app: &AppHandle) {
    app.state::<TrayMenuState>().invalidate();
    refresh_tray(app);
    let prefs = app.state::<PrefsStore>().get();
    let _ = app.emit("usagebar://prefs", prefs);
}

fn window_menu_id(provider: Provider, window_id: &str) -> String {
    format!("win|{}|{}", provider.key(), window_id)
}

/// The Compact/Extended layout picker, shown on the shared (primary) menu.
fn layout_submenu(app: &AppHandle, combined: bool) -> tauri::Result<tauri::menu::Submenu<tauri::Wry>> {
    use tauri::menu::{IsMenuItem, Submenu};
    let compact = CheckMenuItem::with_id(app, "layout-compact", "Compact (one icon)", true, combined, None::<&str>)?;
    let extended = CheckMenuItem::with_id(app, "layout-extended", "Extended (one icon per tool)", true, !combined, None::<&str>)?;
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
    views: &[(Provider, Vec<TrayWindow>)],
) -> tauri::Result<Menu<tauri::Wry>> {
    let prefs = app.state::<PrefsStore>().get();
    let refresh = MenuItem::with_id(app, "refresh-all", "Refresh", true, None::<&str>)?;
    // Each separator must be its own instance: a `PredefinedMenuItem` wraps a
    // single native menu item, and macOS cannot place one item at several
    // positions, so reusing one dropped whatever followed it (the Menu Bar
    // Layout submenu, intermittently) from the rebuilt menu.
    let sep_after_refresh = PredefinedMenuItem::separator(app)?;
    let sep_after_pickers = PredefinedMenuItem::separator(app)?;
    let sep_after_toggles = PredefinedMenuItem::separator(app)?;

    let mut pickers = Vec::new();
    for (provider, windows) in views {
        pickers.push(window_picker(
            app,
            *provider,
            windows,
            &prefs.tray_window(provider.key()),
        )?);
    }

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
    let mut items: Vec<&dyn IsMenuItem<tauri::Wry>> = vec![&refresh, &sep_after_refresh];
    for picker in &pickers {
        items.push(picker);
    }
    items.extend([
        &layout as &dyn IsMenuItem<tauri::Wry>,
        &sep_after_pickers,
        &alerts,
        &autostart,
        &sep_after_toggles,
        &walkthrough,
        &quit,
    ]);
    Menu::with_items(app, &items)
}

/// A single provider's menu for the two-icon layout: its own window picker plus
/// the full app-wide section (layout switch, toggles, setup guide, quit). Every
/// provider icon carries the whole section so either one can reach every
/// setting — right-clicking Claude must not be a dead end that can't even
/// switch back to the compact layout.
fn build_provider_menu(
    app: &AppHandle,
    provider: Provider,
    windows: &[TrayWindow],
) -> tauri::Result<Menu<tauri::Wry>> {
    use tauri::menu::IsMenuItem;

    let prefs = app.state::<PrefsStore>().get();
    let selected = prefs.tray_window(provider.key());
    let refresh = MenuItem::with_id(app, "refresh-all", "Refresh", true, None::<&str>)?;
    // Distinct separator instances — see build_unified_menu; one shared item
    // cannot occupy several menu positions on macOS.
    let sep_after_refresh = PredefinedMenuItem::separator(app)?;
    let sep_after_picker = PredefinedMenuItem::separator(app)?;
    let sep_after_toggles = PredefinedMenuItem::separator(app)?;
    let picker = window_picker(app, provider, windows, &selected)?;
    let layout = layout_submenu(app, prefs.combined_tray)?;
    let alerts = CheckMenuItem::with_id(app, "toggle-alerts", "Usage Alerts", true, prefs.usage_alerts, None::<&str>)?;
    let autostart = CheckMenuItem::with_id(app, "toggle-autostart", "Launch at Login", true, app.autolaunch().is_enabled().unwrap_or(false), None::<&str>)?;
    let walkthrough = MenuItem::with_id(app, "show-onboarding", "Setup Guide…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit-app", "Quit UsageBar", true, None::<&str>)?;
    let items: Vec<&dyn IsMenuItem<tauri::Wry>> = vec![
        &refresh, &sep_after_refresh, &picker, &layout, &sep_after_picker, &alerts, &autostart, &sep_after_toggles, &walkthrough, &quit,
    ];
    Menu::with_items(app, &items)
}

/// A provider's rendered menu-bar state for one refresh.
struct ProviderView {
    provider: Provider,
    seg: MeterSegment,
    windows: Vec<TrayWindow>,
    selected: Option<TrayWindow>,
    updated_at: Option<u64>,
    present: bool,
}

fn view_from_state(
    provider: Provider,
    prefs: &crate::prefs::AppPrefs,
    now: u64,
    connection: ConnectionState,
    rate_limits: Option<&Value>,
    updated_at: Option<u64>,
    incoming: bool,
    always_listed: bool,
) -> ProviderView {
    let visible = prefs.is_visible(provider.key());
    let found = connection != ConnectionState::CliNotFound;
    let present = visible && (always_listed || found);
    let windows = collect_windows(rate_limits);
    let selected = select_window(&windows, &prefs.tray_window(provider.key())).cloned();
    let seg = MeterSegment {
        present: present && selected.is_some(),
        remaining: selected.as_ref().map(remaining_percent).unwrap_or(0.0),
        resets_at: selected.as_ref().and_then(|window| window.resets_at),
        incoming,
        stale: stale_age(updated_at, now).is_some(),
    };
    ProviderView {
        provider,
        seg,
        windows,
        selected,
        updated_at,
        present,
    }
}

async fn all_provider_views(app: &AppHandle, prefs: &crate::prefs::AppPrefs, now: u64) -> Vec<ProviderView> {
    let incoming = app
        .try_state::<ResetRadar>()
        .is_some_and(|radar| radar.incoming_at(now));

    let mut views = Vec::new();

    if let Some(manager) = app.try_state::<CodexManager>() {
        let state = manager.inner().snapshot().await;
        views.push(view_from_state(
            Provider::Codex,
            prefs,
            now,
            state.connection,
            state.rate_limits.as_ref(),
            state.updated_at,
            incoming,
            true,
        ));
    }
    if let Some(manager) = app.try_state::<ClaudeManager>() {
        let state = manager.inner().snapshot().await;
        views.push(view_from_state(
            Provider::Claude,
            prefs,
            now,
            state.connection,
            state.rate_limits.as_ref(),
            state.updated_at,
            false,
            false,
        ));
    }
    if let Some(manager) = app.try_state::<CursorManager>() {
        let state = manager.inner().snapshot().await;
        views.push(optional_view(Provider::Cursor, prefs, now, &state));
    }
    if let Some(manager) = app.try_state::<OpenCodeManager>() {
        let state = manager.inner().snapshot().await;
        views.push(optional_view(Provider::OpenCode, prefs, now, &state));
    }
    views
}

fn optional_view(
    provider: Provider,
    prefs: &crate::prefs::AppPrefs,
    now: u64,
    state: &ProviderState,
) -> ProviderView {
    view_from_state(
        provider,
        prefs,
        now,
        state.connection,
        state.rate_limits.as_ref(),
        state.updated_at,
        false,
        false,
    )
}

/// Repaints the menu bar in whichever layout the user chose, toggling the
/// visibility of the combined item versus the per-provider items so only one
/// layout is on screen at a time.
pub async fn refresh_unified_tray(app: &AppHandle) {
    let Some(prefs) = app.try_state::<PrefsStore>().map(|store| store.get()) else {
        return;
    };
    let now = now_unix_seconds();
    let views = all_provider_views(app, &prefs, now).await;

    set_visible(app, TRAY_ID, prefs.combined_tray);
    for provider in Provider::ALL {
        let present = views
            .iter()
            .find(|view| view.provider == provider)
            .is_some_and(|view| view.present);
        set_visible(app, provider.tray_id(), !prefs.combined_tray && present);
    }

    if prefs.combined_tray {
        let showing: Vec<Provider> = views
            .iter()
            .filter(|view| view.seg.present)
            .map(|view| view.provider)
            .collect();
        paint_combined_icon(app, &showing);
        let segments: Vec<MeterSegment> = views.iter().map(|view| view.seg).collect();
        let title = combined_title(&segments, now);
        let tooltip = unified_tooltip(&views, now);
        paint(app, TRAY_ID, &title, &tooltip);
    } else {
        for view in &views {
            if !view.present {
                continue;
            }
            let title = combined_title(&[view.seg], now);
            let tooltip = provider_tooltip(view, now);
            paint(app, view.provider.tray_id(), &title, &tooltip);
        }
    }

    sync_menus(app, &prefs, &views);
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

/// Compact layout: one colored bar per tool currently in the title, in the
/// same order as the percentages. A lone tool gets its full logo instead.
/// Cached by which tools are showing so the 1s tick does not rebuild AppKit
/// images.
fn paint_combined_icon(app: &AppHandle, providers: &[Provider]) {
    let signature = if providers.is_empty() {
        "brand".to_owned()
    } else {
        providers
            .iter()
            .map(|provider| provider.key())
            .collect::<Vec<_>>()
            .join("+")
    };
    {
        let cache = app.state::<TrayRenderCache>();
        let mut map = cache.icons.lock().expect("tray render cache poisoned");
        if map.get(TRAY_ID) == Some(&signature) {
            return;
        }
        map.insert(TRAY_ID, signature);
    }
    let icon = combined_tray_icon(providers);
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(tray) = handle.tray_by_id(TRAY_ID) {
            let _ = tray.set_icon(Some(icon));
        }
    });
}

/// Rebuilds whichever menus the active layout needs, but only when their window
/// lists or preferences actually changed (native menu builds are not free).
fn sync_menus(app: &AppHandle, prefs: &crate::prefs::AppPrefs, views: &[ProviderView]) {
    let windows_sig = |windows: &[TrayWindow]| {
        windows
            .iter()
            .map(|window| format!("{}={}", window.id, window.label))
            .collect::<Vec<_>>()
            .join(",")
    };
    let shared = format!("{}|{}", prefs.usage_alerts, prefs.combined_tray);
    let listed: Vec<&ProviderView> = views.iter().filter(|view| view.present).collect();

    if prefs.combined_tray {
        let picker_sig = listed
            .iter()
            .map(|view| {
                format!(
                    "{}:{}:{}",
                    view.provider.key(),
                    prefs.tray_window(view.provider.key()),
                    windows_sig(&view.windows)
                )
            })
            .collect::<Vec<_>>()
            .join("|");
        let sig = format!("combined|{shared}|{picker_sig}");
        let menu_views: Vec<(Provider, Vec<TrayWindow>)> = listed
            .iter()
            .map(|view| (view.provider, view.windows.clone()))
            .collect();
        rebuild_menu(app, TRAY_ID, sig, move |app| build_unified_menu(app, &menu_views));
    } else {
        for view in listed {
            let sig = format!(
                "{}|{}|{shared}|{}",
                view.provider.key(),
                prefs.tray_window(view.provider.key()),
                windows_sig(&view.windows)
            );
            let provider = view.provider;
            let windows = view.windows.clone();
            rebuild_menu(app, provider.tray_id(), sig, move |app| {
                build_provider_menu(app, provider, &windows)
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

fn unified_tooltip(views: &[ProviderView], now: u64) -> String {
    let mut lines = Vec::new();
    for view in views {
        if view.seg.present {
            lines.push(window_tooltip_line(
                view.provider.display_name(),
                &view.seg,
                view.selected.as_ref(),
                view.updated_at,
                now,
            ));
        }
    }
    if lines.is_empty() {
        return "UsageBar".to_owned();
    }
    lines.join("\n")
}

fn provider_tooltip(view: &ProviderView, now: u64) -> String {
    if view.seg.present {
        window_tooltip_line(
            view.provider.display_name(),
            &view.seg,
            view.selected.as_ref(),
            view.updated_at,
            now,
        )
    } else {
        format!("{} usage", view.provider.display_name())
    }
}

pub fn refresh_all_providers(app: &AppHandle) {
    let prefs = app.state::<PrefsStore>().get();
    let codex = app.try_state::<CodexManager>().map(|state| state.inner().clone());
    let claude = app.try_state::<ClaudeManager>().map(|state| state.inner().clone());
    let cursor = app.try_state::<CursorManager>().map(|state| state.inner().clone());
    let opencode = app.try_state::<OpenCodeManager>().map(|state| state.inner().clone());
    tauri::async_runtime::spawn(async move {
        if prefs.is_visible(crate::prefs::PROVIDER_CODEX) {
            if let Some(codex) = codex {
                let _ = codex.refresh_or_start().await;
            }
        }
        if prefs.is_visible(crate::prefs::PROVIDER_CLAUDE) {
            if let Some(claude) = claude {
                let _ = claude.refresh().await;
            }
        }
        if prefs.is_visible(crate::prefs::PROVIDER_CURSOR) {
            if let Some(cursor) = cursor {
                let _ = cursor.refresh().await;
            }
        }
        if prefs.is_visible(crate::prefs::PROVIDER_OPENCODE) {
            if let Some(opencode) = opencode {
                let _ = opencode.refresh().await;
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
        let Some(provider) = Provider::from_key(key) else {
            return;
        };
        app.state::<PrefsStore>()
            .update(|prefs| prefs.set_tray_window(provider.key(), window_id.to_owned()));
        app.state::<TrayMenuState>().invalidate();
        refresh_tray(app);
        return;
    }

    match id {
        "refresh-all" => {
            refresh_all_providers(app);
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
                #[cfg(target_os = "macos")]
                crate::activate_app();
                let _ = window.show();
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
    // per-tool layout is active).
    let unified_menu = build_unified_menu(&handle, &[])?;
    TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("UsageBar")
        .icon(usagebar_tray_icon())
        .icon_as_template(false)
        .menu(&unified_menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| handle_menu_event(app, event.id.as_ref()))
        .on_tray_icon_event(on_left_click)
        .build(app)?;

    for provider in Provider::ALL {
        let menu = build_provider_menu(&handle, provider, &[])?;
        TrayIconBuilder::with_id(provider.tray_id())
            .tooltip(&format!("{} usage", provider.display_name()))
            .icon(provider_tray_icon(provider))
            .icon_as_template(false)
            .menu(&menu)
            .show_menu_on_left_click(false)
            .on_tray_icon_event(on_left_click)
            .build(app)?;
        set_visible(&handle, provider.tray_id(), false);
    }
    set_visible(&handle, TRAY_ID, combined);
    if !combined {
        set_visible(&handle, CODEX_TRAY_ID, true);
    }
    Ok(())
}

fn provider_tray_icon(provider: Provider) -> Image<'static> {
    match provider {
        Provider::Codex => codex_tray_icon(),
        Provider::Claude => claude_tray_icon(),
        Provider::Cursor => cursor_tray_icon(),
        Provider::OpenCode => opencode_tray_icon(),
    }
}

/// Compact menu-bar mark for the tools currently in the title. One tool keeps
/// its logo; several tools become staggered colored bars in title order so the
/// percentages can be told apart the same way Codex purple and Claude coral
/// already are. An empty set falls back to the two-bar brand mark.
fn combined_tray_icon(providers: &[Provider]) -> Image<'static> {
    match providers {
        [] => bars_tray_icon(&[Provider::Codex.color(), Provider::Claude.color()]),
        [only] => provider_tray_icon(*only),
        many => {
            let colors: Vec<[f64; 3]> = many.iter().map(|provider| provider.color()).collect();
            bars_tray_icon(&colors)
        }
    }
}

/// The app-wide brand mark: Codex purple + Claude coral bars. Also the compact
/// icon when those two tools are the ones showing.
pub fn usagebar_tray_icon() -> Image<'static> {
    combined_tray_icon(&[Provider::Codex, Provider::Claude])
}

fn bar_slots(count: usize) -> Vec<(f64, f64, f64, f64)> {
    match count {
        0 | 1 => vec![(6.4, 13.6, 4.5, 15.0)],
        2 => vec![(4.0, 8.6, 4.5, 15.0), (11.4, 16.0, 8.0, 15.0)],
        3 => vec![
            (2.4, 6.8, 4.5, 15.0),
            (8.0, 12.4, 7.0, 15.0),
            (13.6, 18.0, 9.2, 15.0),
        ],
        _ => vec![
            (1.6, 5.0, 4.2, 15.0),
            (6.2, 9.6, 6.0, 15.0),
            (10.8, 14.2, 7.8, 15.0),
            (15.4, 18.8, 9.6, 15.0),
        ],
    }
}

fn bars_tray_icon(colors: &[[f64; 3]]) -> Image<'static> {
    const WIDTH: u32 = 20;
    const HEIGHT: u32 = 18;
    const SAMPLES: u32 = 4;
    let bars: Vec<(f64, f64, f64, f64, [f64; 3])> = bar_slots(colors.len())
        .into_iter()
        .zip(colors.iter().copied())
        .map(|(slot, color)| (slot.0, slot.1, slot.2, slot.3, color))
        .collect();
    let mut rgba = vec![0_u8; (WIDTH * HEIGHT * 4) as usize];

    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            for &(x0, x1, top, bottom, color) in &bars {
                let mut coverage = 0_u32;
                for sample_y in 0..SAMPLES {
                    for sample_x in 0..SAMPLES {
                        let px = x as f64 + (sample_x as f64 + 0.5) / SAMPLES as f64;
                        let py = y as f64 + (sample_y as f64 + 0.5) / SAMPLES as f64;
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

/// The Codex provider's own menu-bar mark for the extended (one-icon-per-tool)
/// layout: the cloud/terminal glyph in Codex purple. Compact layout uses
/// colored bars (`combined_tray_icon`) when more than one tool is showing.
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

/// Claude's coral starburst for the extended (one-icon-per-tool) layout.
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

/// Cursor's teal pointer. Same canvas as Codex/Claude so the four extended
/// icons sit at the same visual weight in the menu bar.
pub fn cursor_tray_icon() -> Image<'static> {
    const WIDTH: u32 = 22;
    const HEIGHT: u32 = 18;
    const SAMPLES: u32 = 4;
    let mut rgba = vec![0_u8; (WIDTH * HEIGHT * 4) as usize];

    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let mut coverage = 0_u32;
            for sample_y in 0..SAMPLES {
                for sample_x in 0..SAMPLES {
                    let px = x as f64 + (sample_x as f64 + 0.5) / SAMPLES as f64;
                    let py = y as f64 + (sample_y as f64 + 0.5) / SAMPLES as f64;
                    if inside_cursor_pointer(px, py) {
                        coverage += 1;
                    }
                }
            }
            if coverage == 0 {
                continue;
            }
            let alpha = ((coverage * 255) / (SAMPLES * SAMPLES)) as u8;
            let blend = y as f64 / (HEIGHT - 1) as f64;
            let red = (15.0 + (11.0 - 15.0) * blend).round() as u8;
            let green = (157.0 + (128.0 - 157.0) * blend).round() as u8;
            let blue = (142.0 + (116.0 - 142.0) * blend).round() as u8;
            let index = ((y * WIDTH + x) * 4) as usize;
            rgba[index..index + 4].copy_from_slice(&[red, green, blue, alpha]);
        }
    }
    Image::new_owned(rgba, WIDTH, HEIGHT)
}

fn inside_cursor_pointer(x: f64, y: f64) -> bool {
    // Classic arrow cursor, tip at top-left, wing to the right, notch + tail
    // down the shaft — the Cursor app mark, sized for a 22×18 tray canvas.
    const VERTS: [(f64, f64); 7] = [
        (5.0, 2.2),
        (5.3, 15.5),
        (8.6, 12.1),
        (10.1, 16.6),
        (12.4, 15.6),
        (10.0, 11.4),
        (16.6, 10.1),
    ];
    point_in_polygon(x, y, &VERTS)
}

/// OpenCode's indigo O: a rounded rectangular ring, the square brand mark
/// compressed onto the same 22×18 canvas as the other tray logos.
pub fn opencode_tray_icon() -> Image<'static> {
    const WIDTH: u32 = 22;
    const HEIGHT: u32 = 18;
    const SAMPLES: u32 = 4;
    let mut rgba = vec![0_u8; (WIDTH * HEIGHT * 4) as usize];

    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let mut coverage = 0_u32;
            for sample_y in 0..SAMPLES {
                for sample_x in 0..SAMPLES {
                    let px = x as f64 + (sample_x as f64 + 0.5) / SAMPLES as f64;
                    let py = y as f64 + (sample_y as f64 + 0.5) / SAMPLES as f64;
                    if inside_opencode_mark(px, py) {
                        coverage += 1;
                    }
                }
            }
            if coverage == 0 {
                continue;
            }
            let alpha = ((coverage * 255) / (SAMPLES * SAMPLES)) as u8;
            let blend = y as f64 / (HEIGHT - 1) as f64;
            let red = (79.0 + (67.0 - 79.0) * blend).round() as u8;
            let green = (70.0 + (56.0 - 70.0) * blend).round() as u8;
            let blue = (229.0 + (202.0 - 229.0) * blend).round() as u8;
            let index = ((y * WIDTH + x) * 4) as usize;
            rgba[index..index + 4].copy_from_slice(&[red, green, blue, alpha]);
        }
    }
    Image::new_owned(rgba, WIDTH, HEIGHT)
}

fn inside_opencode_mark(x: f64, y: f64) -> bool {
    inside_rounded_rect(x, y, 5.0, 2.2, 17.0, 15.8, 3.8)
        && !inside_rounded_rect(x, y, 8.6, 5.8, 13.4, 12.2, 1.6)
}

fn inside_rounded_rect(x: f64, y: f64, x0: f64, y0: f64, x1: f64, y1: f64, radius: f64) -> bool {
    let cx = x.clamp(x0 + radius, x1 - radius);
    let cy = y.clamp(y0 + radius, y1 - radius);
    (x - cx).powi(2) + (y - cy).powi(2) <= radius * radius
}

fn point_in_polygon(x: f64, y: f64, verts: &[(f64, f64)]) -> bool {
    let mut inside = false;
    let mut j = verts.len() - 1;
    for i in 0..verts.len() {
        let (xi, yi) = verts[i];
        let (xj, yj) = verts[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
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

/// The combined (compact-layout) menu-bar title. A lone provider keeps its
/// countdown since there is room; several providers show percentages without
/// countdowns so they fit in one narrow item.
pub fn combined_title(segments: &[MeterSegment], now: u64) -> String {
    let present: Vec<&MeterSegment> = segments.iter().filter(|seg| seg.present).collect();
    match present.as_slice() {
        [] => String::new(),
        [only] => {
            let base = tray_title(Some(only.remaining), only.resets_at, now);
            with_incoming_prefix(with_stale_marker(base, only.stale), only.incoming)
        }
        many => many
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
        assert_eq!(combined_title(&[codex, claude], 1_000), "62% · 8%");
        // Markers still attach to the right segment.
        let stale_claude = MeterSegment { stale: true, ..claude };
        assert_eq!(combined_title(&[codex, stale_claude], 1_000), "62% · ~8%");
        let incoming_codex = MeterSegment { incoming: true, ..codex };
        assert_eq!(combined_title(&[incoming_codex, claude], 1_000), "⚡ 62% · 8%");
        let cursor = MeterSegment { present: true, remaining: 41.0, resets_at: None, incoming: false, stale: false };
        assert_eq!(combined_title(&[codex, claude, cursor], 1_000), "62% · 8% · 41%");
    }

    #[test]
    fn combined_title_keeps_the_countdown_for_a_lone_provider() {
        let codex = MeterSegment { present: true, remaining: 62.0, resets_at: Some(4_661), incoming: false, stale: false };
        let absent = MeterSegment::default();
        // A lone provider has room for the countdown.
        assert_eq!(combined_title(&[codex, absent], 1_000), "62% · 1:01:01");
        // Nothing present yields an empty title.
        assert_eq!(combined_title(&[absent, absent], 1_000), "");
        // Claude-only (Codex still loading) shows just Claude.
        let claude = MeterSegment { present: true, remaining: 8.0, resets_at: None, incoming: false, stale: false };
        assert_eq!(combined_title(&[absent, claude], 1_000), "8%");
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
    fn cursor_icon_is_a_teal_pointer() {
        let icon = cursor_tray_icon();
        let rgba = icon.rgba();
        let pixel_at = |x: usize, y: usize| &rgba[(y * 22 + x) * 4..(y * 22 + x) * 4 + 4];
        assert_eq!(pixel_at(0, 0)[3], 0);
        let shaft = pixel_at(7, 8);
        assert!(shaft[3] > 200);
        assert!(shaft[1] > shaft[0] + 80);
        assert!(shaft[2] > shaft[0] + 60);
        let wing = pixel_at(14, 10);
        assert!(wing[3] > 0);
        assert_eq!(pixel_at(21, 2)[3], 0);
    }

    #[test]
    fn opencode_icon_is_an_indigo_o() {
        let icon = opencode_tray_icon();
        let rgba = icon.rgba();
        let pixel_at = |x: usize, y: usize| &rgba[(y * 22 + x) * 4..(y * 22 + x) * 4 + 4];
        assert_eq!(pixel_at(0, 0)[3], 0);
        let ring = pixel_at(6, 9);
        assert!(ring[3] > 200);
        assert!(ring[2] > ring[0] + 80);
        assert!(ring[2] > ring[1] + 80);
        assert_eq!(pixel_at(11, 9)[3], 0);
    }

    #[test]
    fn combined_icon_for_one_provider_is_that_providers_logo() {
        assert_eq!(
            combined_tray_icon(&[Provider::Cursor]).rgba(),
            cursor_tray_icon().rgba()
        );
        assert_eq!(
            combined_tray_icon(&[Provider::OpenCode]).rgba(),
            opencode_tray_icon().rgba()
        );
    }

    #[test]
    fn combined_icon_bars_use_each_providers_color_in_title_order() {
        let icon = combined_tray_icon(&[
            Provider::Codex,
            Provider::Claude,
            Provider::Cursor,
            Provider::OpenCode,
        ]);
        let rgba = icon.rgba();
        let pixel_at = |x: usize, y: usize| &rgba[(y * 20 + x) * 4..(y * 20 + x) * 4 + 4];
        let purple = pixel_at(3, 10);
        assert!(purple[3] > 200);
        assert!(purple[2] > purple[1] && purple[0] > purple[1]);
        let coral = pixel_at(8, 12);
        assert!(coral[3] > 200);
        assert!(coral[0] > coral[1] && coral[1] > coral[2]);
        let teal = pixel_at(12, 12);
        assert!(teal[3] > 200);
        assert!(teal[1] > teal[0] + 80);
        let indigo = pixel_at(17, 12);
        assert!(indigo[3] > 200);
        assert!(indigo[2] > indigo[0] + 80);
    }
}
