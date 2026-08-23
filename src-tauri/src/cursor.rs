//! Cursor usage provider.
//!
//! Cursor has no local app-server equivalent, so this module reads the access
//! token the IDE already stores in `state.vscdb` and asks Cursor's own
//! dashboard API for the current billing-cycle spend. Access is strictly
//! read-only: the token is never written, refreshed, or sent anywhere except
//! `api2.cursor.sh`.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use tauri::{AppHandle, Emitter, Manager};
use tokio::{process::Command, sync::{Mutex, RwLock}};

use crate::codex::process::ConnectionState;
use crate::provider::{
    finite_f64, http_client, now_unix_seconds, parse_reset_timestamp, rate_limits_map,
    window_snapshot, ProviderState,
};
use crate::tray;

const PERIOD_USAGE_URL: &str = "https://api2.cursor.sh/aiserver.v1.DashboardService/GetCurrentPeriodUsage";
const LEGACY_USAGE_URL: &str = "https://api2.cursor.sh/auth/usage";

#[derive(Clone)]
pub struct CursorManager {
    app: AppHandle,
    state: Arc<RwLock<ProviderState>>,
    client: reqwest::Client,
    refresh_lock: Arc<Mutex<()>>,
}

impl CursorManager {
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
            .is_visible(crate::prefs::PROVIDER_CURSOR)
        {
            return Ok(self.snapshot().await);
        }

        let _guard = self.refresh_lock.lock().await;
        let token = match load_access_token().await {
            CredentialRead::Found(token) => token,
            CredentialRead::Absent => {
                self.set_connection(
                    ConnectionState::CliNotFound,
                    Some("No Cursor login was found on this Mac".into()),
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

        let payload = match self.fetch_period_usage(&token).await {
            Ok(payload) => payload,
            Err(period_error) => match self.fetch_legacy_usage(&token).await {
                Ok(payload) => payload,
                Err(_) => {
                    self.set_connection(ConnectionState::Error, Some(period_error.clone()))
                        .await;
                    return Err(period_error);
                }
            },
        };

        let Some(rate_limits) = normalize_usage(&payload) else {
            let message = "Cursor did not report a billing-cycle usage window".to_owned();
            self.set_connection(ConnectionState::Error, Some(message.clone()))
                .await;
            return Err(message);
        };

        let plan = payload
            .pointer("/planInfo/planName")
            .or_else(|| payload.pointer("/planUsage/planName"))
            .or_else(|| payload.get("membershipType"))
            .and_then(Value::as_str)
            .map(str::to_owned);

        {
            let mut state = self.state.write().await;
            state.connection = ConnectionState::Connected;
            state.diagnostic = None;
            state.account = Some(json!({
                "type": "oauth",
                "planType": plan,
            }));
            state.rate_limits = Some(rate_limits.clone());
            state.updated_at = Some(now_unix_seconds());
        }
        if let Some(alerts) = self.app.try_state::<crate::alerts::UsageAlerts>() {
            alerts.observe(&self.app, "Cursor", &rate_limits).await;
        }
        self.emit_state().await;
        Ok(self.snapshot().await)
    }

    async fn fetch_period_usage(&self, token: &str) -> Result<Value, String> {
        let response = self
            .client
            .post(PERIOD_USAGE_URL)
            .bearer_auth(token)
            .header("Content-Type", "application/json")
            .header("Connect-Protocol-Version", "1")
            .body("{}")
            .send()
            .await
            .map_err(|error| format!("Cursor usage request failed: {error}"))?;
        classify_cursor_response(response).await
    }

    async fn fetch_legacy_usage(&self, token: &str) -> Result<Value, String> {
        let response = self
            .client
            .get(LEGACY_USAGE_URL)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|error| format!("Cursor usage request failed: {error}"))?;
        classify_cursor_response(response).await
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
        let _ = self.app.emit("cursor://state", state);
        tray::refresh_unified_tray(&self.app).await;
    }
}

enum CredentialRead {
    Found(String),
    Absent,
    Unavailable(String),
}

async fn classify_cursor_response(response: reqwest::Response) -> Result<Value, String> {
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err("Cursor rejected the stored login. Sign in through the Cursor app, then retry.".into());
    }
    if !status.is_success() {
        return Err(format!("Cursor usage endpoint returned HTTP {status}"));
    }
    response
        .json()
        .await
        .map_err(|error| format!("Cursor usage response was not valid JSON: {error}"))
}

