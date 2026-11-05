use serde_json::Value;
use tauri::{
    image::Image,
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    App, AppHandle, Emitter, Manager,
};
use tauri_plugin_autostart::ManagerExt;

use crate::codex::process::{CodexManager, ConnectionState};
use crate::prefs::PrefsStore;
use crate::provider::now_unix_seconds;
pub use crate::registry::Provider;
use crate::registry::{Meters, Reading};
use crate::updates::UpdateStatus;

/// The combined menu-bar item carrying both providers. One narrow item resists
/// macOS hiding it when the bar is crowded (or collides with a notch), which is
/// why it is the default; the per-provider items (`Provider::tray_id`) are the
/// opt-in layout.
pub const TRAY_ID: &str = "usagebar";

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
        if window.is_visible().unwrap_or(false) && !window.is_minimized().unwrap_or(false) {
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
            if crate::demo::enabled() {
                crate::demo::place_popover(&window);
                let _ = window.set_always_on_top(true);
            }
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
            crate::claude::refresh_on_open(app);
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

/// Menu-bar specifics; the rest of each provider's static data lives in
/// `registry::Provider::info`.
impl Provider {
    /// The provider's own item in the one-icon-per-tool layout.
    pub fn tray_id(self) -> &'static str {
        self.info().tray_id
    }

    fn color(self) -> [f64; 3] {
        self.info().color
    }
}

