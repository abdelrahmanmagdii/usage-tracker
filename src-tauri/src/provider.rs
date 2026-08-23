//! Shared snapshot type and helpers for optional usage providers (Claude,
//! Cursor, OpenCode Go). Each backend still owns its own auth and fetch, but
//! they all emit this shape so the tray and renderer stay generic.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{json, Map, Value};
use tokio::time::Duration;

use crate::codex::process::ConnectionState;

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

pub fn parse_reset_timestamp(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => {
            let parsed = time::OffsetDateTime::parse(
                text,
                &time::format_description::well_known::Rfc3339,
            )
            .ok()?;
            Some(parsed.unix_timestamp() as f64)
        }
        _ => None,
    }
}

pub fn window_snapshot(used_percent: f64, duration_mins: Option<f64>, resets_at: Option<f64>) -> Value {
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

pub fn finite_f64(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(number) => number.as_f64(),
        Value::String(text) if !text.trim().is_empty() => text.parse().ok(),
        _ => None,
    }
}
