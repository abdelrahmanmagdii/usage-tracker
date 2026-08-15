use std::{fs, path::PathBuf, sync::Mutex};

use serde::{Deserialize, Serialize};
use tauri::{
    App, AppHandle, LogicalPosition, LogicalSize, Manager, WebviewUrl, WebviewWindowBuilder,
};

pub const NOTCH_WINDOW_LABEL: &str = "notch";
pub const NOTCH_WIDTH: f64 = 304.0;
pub const NOTCH_COLLAPSED_HEIGHT: f64 = 48.0;
pub const NOTCH_EXPANDED_HEIGHT: f64 = 94.0;

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NotchMode {
    /// Off by default: the notch companion is opt-in from the popover.
    #[default]
    Off,
    Automatic,
    Always,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotchStatus {
    pub mode: NotchMode,
    pub notch_available: bool,
    pub visible: bool,
}

#[derive(Debug, Clone, Copy)]
struct NotchLayout {
    x: f64,
    y: f64,
    has_notch: bool,
}

pub struct NotchController {
    mode: Mutex<NotchMode>,
    status: Mutex<NotchStatus>,
    preference_path: PathBuf,
}

impl NotchController {
    fn new(preference_path: PathBuf) -> Self {
        let mode = fs::read_to_string(&preference_path)
            .ok()
            .and_then(|value| serde_json::from_str::<NotchMode>(&value).ok())
            .unwrap_or_default();
        Self {
            mode: Mutex::new(mode),
            status: Mutex::new(NotchStatus {
                mode,
                notch_available: false,
                visible: false,
            }),
            preference_path,
        }
    }

    pub fn status(&self) -> NotchStatus {
        self.status.lock().expect("notch status poisoned").clone()
    }

    pub fn set_mode(&self, mode: NotchMode) -> Result<(), String> {
        *self
            .mode
            .lock()
            .map_err(|_| "Notch preference is unavailable")? = mode;
        self.status
            .lock()
            .map_err(|_| "Notch status is unavailable")?
            .mode = mode;
        if let Some(parent) = self.preference_path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::write(
            &self.preference_path,
            serde_json::to_vec(&mode).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())
    }

    fn mode(&self) -> NotchMode {
        *self.mode.lock().expect("notch preference poisoned")
    }

    fn update_status(&self, notch_available: bool, visible: bool) {
        *self.status.lock().expect("notch status poisoned") = NotchStatus {
            mode: self.mode(),
            notch_available,
            visible,
        };
    }
}

pub fn setup(app: &App) -> tauri::Result<()> {
    let preference_path = app.path().app_config_dir()?.join("notch-mode.json");
    app.manage(NotchController::new(preference_path));
    sync_window(app.handle())?;
    Ok(())
}

pub fn schedule_sync(app: AppHandle) -> Result<(), String> {
    app.clone()
        .run_on_main_thread(move || {
            if let Err(error) = sync_window(&app) {
                eprintln!("usagebar: unable to update notch window: {error}");
            }
        })
        .map_err(|error| error.to_string())
}

fn sync_window(app: &AppHandle) -> tauri::Result<()> {
    let controller = app.state::<NotchController>();
    let mode = controller.mode();
    let layout = platform_layout(mode);
    let notch_available = layout.is_some_and(|value| value.has_notch);
    let should_show = mode != NotchMode::Off
        && layout.is_some()
        && (mode == NotchMode::Always || notch_available);

    if !should_show {
        if let Some(window) = app.get_webview_window(NOTCH_WINDOW_LABEL) {
            window.destroy()?;
        }
        controller.update_status(notch_available, false);
        return Ok(());
    }

    let layout = layout.expect("visible notch window has a layout");
    let window = if let Some(window) = app.get_webview_window(NOTCH_WINDOW_LABEL) {
        window
    } else {
        WebviewWindowBuilder::new(
            app,
            NOTCH_WINDOW_LABEL,
            WebviewUrl::App("index.html?view=notch".into()),
        )
        .title("UsageBar — Notch")
        .inner_size(NOTCH_WIDTH, NOTCH_COLLAPSED_HEIGHT)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .resizable(false)
        .minimizable(false)
        .maximizable(false)
        .closable(false)
        .focused(false)
        .focusable(true)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .skip_taskbar(true)
        .visible(false)
        .build()?
    };

    window.set_size(LogicalSize::new(NOTCH_WIDTH, NOTCH_COLLAPSED_HEIGHT))?;
    window.set_position(LogicalPosition::new(layout.x, layout.y))?;
    configure_native_window(&window);
    window.show()?;
    controller.update_status(notch_available, true);
    Ok(())
}

#[cfg(target_os = "macos")]
fn platform_layout(mode: NotchMode) -> Option<NotchLayout> {
    use objc2::{runtime::NSObjectProtocol, sel, MainThreadMarker};
    use objc2_app_kit::NSScreen;

    let mtm = MainThreadMarker::new()?;
    let screens = NSScreen::screens(mtm);
    if screens.count() == 0 {
        return None;
    }
    let primary = screens.objectAtIndex(0);
    let primary_top = primary.frame().origin.y + primary.frame().size.height;
    let mut fallback = None;

    for index in 0..screens.count() {
        let screen = screens.objectAtIndex(index);
        let frame = screen.frame();
        let supports_safe_area = screen.respondsToSelector(sel!(safeAreaInsets));
        let top_inset = if supports_safe_area {
            screen.safeAreaInsets().top
        } else {
            0.0
        };
        let has_notch = top_inset > 0.5;
        let x = frame.origin.x + (frame.size.width - NOTCH_WIDTH) / 2.0;
        let screen_top = frame.origin.y + frame.size.height;
        let y = primary_top - screen_top + if has_notch { top_inset } else { 6.0 };
        let layout = NotchLayout { x, y, has_notch };
        if has_notch {
            return Some(layout);
        }
        fallback.get_or_insert(layout);
    }

    if mode == NotchMode::Always {
        fallback
    } else {
        None
    }
}

#[cfg(not(target_os = "macos"))]
fn platform_layout(_mode: NotchMode) -> Option<NotchLayout> {
    None
}

#[cfg(target_os = "macos")]
fn configure_native_window(window: &tauri::WebviewWindow) {
    use objc2_app_kit::{NSStatusWindowLevel, NSWindow, NSWindowCollectionBehavior};

    let Ok(pointer) = window.ns_window() else {
        return;
    };
    // SAFETY: Tauri returns the live NSWindow backing this webview, and this
    // function only runs on the macOS main thread while the window is retained.
    let native = unsafe { &*(pointer.cast::<NSWindow>()) };
    native.setLevel(NSStatusWindowLevel);
    native.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
}

#[cfg(not(target_os = "macos"))]
fn configure_native_window(_window: &tauri::WebviewWindow) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_is_the_default_mode() {
        assert_eq!(NotchMode::default(), NotchMode::Off);
    }

    #[test]
    fn modes_use_stable_lowercase_storage_values() {
        assert_eq!(serde_json::to_string(&NotchMode::Off).unwrap(), "\"off\"");
        assert_eq!(
            serde_json::to_string(&NotchMode::Automatic).unwrap(),
            "\"automatic\""
        );
        assert_eq!(
            serde_json::to_string(&NotchMode::Always).unwrap(),
            "\"always\""
        );
    }
}
