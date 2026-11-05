use std::path::PathBuf;

#[derive(Default)]
pub struct WidgetPlacement {
    pub manually_moved: std::sync::atomic::AtomicBool,
}

#[cfg(target_os = "windows")]
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
struct RECT {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct MONITORINFO {
    cb_size: u32,
    rc_monitor: RECT,
    rc_work: RECT,
    dw_flags: u32,
}

#[cfg(target_os = "windows")]
#[link(name = "user32")]
extern "system" {
    fn MonitorFromWindow(hwnd: *mut std::ffi::c_void, dw_flags: u32) -> *mut std::ffi::c_void;
    fn GetMonitorInfoW(hmonitor: *mut std::ffi::c_void, lpmi: *mut MONITORINFO) -> i32;
    fn GetWindowRect(hwnd: *mut std::ffi::c_void, lp_rect: *mut RECT) -> i32;
    fn SetWindowPos(
        hwnd: *mut std::ffi::c_void,
        hwnd_insert_after: *mut std::ffi::c_void,
        x: i32,
        y: i32,
        cx: i32,
        cy: i32,
        u_flags: u32,
    ) -> i32;
}

/// Dock flush to the taskbar until dragged; then only keep the meter on-screen.
#[cfg(target_os = "windows")]
pub fn position_codex_widget(app: &tauri::AppHandle) -> tauri::Result<()> {
    use tauri::Manager;
    let Some(window) = app.get_webview_window("codex-widget") else {
        return Ok(());
    };
    let hwnd = window.hwnd()?;
    let raw_hwnd = hwnd.0 as *mut std::ffi::c_void;

    let manual = app
        .state::<WidgetPlacement>()
        .manually_moved
        .load(std::sync::atomic::Ordering::Relaxed);

    let mut window_rect = RECT::default();
    if unsafe { GetWindowRect(raw_hwnd, &mut window_rect) } == 0 {
        return Ok(());
    }
    let width = window_rect.right - window_rect.left;
    let height = window_rect.bottom - window_rect.top;
    if width <= 0 || height <= 0 {
        return Ok(());
    }

    // 1 = MONITOR_DEFAULTTOPRIMARY, 2 = MONITOR_DEFAULTTONEAREST
    let flags = if manual { 2 } else { 1 };
    let hmon = unsafe { MonitorFromWindow(raw_hwnd, flags) };
    if hmon.is_null() {
        return Ok(());
    }

    let mut mi = MONITORINFO {
        cb_size: std::mem::size_of::<MONITORINFO>() as u32,
        rc_monitor: RECT::default(),
        rc_work: RECT::default(),
        dw_flags: 0,
    };
    if unsafe { GetMonitorInfoW(hmon, &mut mi) } == 0 {
        return Ok(());
    }

    let (target_x, target_y) = widget_origin(
        (window_rect.left, window_rect.top, width, height),
        (
            mi.rc_work.left,
            mi.rc_work.top,
            mi.rc_work.right,
            mi.rc_work.bottom,
        ),
        manual,
    );

    const HWND_TOPMOST: *mut std::ffi::c_void = -1isize as *mut std::ffi::c_void;
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOACTIVATE: u32 = 0x0010;
    unsafe {
        SetWindowPos(
            raw_hwnd,
            HWND_TOPMOST,
            target_x,
            target_y,
            0,
            0,
            SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
    Ok(())
}

/// Where the widget's top-left corner goes inside a monitor work area
/// (`left, top, right, bottom`). Docked, it sits flush with the bottom-right
/// corner; once dragged, it stays put but is pulled back on-screen.
///
/// `i32::clamp` panics when its bounds cross, which they do whenever the widget
/// is wider or taller than the work area (a small or heavily scaled display, or
/// a tall taskbar). Release builds abort on panic, so that would crash the app;
/// prefer the work area's top-left edge instead.
#[cfg(any(target_os = "windows", test))]
fn widget_origin(
    (left, top, width, height): (i32, i32, i32, i32),
    (work_left, work_top, work_right, work_bottom): (i32, i32, i32, i32),
    manual: bool,
) -> (i32, i32) {
    let right_x = (work_right - width).max(work_left);
    let bottom_y = (work_bottom - height).max(work_top);
    if manual {
        (
            left.clamp(work_left, right_x),
            top.clamp(work_top, bottom_y),
        )
    } else {
        (right_x, bottom_y)
    }
}

/// Native Windows tools store their dot-directories under USERPROFILE.
pub fn home_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    let names = ["USERPROFILE", "HOME"];
    #[cfg(not(target_os = "windows"))]
    let names = ["HOME", "USERPROFILE"];
    names.into_iter().find_map(|name| {
        std::env::var_os(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: (i32, i32, i32, i32) = (0, 0, 1920, 1040);

    #[test]
    fn a_docked_widget_sits_in_the_bottom_right_corner() {
        assert_eq!(widget_origin((10, 10, 300, 120), WORK, false), (1620, 920));
    }

    #[test]
    fn a_dragged_widget_stays_put_but_is_kept_on_screen() {
        assert_eq!(widget_origin((400, 300, 300, 120), WORK, true), (400, 300));
        assert_eq!(
            widget_origin((1900, 1030, 300, 120), WORK, true),
            (1620, 920)
        );
        assert_eq!(widget_origin((-50, -20, 300, 120), WORK, true), (0, 0));
        // A secondary monitor to the left of the primary one.
        assert_eq!(
            widget_origin((-2000, 500, 300, 120), (-1920, 0, 0, 1040), true),
            (-1920, 500)
        );
    }

    #[test]
    fn a_widget_larger_than_the_work_area_does_not_panic() {
        // 640x400 work area, 800x500 widget: the clamp bounds would cross.
        let small = (0, 0, 640, 400);
        assert_eq!(widget_origin((100, 100, 800, 500), small, true), (0, 0));
        assert_eq!(widget_origin((100, 100, 800, 500), small, false), (0, 0));
    }
}
