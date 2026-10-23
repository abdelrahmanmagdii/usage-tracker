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

    let right_x = mi.rc_work.right - width;
    let bottom_y = mi.rc_work.bottom - height;
    let (target_x, target_y) = if manual {
        (
            window_rect.left.clamp(mi.rc_work.left, right_x),
            window_rect.top.clamp(mi.rc_work.top, bottom_y),
        )
    } else {
        (right_x, bottom_y)
    };

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
