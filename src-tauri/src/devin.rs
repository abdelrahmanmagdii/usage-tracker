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
    finite_f64, http_client, now_unix_seconds, parse_reset_timestamp, rate_limits_map,
    window_snapshot, ProviderState,
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
            state: Arc::new(RwLock::new(crate::demo::initial_devin())),
            client: http_client(),
            refresh_lock: Arc::new(Mutex::new(())),
        }
    }

    pub async fn snapshot(&self) -> ProviderState {
        self.state.read().await.clone()
    }

    pub async fn refresh(&self) -> Result<ProviderState, String> {
        if crate::demo::enabled() {
            return Ok(self.snapshot().await);
        }
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
                    Some("No Devin CLI login was found on this computer".into()),
                )
                .await;
                return Ok(self.snapshot().await);
            }
            CredentialRead::Unavailable(message) => {
                let snapshot = self.snapshot().await;
                if !crate::provider::keep_last_meter(
                    snapshot.connection,
                    snapshot.updated_at.is_some(),
                ) {
                    self.set_connection(
                        ConnectionState::CliNotFound,
                        Some("No Devin CLI login was found on this computer".into()),
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
            .map_err(request_failure);
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
                Some(
                    "Devin rejected the stored CLI login. Sign in again with `devin auth login`."
                        .into(),
                ),
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
            if connection == ConnectionState::CliNotFound {
                crate::provider::conceal_provider(&mut state, diagnostic);
            } else {
                state.connection = connection;
                state.diagnostic = diagnostic;
            }
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
            "The home directory is unavailable, so Devin CLI credentials could not be located"
                .to_owned(),
        );
    };
    match crate::platform::read_capped(&path, crate::platform::CREDENTIAL_FILE_LIMIT).await {
        Ok(bytes) => match std::str::from_utf8(&bytes) {
            Ok(contents) => classify_credentials(contents),
            Err(_) => CredentialRead::Unavailable(format!(
                "Could not read {}: it is not UTF-8 text",
                path.display()
            )),
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
    let home = crate::platform::home_dir()?;
    Some(home.join(".local/share/devin/credentials.toml"))
}

/// No key hides the meter. A key line whose value was cut off is a file the
/// CLI is still writing, so it is a passing failure that keeps the last meter.
fn classify_credentials(raw: &str) -> CredentialRead {
    if let Some(auth) = parse_cli_auth(raw) {
        return CredentialRead::Found(auth);
    }
    if ["windsurf_api_key", "api_key"]
        .iter()
        .any(|key| toml_value_truncated(raw, key))
    {
        return CredentialRead::Unavailable(
            "The Devin CLI credentials file could not be parsed".to_owned(),
        );
    }
    CredentialRead::Absent
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

/// The server comes from the CLI's own file, but it ends up in requests and,
/// through reqwest errors, in diagnostics. Accept only a plain https origin
/// (optionally with a path): no embedded `user:password@`, query, or fragment.
fn clean_api_server_url(value: String) -> Option<String> {
    let value = value.trim().trim_end_matches('/');
    if !value.starts_with("https://") {
        return None;
    }
    let url = reqwest::Url::parse(value).ok()?;
    let plain = url.host_str().is_some_and(|host| !host.is_empty())
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none();
    plain.then(|| value.to_owned())
}

/// reqwest's error text embeds the request URL. The diagnostic is shown in
/// the UI, so keep the cause and drop the URL.
fn request_failure(error: reqwest::Error) -> String {
    format!("Devin usage request failed: {}", error.without_url())
}

/// Pulls a top-level TOML string without taking a toml crate dependency.
fn read_toml_string(text: &str, key: &str) -> Option<String> {
    parse_toml_string_value(raw_toml_value(text, key)?)
}

/// The text after `key =` on the first line that assigns `key`.
fn raw_toml_value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().find_map(|raw_line| {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            return None;
        }
        let (candidate, value) = line.split_once('=')?;
        (candidate.trim() == key).then(|| value.trim())
    })
}

/// Whether `key` is assigned but its value stops short: nothing after the
/// `=`, or a string whose closing quote never arrives.
fn toml_value_truncated(text: &str, key: &str) -> bool {
    let Some(value) = raw_toml_value(text, key) else {
        return false;
    };
    if value.is_empty() {
        return true;
    }
    if value.starts_with('"') {
        return basic_string_end(value).is_none();
    }
    if let Some(rest) = value.strip_prefix('\'') {
        return !rest.contains('\'');
    }
    false
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
        let parsed = non_empty(&rest[..closing])?;
        return valid_value_tail(&rest[closing + 1..]).then_some(parsed);
    }
    let value = value.split_once('#').map_or(value, |(value, _)| value);
    non_empty(value)
}

/// Index of the quote that closes a basic string opening at `value[0]`.
fn basic_string_end(value: &str) -> Option<usize> {
    let mut escaped = false;
    for (index, byte) in value.bytes().enumerate().skip(1) {
        if escaped {
            escaped = false;
            continue;
        }
        match byte {
            b'\\' => escaped = true,
            b'"' => return Some(index),
            _ => {}
        }
    }
    None
}

