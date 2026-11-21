//! Notification delivery. On macOS the permission and the post go through
//! `UNUserNotificationCenter` (see `native/notifications.m`), so "accepted"
//! means the system took the request. A banner can still be hidden by Focus.
//! Other platforms post through the notification plugin, which reports only
//! that the submission was accepted, and their permission stays `unknown`.

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Manager};

pub const MAX_TITLE_CHARS: usize = 160;
pub const MAX_BODY_CHARS: usize = 1200;

#[cfg(target_os = "macos")]
mod native {
    use std::os::raw::{c_char, c_int};

    extern "C" {
        pub fn ub_notification_status() -> c_int;
        pub fn ub_notification_request() -> c_int;
        pub fn ub_notification_post(title: *const c_char, body: *const c_char) -> c_int;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Permission {
    NotDetermined,
    Denied,
    Granted,
    Unavailable,
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    Test,
    Reset,
    Threshold,
    FreshWindow,
    Pace,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Test => "test",
            Source::Reset => "reset",
            Source::Threshold => "threshold",
            Source::FreshWindow => "freshWindow",
            Source::Pace => "pace",
        }
    }

    fn from_command(value: &str) -> Option<Source> {
        match value {
            "test" => Some(Source::Test),
            "reset" => Some(Source::Reset),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryError {
    pub source: &'static str,
    pub message: String,
    pub at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationStatus {
    pub permission: Permission,
    pub last_error: Option<DeliveryError>,
}

/// The latest outcome per source. Holds no title, body, or other content.
#[derive(Default)]
pub struct NotificationLog {
    last: Mutex<BTreeMap<Source, Result<(), DeliveryError>>>,
}

impl NotificationLog {
    fn record(&self, source: Source, outcome: &Result<(), String>) {
        let entry = match outcome {
            Ok(()) => Ok(()),
            Err(message) => Err(DeliveryError {
                source: source.as_str(),
                message: message.clone(),
                at: now_unix_seconds(),
            }),
        };
        if let Ok(mut last) = self.last.lock() {
            last.insert(source, entry);
        }
    }

    pub fn last_error(&self) -> Option<DeliveryError> {
        let last = self.last.lock().ok()?;
        last.values()
            .filter_map(|entry| entry.as_ref().err())
            .max_by_key(|error| error.at)
            .cloned()
    }
}

fn now_unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn validate(title: &str, body: &str) -> Result<(), String> {
    if title.contains('\0') || body.contains('\0') {
        return Err("Notification text cannot contain NUL characters.".into());
    }
    if title.trim().is_empty() {
        return Err("Notification title is empty.".into());
    }
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(format!(
            "Notification title is longer than {MAX_TITLE_CHARS} characters."
        ));
    }
    if body.chars().count() > MAX_BODY_CHARS {
        return Err(format!(
            "Notification body is longer than {MAX_BODY_CHARS} characters."
        ));
    }
    Ok(())
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn permission_from_code(code: i32) -> Result<Permission, String> {
    match code {
        0 => Ok(Permission::NotDetermined),
        1 => Ok(Permission::Denied),
        2 => Ok(Permission::Granted),
        3 => Ok(Permission::Unavailable),
        -1 => Err("macOS did not answer the notification permission request in time.".into()),
        other => Err(format!("Unexpected notification permission code {other}.")),
    }
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn post_result(code: i32) -> Result<(), String> {
    match code {
        0 => Ok(()),
        1 => Err("macOS rejected the notification text.".into()),
        2 => Err("Notifications are not allowed for UsageBar.".into()),
        3 => Err("macOS notifications are unavailable in this build.".into()),
        4 => Err("macOS did not accept the notification request.".into()),
        5 => Err("macOS did not answer the notification request in time.".into()),
        other => Err(format!("Unexpected notification result code {other}.")),
    }
}

#[cfg(target_os = "macos")]
pub async fn status() -> Result<Permission, String> {
    let code = tokio::task::spawn_blocking(|| unsafe { native::ub_notification_status() })
        .await
        .map_err(|error| error.to_string())?;
    permission_from_code(code)
}

#[cfg(not(target_os = "macos"))]
pub async fn status() -> Result<Permission, String> {
    Ok(Permission::Unknown)
}

#[cfg(target_os = "macos")]
pub async fn request() -> Result<Permission, String> {
    static ONE_PROMPT: Mutex<()> = Mutex::new(());
    let code = tokio::task::spawn_blocking(|| {
        let _guard = ONE_PROMPT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        unsafe { native::ub_notification_request() }
    })
    .await
    .map_err(|error| error.to_string())?;
    permission_from_code(code)
}

#[cfg(not(target_os = "macos"))]
pub async fn request() -> Result<Permission, String> {
    Ok(Permission::Unknown)
}

#[cfg(target_os = "macos")]
pub async fn post(_app: &AppHandle, title: &str, body: &str) -> Result<(), String> {
    validate(title, body)?;
    let title = std::ffi::CString::new(title).map_err(|error| error.to_string())?;
    let body = std::ffi::CString::new(body).map_err(|error| error.to_string())?;
    let code = tokio::task::spawn_blocking(move || unsafe {
        native::ub_notification_post(title.as_ptr(), body.as_ptr())
    })
    .await
    .map_err(|error| error.to_string())?;
    post_result(code)
}

#[cfg(not(target_os = "macos"))]
pub async fn post(app: &AppHandle, title: &str, body: &str) -> Result<(), String> {
    use tauri_plugin_notification::NotificationExt;
    validate(title, body)?;
    app.notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .map_err(|error| error.to_string())
}

fn blocked_message(permission: Permission) -> Option<&'static str> {
    match permission {
        Permission::Granted | Permission::Unknown => None,
        Permission::NotDetermined => Some("Notification permission has not been granted yet."),
        Permission::Denied => Some("Notifications are turned off for UsageBar."),
        Permission::Unavailable => Some("Notifications are unavailable in this build."),
    }
}

/// The one path every alert takes. It never asks for permission: only an
/// explicit user action does that. The outcome is kept for the settings sheet.
pub async fn dispatch(
    app: &AppHandle,
    source: Source,
    title: &str,
    body: &str,
) -> Result<(), String> {
    let outcome = deliver(app, title, body).await;
    if let Some(log) = app.try_state::<NotificationLog>() {
        log.record(source, &outcome);
    }
    outcome
}

async fn deliver(app: &AppHandle, title: &str, body: &str) -> Result<(), String> {
    validate(title, body)?;
    let permission = status().await?;
    if let Some(message) = blocked_message(permission) {
        return Err(message.into());
    }
    post(app, title, body).await
}

fn current_status(app: &AppHandle, permission: Permission) -> NotificationStatus {
    NotificationStatus {
        permission,
        last_error: app
            .try_state::<NotificationLog>()
            .and_then(|log| log.last_error()),
    }
}

#[tauri::command]
pub async fn get_notification_status(app: AppHandle) -> Result<NotificationStatus, String> {
    Ok(current_status(&app, status().await?))
}

#[tauri::command]
pub async fn request_notification_permission(app: AppHandle) -> Result<NotificationStatus, String> {
    Ok(current_status(&app, request().await?))
}

#[tauri::command]
pub async fn post_usage_notification(
    app: AppHandle,
    source: String,
    title: String,
    body: String,
) -> Result<(), String> {
    let source =
        Source::from_command(&source).ok_or_else(|| "Unknown notification source.".to_string())?;
    dispatch(&app, source, &title, &body).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_codes_map_to_permissions() {
        assert_eq!(permission_from_code(0), Ok(Permission::NotDetermined));
        assert_eq!(permission_from_code(1), Ok(Permission::Denied));
        assert_eq!(permission_from_code(2), Ok(Permission::Granted));
        assert_eq!(permission_from_code(3), Ok(Permission::Unavailable));
        assert!(permission_from_code(-1).is_err());
        assert!(permission_from_code(9).is_err());
    }

    #[test]
    fn only_a_zero_post_code_is_success() {
        assert_eq!(post_result(0), Ok(()));
        for code in [1, 2, 3, 4, 5, -1, 42] {
            assert!(post_result(code).is_err(), "code {code}");
        }
    }

    #[test]
    fn status_serializes_in_the_frontend_shape() {
        let status = NotificationStatus {
            permission: Permission::NotDetermined,
            last_error: Some(DeliveryError {
                source: "freshWindow",
                message: "no".into(),
                at: 7,
            }),
        };
        assert_eq!(
            serde_json::to_value(&status).unwrap(),
            serde_json::json!({
                "permission": "notDetermined",
                "lastError": { "source": "freshWindow", "message": "no", "at": 7 }
            })
        );
        let clean = NotificationStatus {
            permission: Permission::Unknown,
            last_error: None,
        };
        assert_eq!(
            serde_json::to_value(&clean).unwrap(),
            serde_json::json!({ "permission": "unknown", "lastError": null })
        );
    }

    #[test]
    fn text_is_bounded_and_rejects_nul() {
        assert!(validate("Title", "Body").is_ok());
        assert!(validate(&"t".repeat(MAX_TITLE_CHARS), &"b".repeat(MAX_BODY_CHARS)).is_ok());
        assert!(validate(&"t".repeat(MAX_TITLE_CHARS + 1), "b").is_err());
        assert!(validate("t", &"b".repeat(MAX_BODY_CHARS + 1)).is_err());
        assert!(validate("a\0b", "b").is_err());
        assert!(validate("t", "a\0b").is_err());
        assert!(validate("  ", "b").is_err());
    }

    #[test]
    fn only_granted_or_unknown_permission_may_post() {
        assert!(blocked_message(Permission::Granted).is_none());
        assert!(blocked_message(Permission::Unknown).is_none());
        for permission in [
            Permission::NotDetermined,
            Permission::Denied,
            Permission::Unavailable,
        ] {
            assert!(blocked_message(permission).is_some());
        }
    }

    #[test]
    fn the_log_surfaces_the_latest_failure_until_that_source_succeeds() {
        let log = NotificationLog::default();
        assert!(log.last_error().is_none());
        log.record(Source::Pace, &Err("denied".into()));
        assert_eq!(log.last_error().unwrap().source, "pace");
        log.record(Source::Test, &Ok(()));
        assert_eq!(log.last_error().unwrap().source, "pace");
        log.record(Source::Pace, &Ok(()));
        assert!(log.last_error().is_none());
    }

    #[test]
    fn commands_accept_only_test_and_reset_sources() {
        assert_eq!(Source::from_command("test"), Some(Source::Test));
        assert_eq!(Source::from_command("reset"), Some(Source::Reset));
        assert_eq!(Source::from_command("pace"), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_non_bundle_process_reports_unavailable_without_prompting() {
        unsafe {
            assert_eq!(native::ub_notification_status(), 3);
            assert_eq!(native::ub_notification_request(), 3);
            let (title, body) = (c"t", c"b");
            assert_eq!(
                native::ub_notification_post(title.as_ptr(), body.as_ptr()),
                3
            );
            assert_eq!(
                native::ub_notification_post(std::ptr::null(), body.as_ptr()),
                1
            );
        }
    }
}
