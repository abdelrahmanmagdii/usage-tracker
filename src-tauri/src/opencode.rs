//! OpenCode Go usage provider.
//!
//! OpenCode Go is the subscription plan with 5-hour / weekly / monthly quota
//! windows. The official endpoint is `GET /zen/go/v1/usage`, authenticated with
//! the API key OpenCode already stores in `auth.json`. Zen (prepaid balance)
//! is intentionally not tracked — it has no comparable quota API.
//!
//! Access is read-only: the key is never written or sent anywhere except
//! `opencode.ai`.

use std::sync::Arc;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{Mutex, RwLock};

use crate::codex::process::ConnectionState;
use crate::provider::{
    finite_f64, http_client, now_unix_seconds, parse_reset_timestamp, rate_limits_map,
    window_snapshot, ProviderState,
};
use crate::tray;

const USAGE_URL: &str = "https://opencode.ai/zen/go/v1/usage";
/// Catalog ids OpenCode uses for the Go subscription in `auth.json`.
const GO_PROVIDER_IDS: &[&str] = &["opencode-go", "opencode.go"];

#[derive(Clone)]
pub struct OpenCodeManager {
    app: AppHandle,
    state: Arc<RwLock<ProviderState>>,
    client: reqwest::Client,
    refresh_lock: Arc<Mutex<()>>,
}

impl OpenCodeManager {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            state: Arc::new(RwLock::new(ProviderState::default())),
            client: http_client(),
            refresh_lock: Arc::new(Mutex::new(())),
        }
    }

    pub async fn snapshot(&self) -> ProviderState {
        self.state.read().await.clone()
    }

    pub async fn refresh(&self) -> Result<ProviderState, String> {
        if !self
            .app
            .state::<crate::prefs::PrefsStore>()
            .get()
            .is_visible(crate::prefs::PROVIDER_OPENCODE)
        {
            return Ok(self.snapshot().await);
        }

        let _guard = self.refresh_lock.lock().await;
        let key = match load_go_key().await {
            CredentialRead::Found(key) => key,
            CredentialRead::Absent => {
                self.set_connection(
                    ConnectionState::CliNotFound,
                    Some("No OpenCode Go login was found on this Mac".into()),
                )
                .await;
                return Ok(self.snapshot().await);
            }
            CredentialRead::Unavailable(message) => {
                self.set_connection(ConnectionState::Error, Some(message.clone()))
                    .await;
                return Err(message);
            }
        };

        let response = self
            .client
            .get(USAGE_URL)
            .bearer_auth(&key)
            .send()
            .await
            .map_err(|error| format!("OpenCode Go usage request failed: {error}"));
        let response = match response {
            Ok(response) => response,
            Err(message) => {
                self.set_connection(ConnectionState::Error, Some(message.clone()))
                    .await;
                return Err(message);
            }
        };

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            self.set_connection(
                ConnectionState::NotAuthenticated,
                Some("OpenCode rejected the stored Go key. Sign in again with `/connect` and choose OpenCode Go.".into()),
            )
            .await;
            return Ok(self.snapshot().await);
        }
        if status == reqwest::StatusCode::NOT_FOUND {
            self.set_connection(
                ConnectionState::CliNotFound,
                Some("This OpenCode login is not an OpenCode Go subscription.".into()),
            )
            .await;
            return Ok(self.snapshot().await);
        }
        if !status.is_success() {
            let message = format!("OpenCode Go usage endpoint returned HTTP {status}");
            self.set_connection(ConnectionState::Error, Some(message.clone()))
                .await;
            return Err(message);
        }

        let payload: Value = match response.json().await {
            Ok(payload) => payload,
            Err(error) => {
                let message = format!("OpenCode Go usage response was not valid JSON: {error}");
                self.set_connection(ConnectionState::Error, Some(message.clone()))
                    .await;
                return Err(message);
            }
        };

        let Some(rate_limits) = normalize_usage(&payload) else {
            let message = "OpenCode Go did not report any quota windows".to_owned();
            self.set_connection(ConnectionState::Error, Some(message.clone()))
                .await;
            return Err(message);
        };

        {
            let mut state = self.state.write().await;
            state.connection = ConnectionState::Connected;
            state.diagnostic = None;
            state.account = Some(json!({ "type": "api", "planType": "go" }));
            state.rate_limits = Some(rate_limits.clone());
            state.updated_at = Some(now_unix_seconds());
        }
        if let Some(alerts) = self.app.try_state::<crate::alerts::UsageAlerts>() {
            alerts.observe(&self.app, "OpenCode Go", &rate_limits).await;
        }
        self.emit_state().await;
        Ok(self.snapshot().await)
    }

    async fn set_connection(&self, connection: ConnectionState, diagnostic: Option<String>) {
        {
            let mut state = self.state.write().await;
            state.connection = connection;
            state.diagnostic = diagnostic;
        }
        self.emit_state().await;
    }

    async fn emit_state(&self) {
        let state = self.snapshot().await;
        let _ = self.app.emit("opencode://state", state);
        tray::refresh_unified_tray(&self.app).await;
    }
}

enum CredentialRead {
    Found(String),
    Absent,
    Unavailable(String),
}