/// Remembers the menu each tray currently displays so the once-per-second
/// title tick does not rebuild native menus continuously.
#[derive(Default)]
pub struct TrayMenuState(std::sync::Mutex<std::collections::HashMap<&'static str, String>>);

impl TrayMenuState {
    fn changed(&self, key: &'static str, signature: &str) -> bool {
        let mut map = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if map.get(key).map(String::as_str) == Some(signature) {
            return false;
        }
        map.insert(key, signature.to_owned());
        true
    }

    pub(crate) fn invalidate(&self) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
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
fn layout_submenu(
    app: &AppHandle,
    combined: bool,
) -> tauri::Result<tauri::menu::Submenu<tauri::Wry>> {
    use tauri::menu::{IsMenuItem, Submenu};
    let compact = CheckMenuItem::with_id(
        app,
        "layout-compact",
        "Compact (one icon)",
        true,
        combined,
        None::<&str>,
    )?;
    let extended = CheckMenuItem::with_id(
        app,
        "layout-extended",
        "Extended (one icon per tool)",
        true,
        !combined,
        None::<&str>,
    )?;
    let refs: Vec<&dyn IsMenuItem<tauri::Wry>> = vec![&compact, &extended];
    Submenu::with_items(app, "Menu Bar Layout", true, &refs)
}

/// The updates row: a manual check, or the restart action once an update
/// has downloaded and is parked waiting for a relaunch.
fn update_menu_item(app: &AppHandle) -> tauri::Result<MenuItem<tauri::Wry>> {
    if let Some(version) = app.state::<UpdateStatus>().pending_version() {
        return MenuItem::with_id(
            app,
            "restart-update",
            format!("Restart to Update — v{version}"),
            true,
            None::<&str>,
        );
    }
    MenuItem::with_id(
        app,
        "check-updates",
        "Check for Updates…",
        true,
        None::<&str>,
    )
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
    let updates = update_menu_item(app)?;
    let walkthrough =
        MenuItem::with_id(app, "show-onboarding", "Setup Guide…", true, None::<&str>)?;
    let star = MenuItem::with_id(
        app,
        "star-github",
        "Star UsageBar on GitHub",
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit-app", "Quit UsageBar", true, None::<&str>)?;

    use tauri::menu::IsMenuItem;
    let mut items: Vec<&dyn IsMenuItem<tauri::Wry>> = vec![&refresh, &sep_after_refresh];
    #[cfg(target_os = "windows")]
    let show_widget = MenuItem::with_id(
        app,
        "show-codex-widget",
        "Show Codex meter",
        true,
        None::<&str>,
    )?;
    #[cfg(target_os = "windows")]
    items.push(&show_widget);
    for picker in &pickers {
        items.push(picker);
    }
    items.extend([
        &layout as &dyn IsMenuItem<tauri::Wry>,
        &sep_after_pickers,
        &alerts,
        &autostart,
        &updates,
        &sep_after_toggles,
        &walkthrough,
        &star,
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
    let updates = update_menu_item(app)?;
    let walkthrough =
        MenuItem::with_id(app, "show-onboarding", "Setup Guide…", true, None::<&str>)?;
    let star = MenuItem::with_id(
        app,
        "star-github",
        "Star UsageBar on GitHub",
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit-app", "Quit UsageBar", true, None::<&str>)?;
    #[cfg_attr(not(target_os = "windows"), allow(unused_mut))]
    let mut items: Vec<&dyn IsMenuItem<tauri::Wry>> = vec![
        &refresh,
        &sep_after_refresh,
        &picker,
        &layout,
        &sep_after_picker,
        &alerts,
        &autostart,
        &updates,
        &sep_after_toggles,
        &walkthrough,
        &star,
        &quit,
    ];
    #[cfg(target_os = "windows")]
    let show_widget = MenuItem::with_id(
        app,
        "show-codex-widget",
        "Show Codex meter",
        true,
        None::<&str>,
    )?;
    #[cfg(target_os = "windows")]
    items.insert(0, &show_widget);
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

fn view_from_reading(
    provider: Provider,
    prefs: &crate::prefs::AppPrefs,
    now: u64,
    reading: &Reading,
    incoming: bool,
) -> ProviderView {
    let visible = prefs.is_visible(provider.key());
    let found = reading.connection != ConnectionState::CliNotFound;
    let present = visible && (provider.always_listed() || found);
    let windows = collect_windows(reading.rate_limits.as_ref());
    let selected = select_window(&windows, &prefs.tray_window(provider.key())).cloned();
    let seg = MeterSegment {
        present: present && selected.is_some(),
        remaining: selected.as_ref().map(remaining_percent).unwrap_or(0.0),
        resets_at: selected.as_ref().and_then(|window| window.resets_at),
        incoming: incoming && provider.follows_reset_radar(),
        stale: stale_age(reading.updated_at, now).is_some(),
    };
    ProviderView {
        provider,
        seg,
        windows,
        selected,
        updated_at: reading.updated_at,
        present,
    }
}

async fn all_provider_views(
    app: &AppHandle,
    prefs: &crate::prefs::AppPrefs,
    now: u64,
) -> Vec<ProviderView> {
    let incoming = app
        .try_state::<ResetRadar>()
        .is_some_and(|radar| radar.incoming_at(now));
    let Some(meters) = app.try_state::<Meters>() else {
        return Vec::new();
    };
    let mut views = Vec::with_capacity(Provider::ALL.len());
    for meter in meters.iter() {
        let reading = meter.reading().await;
        views.push(view_from_reading(
            meter.provider(),
            prefs,
            now,
            &reading,
            incoming,
        ));
    }
    views
}

/// Whether the combined item is the one on screen. It is whenever the user
/// picked the compact layout, and also when the per-tool layout would show
/// nothing at all (every tool hidden, or none found yet): a menu-bar app with
/// no menu-bar item cannot be opened, configured, or even quit.
fn shows_combined_item(prefs: &crate::prefs::AppPrefs, views: &[ProviderView]) -> bool {
    prefs.combined_tray || !views.iter().any(|view| view.present)
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
    let combined = shows_combined_item(&prefs, &views);

    set_visible(app, TRAY_ID, combined);
    for view in &views {
        set_visible(app, view.provider.tray_id(), !combined && view.present);
    }

    if combined {
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

    sync_menus(app, &prefs, &views, combined);
}

fn set_visible(app: &AppHandle, id: &'static str, visible: bool) {
    {
        let cache = app.state::<TrayRenderCache>();
        let mut map = cache
            .visible
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        let mut map = cache
            .labels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
            // Shell_NotifyIcon limits tooltip text to 127 UTF-16 code units.
            #[cfg(target_os = "windows")]
            let tooltip: String = {
                let mut units = 0;
                tooltip
                    .chars()
                    .take_while(|c| {
                        units += c.len_utf16();
                        units <= 127
                    })
                    .collect()
            };
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
        let mut map = cache
            .icons
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
fn sync_menus(
    app: &AppHandle,
    prefs: &crate::prefs::AppPrefs,
    views: &[ProviderView],
    combined: bool,
) {
    let windows_sig = |windows: &[TrayWindow]| {
        windows
            .iter()
            .map(|window| format!("{}={}", window.id, window.label))
            .collect::<Vec<_>>()
            .join(",")
    };
    // A downloaded-but-not-restarted update is part of the signature so the
    // "Check for Updates…" item becomes "Restart to Update" once one lands.
    let pending = app
        .state::<UpdateStatus>()
        .pending_version()
        .unwrap_or_default();
    let shared = format!("{}|{}|{}", prefs.usage_alerts, prefs.combined_tray, pending);
    let listed: Vec<&ProviderView> = views.iter().filter(|view| view.present).collect();

    if combined {
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
        rebuild_menu(app, TRAY_ID, sig, move |app| {
            build_unified_menu(app, &menu_views)
        });
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
    let label = window
        .map(|window| window.label.as_str())
        .unwrap_or("usage");
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
        // Icon-only title gives no hint why the bar is blank; the tooltip is
        // the cheapest place to say so and point at the popover's setup path.
        return "UsageBar — no tools found yet. Click to set up.".to_owned();
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

/// Refreshes every visible provider. Each runs on its own task, so one slow
/// or timing-out backend does not hold the others behind it.
pub fn refresh_all_providers(app: &AppHandle) {
    let prefs = app.state::<PrefsStore>().get();
    for provider in Provider::ALL {
        if prefs.is_visible(provider.key()) {
            refresh_provider(app, provider);
        }
    }
}

/// A background refresh of one provider, e.g. right after it is shown again.
pub fn refresh_provider(app: &AppHandle, provider: Provider) {
    let Some(meter) = app
        .try_state::<Meters>()
        .map(|meters| meters.get(provider).clone())
    else {
        return;
    };
    tauri::async_runtime::spawn(async move {
        meter.background_refresh().await;
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
        "check-updates" => {
            let handle = app.clone();
            tauri::async_runtime::spawn(async move {
                crate::updates::check_for_update(handle, true).await;
            });
        }
        "restart-update" => {
            tauri::process::restart(&app.env());
        }
        "star-github" => {
            let _ = crate::commands::open_in_browser(
                "https://github.com/abdelrahmanmagdii/usage-tracker",
            );
        }
        "show-onboarding" => {
            if let Some(window) = app.get_webview_window("main") {
                #[cfg(target_os = "macos")]
                crate::activate_app();
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
            let _ = app.emit("usagebar://show-onboarding", ());
        }
        "show-codex-widget" => {
            let _ = crate::commands::show_codex_widget(app);
        }
        "quit-app" => quit_from_menu(app),
        _ => {}
    }
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
            .tooltip(format!("{} usage", provider.display_name()))
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
        set_visible(&handle, Provider::Codex.tray_id(), true);
    }
    Ok(())
}

mod format;
mod icons;

#[allow(unused_imports)]
pub use format::*;
#[allow(unused_imports)]
pub use icons::*;

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
        let codex = MeterSegment {
            present: true,
            remaining: 62.0,
            resets_at: Some(9_000),
            incoming: false,
            stale: false,
        };
        let claude = MeterSegment {
            present: true,
            remaining: 8.0,
            resets_at: Some(9_000),
            incoming: false,
            stale: false,
        };
        // Two providers: percentages only, joined, no countdowns.
        assert_eq!(combined_title(&[codex, claude], 1_000), "62% · 8%");
        // Markers still attach to the right segment.
        let stale_claude = MeterSegment {
            stale: true,
            ..claude
        };
        assert_eq!(combined_title(&[codex, stale_claude], 1_000), "62% · ~8%");
        let incoming_codex = MeterSegment {
            incoming: true,
            ..codex
        };
        assert_eq!(
            combined_title(&[incoming_codex, claude], 1_000),
            "⚡ 62% · 8%"
        );
        let cursor = MeterSegment {
            present: true,
            remaining: 41.0,
            resets_at: None,
            incoming: false,
            stale: false,
        };
        assert_eq!(
            combined_title(&[codex, claude, cursor], 1_000),
            "62% · 8% · 41%"
        );
    }

    #[test]
    fn combined_title_keeps_the_countdown_for_a_lone_provider() {
        let codex = MeterSegment {
            present: true,
            remaining: 62.0,
            resets_at: Some(4_661),
            incoming: false,
            stale: false,
        };
        let absent = MeterSegment::default();
        // A lone provider has room for the countdown.
        assert_eq!(combined_title(&[codex, absent], 1_000), "62% · 1:01:01");
        // Nothing present yields an empty title.
        assert_eq!(combined_title(&[absent, absent], 1_000), "");
        // Claude-only (Codex still loading) shows just Claude.
        let claude = MeterSegment {
            present: true,
            remaining: 8.0,
            resets_at: None,
            incoming: false,
            stale: false,
        };
        assert_eq!(combined_title(&[absent, claude], 1_000), "8%");
    }

    #[test]
    fn incoming_reset_prefixes_the_title() {
        assert_eq!(
            with_incoming_prefix("42% · 9:05".into(), true),
            "⚡ 42% · 9:05"
        );
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
        assert_eq!(
            stale_age(Some(now - TRAY_STALE_AFTER_SECS), now),
            Some(TRAY_STALE_AFTER_SECS)
        );
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
    fn devin_icon_is_an_amber_diamond() {
        let icon = devin_tray_icon();
        let rgba = icon.rgba();
        let pixel_at = |x: usize, y: usize| &rgba[(y * 22 + x) * 4..(y * 22 + x) * 4 + 4];
        assert_eq!(pixel_at(0, 0)[3], 0);
        let center = pixel_at(11, 9);
        assert!(center[3] > 200);
        assert!(center[0] > center[1] && center[1] > center[2]);
        assert!(pixel_at(11, 3)[3] > 0);
        assert_eq!(pixel_at(21, 2)[3], 0);
    }

    #[test]
    fn antigravity_icon_is_a_blue_star() {
        let icon = antigravity_tray_icon();
        let rgba = icon.rgba();
        let pixel_at = |x: usize, y: usize| &rgba[(y * 22 + x) * 4..(y * 22 + x) * 4 + 4];
        assert_eq!(pixel_at(0, 0)[3], 0);
        let center = pixel_at(11, 9);
        assert!(center[3] > 200);
        assert!(center[2] > center[0] + 80);
        assert!(center[2] > center[1]);
        assert!(pixel_at(11, 3)[3] > 0);
        assert_eq!(pixel_at(21, 2)[3], 0);
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
        assert_eq!(
            combined_tray_icon(&[Provider::Devin]).rgba(),
            devin_tray_icon().rgba()
        );
        assert_eq!(
            combined_tray_icon(&[Provider::Antigravity]).rgba(),
            antigravity_tray_icon().rgba()
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

    fn reading(connection: ConnectionState) -> Reading {
        Reading {
            connection,
            rate_limits: Some(sample_payload()),
            updated_at: Some(1_000),
        }
    }

    #[test]
    fn codex_is_listed_before_it_is_found_and_other_tools_only_once_found() {
        let prefs = crate::prefs::AppPrefs::default();
        let missing = reading(ConnectionState::CliNotFound);
        let codex = view_from_reading(Provider::Codex, &prefs, 1_000, &missing, false);
        assert!(codex.present);
        let cursor = view_from_reading(Provider::Cursor, &prefs, 1_000, &missing, false);
        assert!(!cursor.present);
        assert!(!cursor.seg.present);
        let found = reading(ConnectionState::Connected);
        let cursor = view_from_reading(Provider::Cursor, &prefs, 1_000, &found, false);
        assert!(cursor.present && cursor.seg.present);
        // A tool the user hid stays off the menu bar whatever its state.
        let mut hidden = crate::prefs::AppPrefs::default();
        hidden.set_visible(Provider::Codex.key(), false);
        let codex = view_from_reading(Provider::Codex, &hidden, 1_000, &found, false);
        assert!(!codex.present && !codex.seg.present);
    }

    #[test]
    fn only_codex_carries_the_reset_radar_marker() {
        let prefs = crate::prefs::AppPrefs::default();
        let found = reading(ConnectionState::Connected);
        for provider in Provider::ALL {
            let view = view_from_reading(provider, &prefs, 1_000, &found, true);
            assert_eq!(
                view.seg.incoming,
                provider == Provider::Codex,
                "{provider:?}"
            );
        }
    }

    #[test]
    fn the_combined_item_stands_in_when_the_per_tool_layout_would_be_empty() {
        let mut prefs = crate::prefs::AppPrefs {
            combined_tray: false,
            ..Default::default()
        };
        let found = reading(ConnectionState::Connected);
        let views: Vec<ProviderView> = Provider::ALL
            .into_iter()
            .map(|provider| view_from_reading(provider, &prefs, 1_000, &found, false))
            .collect();
        assert!(!shows_combined_item(&prefs, &views));
        // Every tool hidden: without the fallback no menu-bar item would be
        // left to open the app, switch layouts, or quit.
        for provider in Provider::ALL {
            prefs.set_visible(provider.key(), false);
        }
        let views: Vec<ProviderView> = Provider::ALL
            .into_iter()
            .map(|provider| view_from_reading(provider, &prefs, 1_000, &found, false))
            .collect();
        assert!(shows_combined_item(&prefs, &views));
        assert!(shows_combined_item(&prefs, &[]));
        // The compact layout always uses the combined item.
        prefs.combined_tray = true;
        assert!(shows_combined_item(&prefs, &views));
    }
}
