//! Devin CLI usage provider.
//!
//! Reads the API key the Devin CLI already stores in `credentials.toml` and
//! asks Cognition's `GetUserStatus` service for the daily and weekly quota
//! windows that `/usage` shows. Access is read-only: the key is never written,
//! refreshed, or sent anywhere except the API server the CLI itself uses
//! (typically `server.codeium.com`).

use std::sync::Arc;

use serde_json::{json, Map, Value};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{Mutex, RwLock};

use crate::codex::process::ConnectionState;
use crate::provider::{
    finite_f64, http_client, now_unix_seconds, rate_limits_map, window_snapshot, ProviderState,
};
use crate::tray;

const DEFAULT_API_SERVER: &str = "https://server.codeium.com";
const USER_STATUS_PATH: &str = "/exa.seat_management_pb.SeatManagementService/GetUserStatus";
const CLIENT_VERSION: &str = "1.108.2";
const DAILY_MINS: f64 = 1_440.0;
const WEEKLY_MINS: f64 = 10_080.0;

#[derive(Clone)]
pub struct DevinManager {
    app: AppHandle,
    state: Arc<RwLock<ProviderState>>,
    client: reqwest::Client,
    refresh_lock: Arc<Mutex<()>>,
}

impl DevinManager {
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
            .is_visible(crate::prefs::PROVIDER_DEVIN)
        {
            return Ok(self.snapshot().await);
        }

        let _guard = self.refresh_lock.lock().await;
        let auth = match load_cli_auth().await {
            CredentialRead::Found(auth) => auth,
            CredentialRead::Absent => {
                self.set_connection(
                    ConnectionState::CliNotFound,
                    Some("No Devin CLI login was found on this Mac".into()),
                )
                .await;
                return Ok(self.snapshot().await);
            }
            CredentialRead::Unavailable(message) => {
                if self.snapshot().await.updated_at.is_none() {
                    self.set_connection(
                        ConnectionState::CliNotFound,
                        Some("No Devin CLI login was found on this Mac".into()),
                    )
                    .await;
                    return Ok(self.snapshot().await);
                }
                self.set_connection(ConnectionState::Error, Some(message.clone()))
                    .await;
                return Err(message);
            }
        };

        let response = self
            .client
            .post(user_status_url(&auth.api_server_url))
            .header("Content-Type", "application/json")
            .header("Connect-Protocol-Version", "1")
            .json(&json!({
                "metadata": {
                    "apiKey": auth.api_key,
                    "ideName": "devin-cli",
                    "ideVersion": CLIENT_VERSION,
                    "extensionName": "chisel",
                    "extensionVersion": CLIENT_VERSION,
                    "locale": "en",
                    "os": "darwin",
                    "ideType": "chisel"
                }
            }))
            .send()
            .await
            .map_err(|error| format!("Devin usage request failed: {error}"));
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
                Some("Devin rejected the stored CLI login. Sign in again with `devin auth login`.".into()),
            )
            .await;
            return Ok(self.snapshot().await);
        }
        if !status.is_success() {
            let message = format!("Devin usage endpoint returned HTTP {status}");
            self.set_connection(ConnectionState::Error, Some(message.clone()))
                .await;
            return Err(message);
        }

        let payload: Value = match response.json().await {
            Ok(payload) => payload,
            Err(error) => {
                let message = format!("Devin usage response was not valid JSON: {error}");
                self.set_connection(ConnectionState::Error, Some(message.clone()))
                    .await;
                return Err(message);
            }
        };

        let Some(normalized) = normalize_usage(&payload) else {
            let message = "Devin did not report any quota windows".to_owned();
            self.set_connection(ConnectionState::Error, Some(message.clone()))
                .await;
            return Err(message);
        };

        {
            let mut state = self.state.write().await;
            state.connection = ConnectionState::Connected;
            state.diagnostic = None;
            state.account = Some(json!({
                "type": "api",
                "planType": plan_name(&payload),
            }));
            state.rate_limits = Some(normalized.rate_limits.clone());
            state.updated_at = Some(now_unix_seconds());
        }
        if let Some(alerts) = self.app.try_state::<crate::alerts::UsageAlerts>() {
            alerts
                .observe(&self.app, "Devin", &normalized.rate_limits)
                .await;
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
        let _ = self.app.emit("devin://state", state);
        tray::refresh_unified_tray(&self.app).await;
    }
}

