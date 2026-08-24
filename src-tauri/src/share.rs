//! Open https links and the macOS share picker so quota cards can go to X and LinkedIn.

use tauri::{AppHandle, Manager};

pub fn open_url(url: &str) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("Only http(s) URLs can be opened".into());
    }
    std::process::Command::new("/usr/bin/open")
        .arg(url)
        .status()
        .map_err(|error| error.to_string())
        .and_then(|status| {
            if status.success() {
                Ok(())
            } else {
                Err("macOS could not open that link".into())
            }
        })
}

pub fn present_share_sheet(app: AppHandle, png: Vec<u8>, caption: String) -> Result<(), String> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, png, caption);
        return Err("The system share sheet is only available on macOS".into());
    }

    #[cfg(target_os = "macos")]
    {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let app_for_thread = app.clone();
        app.run_on_main_thread(move || {
            let result = present_share_sheet_on_main(&app_for_thread, png, &caption);
            let _ = tx.send(result);
        })
        .map_err(|error| error.to_string())?;
        rx.recv().map_err(|error| error.to_string())?
    }
}

#[cfg(target_os = "macos")]
fn present_share_sheet_on_main(app: &AppHandle, png: Vec<u8>, caption: &str) -> Result<(), String> {
    use std::cell::RefCell;

    use objc2::AnyThread;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_app_kit::{NSImage, NSSharingServicePicker, NSWindow};
    use objc2_foundation::{NSArray, NSData, NSRectEdge};

    thread_local! {
        static SHARE_PICKER: RefCell<Option<Retained<NSSharingServicePicker>>> = const { RefCell::new(None) };
    }

    let window = app
        .get_webview_window("main")
        .ok_or("UsageBar's window is not available")?;
    let ns_window = window.ns_window().map_err(|error| error.to_string())? as *const NSWindow;
    if ns_window.is_null() {
        return Err("Could not reach the macOS window for the share sheet".into());
    }
    let ns_window = unsafe { &*ns_window };
    let view = ns_window
        .contentView()
        .ok_or("The window has no content view to attach the share sheet to")?;

    let data = NSData::with_bytes(&png);
    let image = NSImage::initWithData(NSImage::alloc(), &data)
        .ok_or("The share card PNG could not be decoded")?;
    let object: Retained<AnyObject> = Retained::into_super(image).into();
    let items = NSArray::from_retained_slice(&[object]);
    let picker = unsafe { NSSharingServicePicker::initWithItems(NSSharingServicePicker::alloc(), &items) };
    SHARE_PICKER.with(|slot| {
        *slot.borrow_mut() = Some(picker.clone());
    });
    let _ = caption;
    let bounds = view.bounds();
    picker.showRelativeToRect_ofView_preferredEdge(bounds, &view, NSRectEdge::MaxY);
    Ok(())
}