async fn load_go_key() -> CredentialRead {
    let Some(path) = auth_json_path() else {
        return CredentialRead::Unavailable(
            "HOME is not set, so OpenCode's auth.json could not be located".to_owned(),
        );
    };
    match tokio::fs::read_to_string(&path).await {
        Ok(contents) => match parse_go_key(&contents) {
            Some(key) => CredentialRead::Found(key),
            None => CredentialRead::Absent,
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => CredentialRead::Absent,
        Err(error) => CredentialRead::Unavailable(format!("Could not read {}: {error}", path.display())),
    }
}

fn auth_json_path() -> Option<std::path::PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        return Some(std::path::PathBuf::from(xdg).join("opencode/auth.json"));
    }
    let home = std::env::var_os("HOME")?;
    Some(std::path::PathBuf::from(home).join(".local/share/opencode/auth.json"))
}

/// Pulls the OpenCode Go API key and ignores Zen (`opencode`) entries.
fn parse_go_key(raw: &str) -> Option<String> {
    let value: Value = serde_json::from_str(raw.trim()).ok()?;
    let object = value.as_object()?;
    for id in GO_PROVIDER_IDS {
        if let Some(entry) = object.get(*id) {
            if let Some(key) = key_from_entry(entry) {
                return Some(key);
            }
        }
    }
    // Some builds store Go under a key that merely contains "go".
    for (id, entry) in object {
        let lower = id.to_ascii_lowercase();
        if lower.contains("go") && !lower.contains("google") && !lower.contains("golang") {
            if let Some(key) = key_from_entry(entry) {
                return Some(key);
            }
        }
    }
    None
}

fn key_from_entry(entry: &Value) -> Option<String> {
    let key = entry
        .get("key")
        .and_then(Value::as_str)
        .or_else(|| entry.get("apiKey").and_then(Value::as_str))
        .or_else(|| entry.as_str())?;
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    Some(key.to_owned())
}

fn normalize_usage(raw: &Value) -> Option<Value> {
    let usage = raw.get("usage").unwrap_or(raw);
    let mut entries = Vec::new();
    for (id, duration, kind, label) in [
        ("rolling", 300.0, "primary", None),
        ("weekly", 10_080.0, "secondary", None),
        ("monthly", 43_200.0, "secondary", Some("Monthly")),
    ] {
        let Some(window) = usage.get(id) else {
            continue;
        };
        let Some(used) = finite_f64(window.get("percent"))
            .or_else(|| finite_f64(window.get("usedPercent")))
        else {
            continue;
        };
        let resets_at = parse_reset_timestamp(
            window
                .get("resetsAt")
                .or_else(|| window.get("resets_at")),
        );
        let mut snapshot = serde_json::Map::new();
        snapshot.insert("limitId".into(), Value::from(id));
        if let Some(label) = label {
            snapshot.insert("windowLabel".into(), Value::from(label));
        }
        if used >= 100.0
            || window
                .get("status")
                .and_then(Value::as_str)
                .is_some_and(|status| matches!(status, "rate_limited" | "blocked" | "exceeded"))
        {
            snapshot.insert("rateLimitReachedType".into(), Value::from("limit_reached"));
        }
        snapshot.insert(kind.into(), window_snapshot(used, Some(duration), resets_at));
        entries.push((id.to_owned(), Value::Object(snapshot)));
    }
    if entries.is_empty() {
        return None;
    }
    Some(rate_limits_map(entries))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_go_key_and_ignores_zen() {
        let raw = r#"{
            "opencode": { "type": "api", "key": "zen-key" },
            "opencode-go": { "type": "api", "key": "go-key" },
            "anthropic": { "type": "api", "key": "sk-ant" }
        }"#;
        assert_eq!(parse_go_key(raw).as_deref(), Some("go-key"));
        assert!(parse_go_key(r#"{"opencode":{"type":"api","key":"zen-key"}}"#).is_none());
        assert!(parse_go_key("{}").is_none());
        assert_eq!(
            parse_go_key(r#"{"opencode.go":{"key":"dotted"}}"#).as_deref(),
            Some("dotted")
        );
    }

    #[test]
    fn normalizes_rolling_weekly_monthly_windows() {
        let payload = json!({
            "usage": {
                "rolling": { "status": "ok", "percent": 4, "resetsAt": "2026-08-13T16:27:38Z" },
                "weekly": { "status": "ok", "percent": 3, "resetsAt": "2026-08-17T00:00:00Z" },
                "monthly": { "status": "ok", "percent": 1, "resetsAt": "2026-09-13T06:06:01Z" }
            }
        });
        let normalized = normalize_usage(&payload).expect("windows");
        let by_id = normalized
            .get("rateLimitsByLimitId")
            .and_then(Value::as_object)
            .expect("map");
        assert_eq!(by_id.len(), 3);
        assert_eq!(by_id["rolling"].pointer("/primary/usedPercent"), Some(&json!(4.0)));
        assert_eq!(
            by_id["rolling"].pointer("/primary/windowDurationMins"),
            Some(&json!(300.0))
        );
        assert_eq!(by_id["monthly"].get("windowLabel"), Some(&json!("Monthly")));
        let windows = crate::tray::collect_windows(Some(&normalized));
        let labels: Vec<&str> = windows.iter().map(|window| window.label.as_str()).collect();
        assert_eq!(labels, vec!["5-hour", "Weekly", "Monthly"]);
    }

    #[test]
    fn empty_payload_is_rejected() {
        assert!(normalize_usage(&json!({})).is_none());
        assert!(normalize_usage(&json!({ "usage": {} })).is_none());
    }
}