struct CliAuth {
    api_key: String,
    api_server_url: String,
}

enum CredentialRead {
    Found(CliAuth),
    Absent,
    Unavailable(String),
}

async fn load_cli_auth() -> CredentialRead {
    let Some(path) = credentials_path() else {
        return CredentialRead::Unavailable(
            "HOME is not set, so Devin CLI credentials could not be located".to_owned(),
        );
    };
    match tokio::fs::read_to_string(&path).await {
        Ok(contents) => match parse_cli_auth(&contents) {
            Some(auth) => CredentialRead::Found(auth),
            None => CredentialRead::Absent,
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => CredentialRead::Absent,
        Err(error) => {
            CredentialRead::Unavailable(format!("Could not read {}: {error}", path.display()))
        }
    }
}

fn credentials_path() -> Option<std::path::PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        if !xdg.is_empty() {
            return Some(std::path::PathBuf::from(xdg).join("devin/credentials.toml"));
        }
    }
    let home = std::env::var_os("HOME")?;
    Some(std::path::PathBuf::from(home).join(".local/share/devin/credentials.toml"))
}

fn parse_cli_auth(raw: &str) -> Option<CliAuth> {
    let api_key = read_toml_string(raw, "windsurf_api_key")
        .or_else(|| read_toml_string(raw, "api_key"))
        .and_then(non_empty)?;
    let api_server_url = read_toml_string(raw, "api_server_url")
        .and_then(clean_api_server_url)
        .unwrap_or_else(|| DEFAULT_API_SERVER.to_owned());
    Some(CliAuth {
        api_key,
        api_server_url,
    })
}

fn user_status_url(api_server_url: &str) -> String {
    format!("{}{USER_STATUS_PATH}", api_server_url.trim_end_matches('/'))
}

fn clean_api_server_url(value: String) -> Option<String> {
    let value = value.trim().trim_end_matches('/');
    if !value.starts_with("https://") || value.len() <= "https://".len() {
        return None;
    }
    Some(value.to_owned())
}

/// Pulls a top-level TOML string without taking a toml crate dependency.
fn read_toml_string(text: &str, key: &str) -> Option<String> {
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((candidate, value)) = line.split_once('=') else {
            continue;
        };
        if candidate.trim() != key {
            continue;
        }
        return parse_toml_string_value(value.trim());
    }
    None
}

fn parse_toml_string_value(value: &str) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    if value.starts_with('"') {
        return parse_basic_toml_string(value);
    }
    if let Some(rest) = value.strip_prefix('\'') {
        let closing = rest.find('\'')?;
        let parsed = non_empty(rest[..closing].to_owned())?;
        return valid_value_tail(&rest[closing + 1..]).then_some(parsed);
    }
    let value = value.split_once('#').map_or(value, |(value, _)| value);
    non_empty(value.to_owned())
}

fn parse_basic_toml_string(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut escaped = false;
    let mut closing = None;
    for (index, byte) in bytes.iter().enumerate().skip(1) {
        if escaped {
            escaped = false;
            continue;
        }
        match byte {
            b'\\' => escaped = true,
            b'"' => {
                closing = Some(index);
                break;
            }
            _ => {}
        }
    }
    let closing = closing?;
    if !valid_value_tail(&value[closing + 1..]) {
        return None;
    }
    let parsed: String = serde_json::from_str(&value[..=closing]).ok()?;
    non_empty(parsed)
}

fn valid_value_tail(value: &str) -> bool {
    let value = value.trim();
    value.is_empty() || value.starts_with('#')
}

fn non_empty(value: impl AsRef<str>) -> Option<String> {
    let value = value.as_ref().trim();
    (!value.is_empty()).then(|| value.to_owned())
}

struct NormalizedUsage {
    rate_limits: Value,
}