fn parse_basic_toml_string(value: &str) -> Option<String> {
    let closing = basic_string_end(value)?;
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
    let hide_daily =
        bool_value(field_in(plan_info, &["hideDailyQuota", "hide_daily_quota"])).unwrap_or(false);

    let daily_reset = parse_reset_timestamp(field(
        plan_status,
        &["dailyQuotaResetAtUnix", "daily_quota_reset_at_unix"],
    ));
    let weekly_reset = parse_reset_timestamp(field(
        plan_status,
        &["weeklyQuotaResetAtUnix", "weekly_quota_reset_at_unix"],
    ));
    let daily_remaining = remaining_percent(
        field(
            plan_status,
            &[
                "dailyQuotaRemainingPercent",
                "daily_quota_remaining_percent",
            ],
        ),
        daily_reset,
    );
    let weekly_remaining = remaining_percent(
        field(
            plan_status,
            &[
                "weeklyQuotaRemainingPercent",
                "weekly_quota_remaining_percent",
            ],
        ),
        weekly_reset,
    );

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
        let kind = if entries.is_empty() {
            "primary"
        } else {
            "secondary"
        };
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
        .and_then(non_empty)
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
    snapshot.insert(
        kind.into(),
        window_snapshot(used, Some(duration), resets_at),
    );
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

/// `GetUserStatus` is a Connect endpoint, and protobuf JSON leaves out any
/// field equal to zero. A spent window therefore arrives with its reset time
/// but no remaining percent; reading that as "no window" hid the meter at the
/// moment it mattered most. A window that has a reset time is a real window,
/// so its missing remainder is 0%.
fn remaining_percent(value: Option<&Value>, resets_at: Option<f64>) -> Option<f64> {
    finite_f64(value).or(resets_at.map(|_| 0.0))
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
            parse_cli_auth("windsurf_api_key = \"esc\\\"aped\"")
                .unwrap()
                .api_key,
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
    fn the_api_server_must_be_a_plain_https_origin() {
        let server = |url: &str| clean_api_server_url(url.to_owned());
        assert_eq!(
            server(" https://server.codeium.com/ ").as_deref(),
            Some("https://server.codeium.com")
        );
        assert_eq!(
            server("https://eu.windsurf.example/api").as_deref(),
            Some("https://eu.windsurf.example/api")
        );
        for rejected in [
            "https://",
            "http://server.codeium.com",
            "https://user:secret@server.codeium.com",
            "https://server.codeium.com/?token=secret",
            "https://server.codeium.com/#secret",
            "server.codeium.com",
        ] {
            assert_eq!(server(rejected), None, "{rejected}");
        }
    }

    #[test]
    fn request_failures_do_not_repeat_the_url() {
        // Nothing listens on port 9 of this machine, so the send fails.
        let error = tauri::async_runtime::block_on(async {
            reqwest::Client::builder()
                .no_proxy()
                .build()
                .expect("client")
                .post("http://127.0.0.1:9/exa.seat_management_pb.SeatManagementService/GetUserStatus?trace=marker")
                .send()
                .await
                .expect_err("nothing listens on the discard port")
        });
        let message = request_failure(error);
        assert!(message.starts_with("Devin usage request failed"));
        assert!(!message.contains("marker"), "{message}");
        assert!(!message.contains("127.0.0.1"), "{message}");
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
    fn a_spent_window_omitted_by_protobuf_json_reads_as_full() {
        // Protobuf JSON drops `dailyQuotaRemainingPercent: 0`; int64 resets
        // arrive as strings.
        let payload = json!({
            "userStatus": {
                "planStatus": {
                    "planInfo": { "planName": "Pro" },
                    "weeklyQuotaRemainingPercent": 40,
                    "dailyQuotaResetAtUnix": "1773331200",
                    "weeklyQuotaResetAtUnix": "1773763200"
                }
            }
        });
        let normalized = normalize_usage(&payload).expect("windows");
        let daily = &normalized.rate_limits["rateLimitsByLimitId"]["daily"];
        assert_eq!(daily.pointer("/primary/usedPercent"), Some(&json!(100.0)));
        assert_eq!(
            daily.pointer("/primary/resetsAt"),
            Some(&json!(1_773_331_200.0))
        );
        assert_eq!(
            daily.get("rateLimitReachedType"),
            Some(&json!("limit_reached"))
        );
        assert_eq!(
            normalized
                .rate_limits
                .pointer("/rateLimitsByLimitId/weekly/secondary/usedPercent"),
            Some(&json!(60.0))
        );
    }

    #[test]
    fn a_window_with_neither_percent_nor_reset_is_not_invented() {
        let payload = json!({
            "userStatus": { "planStatus": { "weeklyQuotaRemainingPercent": 70 } }
        });
        let normalized = normalize_usage(&payload).expect("windows");
        let by_id = normalized.rate_limits["rateLimitsByLimitId"]
            .as_object()
            .expect("map");
        assert_eq!(by_id.len(), 1);
        assert!(by_id.contains_key("weekly"));
    }

    #[test]
    fn a_cut_off_credentials_file_is_a_passing_failure() {
        for partial in [
            "windsurf_api_key = \"cog_abc",
            "windsurf_api_key = ",
            "api_key = 'cog_abc",
        ] {
            assert!(
                matches!(
                    classify_credentials(partial),
                    CredentialRead::Unavailable(_)
                ),
                "{partial:?} must keep the last meter"
            );
        }
        assert!(matches!(classify_credentials(""), CredentialRead::Absent));
        assert!(matches!(
            classify_credentials("windsurf_api_key = \"\"\n"),
            CredentialRead::Absent
        ));
        assert!(matches!(
            classify_credentials("windsurf_api_key = \"cog\" # signed in\n"),
            CredentialRead::Found(_)
        ));
    }

    #[test]
    fn empty_payload_is_rejected() {
        assert!(normalize_usage(&json!({})).is_none());
        assert!(normalize_usage(&json!({ "userStatus": { "planStatus": {} } })).is_none());
    }
}
