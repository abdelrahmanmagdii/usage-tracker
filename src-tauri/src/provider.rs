//! Shared snapshot type and helpers for optional usage providers (Claude,
//! Cursor, OpenCode Go, Devin, Antigravity). Each backend still owns its own auth and fetch,
//! but they all emit this shape so the tray and renderer stay generic.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{json, Map, Value};
use tokio::time::Duration;

use crate::codex::process::ConnectionState;

/// A Security.framework status that means this Mac has no usable login.
///
/// `-25300` is `errSecItemNotFound`.
/// `-128` is `errSecUserCanceled`. A background poll disables the keychain
/// sheet, and macOS reports that disabled sheet as a cancel.
/// `-25308` is `errSecInteractionNotAllowed`.
///
/// Only the macOS keychain paths call this (Claude and Antigravity); tests
/// still compile it everywhere so they run on every platform's CI.
#[cfg(any(target_os = "macos", test))]
pub fn keychain_login_absent(code: i32) -> bool {
    matches!(code, -25300 | -128 | -25308)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderState {
    pub connection: ConnectionState,
    pub diagnostic: Option<String>,
    pub account: Option<Value>,
    pub rate_limits: Option<Value>,
    pub updated_at: Option<u64>,
}

impl Default for ProviderState {
    fn default() -> Self {
        Self {
            connection: ConnectionState::Starting,
            diagnostic: None,
            account: None,
            rate_limits: None,
            updated_at: None,
        }
    }
}

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(concat!("usagebar/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("reqwest client construction is infallible with these options")
}

pub fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// A reset (or expiry) time as unix seconds, from whichever form a vendor
/// sends: a number or numeric string in seconds or milliseconds (Connect /
/// protobuf JSON sends int64 epochs as strings, often in milliseconds), or an
/// RFC3339 string. Anything past 10^10 is read as milliseconds, since that is
/// the year 2286 in seconds. Zero, negatives and non-finite values are "not
/// reported" (protobuf's unset), never 1970.
pub fn parse_reset_timestamp(value: Option<&Value>) -> Option<f64> {
    let value = value?;
    let seconds = match finite_f64(Some(value)) {
        Some(number) if number > 10_000_000_000.0 => number / 1_000.0,
        Some(number) => number,
        None => {
            let text = value.as_str()?;
            time::OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339)
                .ok()?
                .unix_timestamp() as f64
        }
    };
    (seconds > 0.0).then_some(seconds)
}

pub fn window_snapshot(
    used_percent: f64,
    duration_mins: Option<f64>,
    resets_at: Option<f64>,
) -> Value {
    let mut window = Map::new();
    window.insert(
        "usedPercent".into(),
        Value::from(used_percent.clamp(0.0, 100.0)),
    );
    if let Some(duration) = duration_mins {
        window.insert("windowDurationMins".into(), Value::from(duration));
    }
    if let Some(resets_at) = resets_at {
        window.insert("resetsAt".into(), Value::from(resets_at));
    }
    Value::Object(window)
}

pub fn rate_limits_map(entries: Vec<(String, Value)>) -> Value {
    let mut by_id = Map::new();
    for (id, snapshot) in entries {
        by_id.insert(id, snapshot);
    }
    json!({ "rateLimitsByLimitId": Value::Object(by_id) })
}

/// An unreadable credential store keeps the last meter only while that meter
/// is still on screen. A provider already marked missing stays hidden, so a
/// later Keychain failure cannot bring it back as a status error.
pub fn keep_last_meter(connection: ConnectionState, has_shown_usage: bool) -> bool {
    connection != ConnectionState::CliNotFound && has_shown_usage
}

/// Mark a provider missing and drop the numbers that would otherwise make the
/// next failed read look like a live meter.
pub fn conceal_provider(state: &mut ProviderState, diagnostic: Option<String>) {
    state.connection = ConnectionState::CliNotFound;
    state.diagnostic = diagnostic;
    state.account = None;
    state.rate_limits = None;
    state.updated_at = None;
}

/// A number from a JSON number or numeric string. `str::parse` also accepts
/// "NaN" and "inf", which would reach the renderer as a null percentage, so
/// only finite values count.
pub fn finite_f64(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(number) => number.as_f64(),
        Value::String(text) if !text.trim().is_empty() => text.trim().parse().ok(),
        _ => None,
    }
    .filter(|number: &f64| number.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_hidden_provider_stays_hidden_when_the_store_cannot_be_read() {
        assert!(!keep_last_meter(ConnectionState::CliNotFound, true));
        assert!(!keep_last_meter(ConnectionState::CliNotFound, false));
        assert!(!keep_last_meter(ConnectionState::Connected, false));
        assert!(keep_last_meter(ConnectionState::Connected, true));
        assert!(keep_last_meter(ConnectionState::Error, true));
    }

    #[test]
    fn only_missing_or_unreachable_keychain_items_count_as_signed_out() {
        assert!(keychain_login_absent(-25300));
        assert!(keychain_login_absent(-128));
        assert!(keychain_login_absent(-25308));
        // errSecAuthFailed and success are not "no login".
        assert!(!keychain_login_absent(-25293));
        assert!(!keychain_login_absent(0));
    }

    #[test]
    fn reset_timestamps_accept_every_vendor_form() {
        let at = 1_788_220_800.0;
        assert_eq!(parse_reset_timestamp(Some(&json!(1_788_220_800))), Some(at));
        assert_eq!(
            parse_reset_timestamp(Some(&json!(1_788_220_800_000_i64))),
            Some(at)
        );
        assert_eq!(
            parse_reset_timestamp(Some(&json!("1788220800000"))),
            Some(at)
        );
        assert_eq!(parse_reset_timestamp(Some(&json!("1788220800"))), Some(at));
        assert_eq!(
            parse_reset_timestamp(Some(&json!("2026-09-01T00:00:00.000Z"))),
            Some(at)
        );
        assert_eq!(
            parse_reset_timestamp(Some(&json!(1_788_220_800.5))),
            Some(at + 0.5)
        );
        // Unset, nonsense and non-finite values are "not reported".
        assert_eq!(parse_reset_timestamp(Some(&json!(0))), None);
        assert_eq!(parse_reset_timestamp(Some(&json!("0"))), None);
        assert_eq!(parse_reset_timestamp(Some(&json!(-5))), None);
        assert_eq!(parse_reset_timestamp(Some(&json!("NaN"))), None);
        assert_eq!(parse_reset_timestamp(Some(&json!("soon"))), None);
        assert_eq!(parse_reset_timestamp(Some(&Value::Null)), None);
        assert_eq!(parse_reset_timestamp(None), None);
    }

    #[test]
    fn only_finite_numbers_count() {
        assert_eq!(finite_f64(Some(&json!(41.5))), Some(41.5));
        assert_eq!(finite_f64(Some(&json!(" 12 "))), Some(12.0));
        assert_eq!(finite_f64(Some(&json!("NaN"))), None);
        assert_eq!(finite_f64(Some(&json!("inf"))), None);
        assert_eq!(finite_f64(Some(&json!("-infinity"))), None);
        assert_eq!(finite_f64(Some(&json!(""))), None);
        assert_eq!(finite_f64(Some(&json!(true))), None);
    }

    #[test]
    fn concealing_a_provider_drops_the_old_meter() {
        let mut state = ProviderState {
            connection: ConnectionState::Connected,
            diagnostic: Some("status -25293".into()),
            account: Some(json!({ "type": "oauth" })),
            rate_limits: Some(json!({ "rateLimitsByLimitId": {} })),
            updated_at: Some(1_700_000_000),
        };
        conceal_provider(&mut state, Some("No login was found on this Mac".into()));
        assert_eq!(state.connection, ConnectionState::CliNotFound);
        assert!(state.account.is_none());
        assert!(state.rate_limits.is_none());
        assert!(state.updated_at.is_none());
        assert!(!keep_last_meter(
            state.connection,
            state.updated_at.is_some()
        ));
    }
}