fn normalize_usage(raw: &Value) -> Option<NormalizedUsage> {
    let user_status = field(raw, &["userStatus", "user_status"]).unwrap_or(raw);
    let plan_status = field(user_status, &["planStatus", "plan_status"]).unwrap_or(user_status);
    let plan_info = field(plan_status, &["planInfo", "plan_info"]);
    let hide_daily = bool_value(field_in(
        plan_info,
        &["hideDailyQuota", "hide_daily_quota"],
    ))
    .unwrap_or(false);

    let daily_remaining = finite_f64(field(
        plan_status,
        &[
            "dailyQuotaRemainingPercent",
            "daily_quota_remaining_percent",
        ],
    ));
    let weekly_remaining = finite_f64(field(
        plan_status,
        &[
            "weeklyQuotaRemainingPercent",
            "weekly_quota_remaining_percent",
        ],
    ));
    let daily_reset = parse_unix(field(
        plan_status,
        &["dailyQuotaResetAtUnix", "daily_quota_reset_at_unix"],
    ));
    let weekly_reset = parse_unix(field(
        plan_status,
        &["weeklyQuotaResetAtUnix", "weekly_quota_reset_at_unix"],
    ));

    let mut entries = Vec::new();
    if !hide_daily {
        if let Some(remaining) = daily_remaining {
            entries.push(limit_entry(
                "daily",
                "Daily",
                "primary",
                remaining,
                DAILY_MINS,
                daily_reset,
            ));
        }
    }
    if let Some(remaining) = weekly_remaining {
        let kind = if entries.is_empty() { "primary" } else { "secondary" };
        entries.push(limit_entry(
            "weekly",
            "Weekly",
            kind,
            remaining,
            WEEKLY_MINS,
            weekly_reset,
        ));
    } else if hide_daily {
        if let Some(remaining) = daily_remaining {
            entries.push(limit_entry(
                "weekly",
                "Weekly",
                "primary",
                remaining,
                WEEKLY_MINS,
                weekly_reset.or(daily_reset),
            ));
        }
    }

    if entries.is_empty() {
        return None;
    }
    Some(NormalizedUsage {
        rate_limits: rate_limits_map(entries),
    })
}

fn plan_name(raw: &Value) -> Option<String> {
    let user_status = field(raw, &["userStatus", "user_status"]).unwrap_or(raw);
    let plan_status = field(user_status, &["planStatus", "plan_status"]).unwrap_or(user_status);
    let plan_info = field(plan_status, &["planInfo", "plan_info"]).unwrap_or(plan_status);
    field(plan_info, &["planName", "plan_name"])
        .and_then(Value::as_str)
        .and_then(|name| non_empty(name))
}

fn limit_entry(
    id: &str,
    window_label: &str,
    kind: &str,
    remaining: f64,
    duration: f64,
    resets_at: Option<f64>,
) -> (String, Value) {
    let used = (100.0 - remaining).clamp(0.0, 100.0);
    let mut snapshot = Map::new();
    snapshot.insert("limitId".into(), Value::from(id));
    snapshot.insert("windowLabel".into(), Value::from(window_label));
    snapshot.insert("limitName".into(), Value::from("Devin plan"));
    if used >= 100.0 {
        snapshot.insert("rateLimitReachedType".into(), Value::from("limit_reached"));
    }
    snapshot.insert(kind.into(), window_snapshot(used, Some(duration), resets_at));
    (id.to_owned(), Value::Object(snapshot))
}

fn field<'a>(value: &'a Value, names: &[&str]) -> Option<&'a Value> {
    let object = value.as_object()?;
    names.iter().find_map(|name| object.get(*name))
}

fn field_in<'a>(value: Option<&'a Value>, names: &[&str]) -> Option<&'a Value> {
    field(value?, names)
}