fn state_db_path() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        std::path::PathBuf::from(home)
            .join("Library/Application Support/Cursor/User/globalStorage/state.vscdb"),
    )
}

async fn load_access_token() -> CredentialRead {
    let Some(path) = state_db_path() else {
        return CredentialRead::Unavailable(
            "HOME is not set, so Cursor's login database could not be located".to_owned(),
        );
    };
    match tokio::fs::metadata(&path).await {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return CredentialRead::Absent,
        Err(error) => {
            return CredentialRead::Unavailable(format!("Could not read {}: {error}", path.display()))
        }
    }
    // `immutable=1` lets the read succeed while Cursor has the DB open (WAL).
    let uri = format!("file:{}?mode=ro&immutable=1", path.display());
    let output = Command::new("/usr/bin/sqlite3")
        .args([
            "-noheader",
            "-batch",
            &uri,
            "SELECT value FROM ItemTable WHERE key = 'cursorAuth/accessToken' LIMIT 1;",
        ])
        .output()
        .await;
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            return CredentialRead::Unavailable(format!("sqlite3 failed to run: {error}"))
        }
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return CredentialRead::Unavailable(format!(
            "Cursor's login database could not be read ({stderr})"
        ));
    }
    match parse_stored_token(&String::from_utf8_lossy(&output.stdout)) {
        Some(token) => CredentialRead::Found(token),
        None => CredentialRead::Absent,
    }
}

fn parse_stored_token(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(Value::String(token)) = serde_json::from_str::<Value>(trimmed) {
        let token = token.trim();
        return (!token.is_empty()).then(|| token.to_owned());
    }
    Some(trimmed.to_owned())
}

fn normalize_usage(raw: &Value) -> Option<Value> {
    normalize_period_usage(raw).or_else(|| normalize_legacy_usage(raw))
}

fn normalize_period_usage(raw: &Value) -> Option<Value> {
    let plan = raw
        .get("planUsage")
        .or_else(|| raw.pointer("/individualUsage/plan"))
        .unwrap_or(raw);
    let used = finite_f64(plan.get("totalPercentUsed"))
        .or_else(|| finite_f64(plan.get("percentUsed")))
        .or_else(|| percent_from_spend(plan))?;
    let auto = finite_f64(plan.get("autoPercentUsed"));
    let api = finite_f64(plan.get("apiPercentUsed"));
    let resets_at = parse_reset_timestamp(
        raw.get("billingCycleEnd")
            .or_else(|| plan.get("billingCycleEnd"))
            .or_else(|| raw.pointer("/planInfo/billingCycleEnd")),
    );
    let start = parse_reset_timestamp(
        raw.get("billingCycleStart")
            .or_else(|| plan.get("billingCycleStart")),
    );
    let duration = match (start, resets_at) {
        (Some(start), Some(end)) if end > start => Some(((end - start) / 60.0).round()),
        _ => Some(43_200.0),
    };

    let mut entries = Vec::new();
    entries.push(limit_entry(
        "plan",
        "Monthly",
        "Cursor plan",
        "primary",
        used,
        duration,
        resets_at,
    ));
    if let Some(auto) = auto {
        entries.push(limit_entry(
            "auto",
            "Auto",
            "Auto + Composer",
            "secondary",
            auto,
            duration,
            resets_at,
        ));
    }
    if let Some(api) = api {
        entries.push(limit_entry(
            "api",
            "API",
            "API usage",
            "secondary",
            api,
            duration,
            resets_at,
        ));
    }
    Some(rate_limits_map(entries))
}

fn percent_from_spend(plan: &Value) -> Option<f64> {
    let used = finite_f64(plan.get("totalSpend")).or_else(|| finite_f64(plan.get("includedSpend")))?;
    let limit = finite_f64(plan.get("limit")).filter(|limit| *limit > 0.0)?;
    Some((used / limit) * 100.0)
}

fn normalize_legacy_usage(raw: &Value) -> Option<Value> {
    let gpt4 = raw.get("gpt-4")?;
    let used = finite_f64(gpt4.get("numRequests"))?;
    let limit = finite_f64(gpt4.get("maxRequestUsage")).filter(|limit| *limit > 0.0)?;
    let resets_at = parse_reset_timestamp(raw.get("startOfMonth")).map(|start| start + 30.0 * 86_400.0);
    Some(rate_limits_map(vec![limit_entry(
        "premium",
        "Monthly",
        "Premium requests",
        "primary",
        (used / limit) * 100.0,
        Some(43_200.0),
        resets_at,
    )]))
}

fn limit_entry(
    id: &str,
    window_label: &str,
    limit_name: &str,
    kind: &str,
    used: f64,
    duration: Option<f64>,
    resets_at: Option<f64>,
) -> (String, Value) {
    let mut snapshot = Map::new();
    snapshot.insert("limitId".into(), Value::from(id));
    snapshot.insert("windowLabel".into(), Value::from(window_label));
    snapshot.insert("limitName".into(), Value::from(limit_name));
    if used >= 100.0 {
        snapshot.insert("rateLimitReachedType".into(), Value::from("limit_reached"));
    }
    snapshot.insert(kind.into(), window_snapshot(used, duration, resets_at));
    (id.to_owned(), Value::Object(snapshot))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_json_quotes_from_the_sqlite_value() {
        assert_eq!(parse_stored_token("\"abc.def.ghi\"\n").as_deref(), Some("abc.def.ghi"));
        assert_eq!(parse_stored_token("abc.def.ghi").as_deref(), Some("abc.def.ghi"));
        assert!(parse_stored_token("   ").is_none());
    }

    #[test]
    fn normalizes_period_spend_into_codex_shape() {
        let payload = json!({
            "billingCycleStart": "2026-08-01T00:00:00Z",
            "billingCycleEnd": "2026-09-01T00:00:00Z",
            "planUsage": {
                "totalPercentUsed": 41,
                "autoPercentUsed": 12,
                "apiPercentUsed": 8
            }
        });
        let normalized = normalize_usage(&payload).expect("windows");
        let by_id = normalized
            .get("rateLimitsByLimitId")
            .and_then(Value::as_object)
            .expect("map");
        assert_eq!(by_id["plan"].pointer("/primary/usedPercent"), Some(&json!(41.0)));
        assert_eq!(by_id["plan"].get("windowLabel"), Some(&json!("Monthly")));
        assert_eq!(by_id["auto"].pointer("/secondary/usedPercent"), Some(&json!(12.0)));
        let windows = crate::tray::collect_windows(Some(&normalized));
        let labels: Vec<&str> = windows.iter().map(|window| window.label.as_str()).collect();
        assert!(labels.contains(&"Monthly"));
        assert!(labels.contains(&"Auto"));
        assert!(labels.contains(&"API"));
    }

    #[test]
    fn falls_back_to_legacy_request_counts() {
        let payload = json!({
            "gpt-4": { "numRequests": 150, "maxRequestUsage": 500 },
            "startOfMonth": "2026-08-01T00:00:00Z"
        });
        let normalized = normalize_usage(&payload).expect("legacy");
        assert_eq!(
            normalized.pointer("/rateLimitsByLimitId/premium/primary/usedPercent"),
            Some(&json!(30.0))
        );
    }
}