fn bool_value(value: Option<&Value>) -> Option<bool> {
    match value? {
        Value::Bool(flag) => Some(*flag),
        Value::Number(number) => number.as_f64().map(|n| n != 0.0),
        Value::String(text) => match text.trim().to_ascii_lowercase().as_str() {
            "true" | "1" => Some(true),
            "false" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn parse_unix(value: Option<&Value>) -> Option<f64> {
    let number = finite_f64(value)?;
    if number > 10_000_000_000.0 {
        Some(number / 1_000.0)
    } else {
        Some(number)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_cli_key_and_https_server() {
        let raw = r#"
            # Devin CLI credentials
            windsurf_api_key = "cog_test"
            api_server_url = "https://server.codeium.com/"
        "#;
        let auth = parse_cli_auth(raw).expect("auth");
        assert_eq!(auth.api_key, "cog_test");
        assert_eq!(auth.api_server_url, "https://server.codeium.com");
        assert!(parse_cli_auth("api_server_url = \"https://server.codeium.com\"").is_none());
        assert_eq!(
            parse_cli_auth("api_key = 'bare-key'").unwrap().api_key,
            "bare-key"
        );
        assert_eq!(
            parse_cli_auth("windsurf_api_key = \"esc\\\"aped\"").unwrap().api_key,
            "esc\"aped"
        );
        assert!(parse_cli_auth("windsurf_api_key = \"http://insecure.example\"").is_some());
        let insecure = parse_cli_auth(
            "windsurf_api_key = \"cog\"\napi_server_url = \"http://insecure.example\"",
        )
        .unwrap();
        assert_eq!(insecure.api_server_url, DEFAULT_API_SERVER);
    }

    #[test]
    fn builds_the_connect_rpc_url() {
        assert_eq!(
            user_status_url("https://server.codeium.com/"),
            "https://server.codeium.com/exa.seat_management_pb.SeatManagementService/GetUserStatus"
        );
    }

    #[test]
    fn normalizes_daily_and_weekly_remaining_into_used() {
        let payload = json!({
            "userStatus": {
                "planStatus": {
                    "planInfo": { "planName": "Pro", "hideDailyQuota": false },
                    "dailyQuotaRemainingPercent": 72,
                    "weeklyQuotaRemainingPercent": 45,
                    "dailyQuotaResetAtUnix": 1_773_331_200,
                    "weeklyQuotaResetAtUnix": 1_773_763_200
                }
            }
        });
        let normalized = normalize_usage(&payload).expect("windows");
        let by_id = normalized
            .rate_limits
            .get("rateLimitsByLimitId")
            .and_then(Value::as_object)
            .expect("map");
        assert_eq!(
            by_id["daily"].pointer("/primary/usedPercent"),
            Some(&json!(28.0))
        );
        assert_eq!(
            by_id["daily"].pointer("/primary/windowDurationMins"),
            Some(&json!(1_440.0))
        );
        assert_eq!(
            by_id["weekly"].pointer("/secondary/usedPercent"),
            Some(&json!(55.0))
        );
        assert_eq!(by_id["daily"].get("windowLabel"), Some(&json!("Daily")));
        assert_eq!(plan_name(&payload).as_deref(), Some("Pro"));
        let windows = crate::tray::collect_windows(Some(&normalized.rate_limits));
        let labels: Vec<&str> = windows.iter().map(|window| window.label.as_str()).collect();
        assert_eq!(labels, vec!["Daily", "Weekly"]);
    }

    #[test]
    fn hides_daily_on_max_and_keeps_weekly() {
        let payload = json!({
            "user_status": {
                "plan_status": {
                    "plan_info": { "plan_name": "Max", "hide_daily_quota": true },
                    "weekly_quota_remaining_percent": 81,
                    "weekly_quota_reset_at_unix": 1_773_763_200_000_i64
                }
            }
        });
        let normalized = normalize_usage(&payload).expect("windows");
        let by_id = normalized
            .rate_limits
            .get("rateLimitsByLimitId")
            .and_then(Value::as_object)
            .expect("map");
        assert_eq!(by_id.len(), 1);
        assert_eq!(
            by_id["weekly"].pointer("/primary/usedPercent"),
            Some(&json!(19.0))
        );
        assert_eq!(
            by_id["weekly"].pointer("/primary/resetsAt"),
            Some(&json!(1_773_763_200.0))
        );
        assert_eq!(plan_name(&payload).as_deref(), Some("Max"));
    }

    #[test]
    fn empty_payload_is_rejected() {
        assert!(normalize_usage(&json!({})).is_none());
        assert!(normalize_usage(&json!({ "userStatus": { "planStatus": {} } })).is_none());
    }
}
