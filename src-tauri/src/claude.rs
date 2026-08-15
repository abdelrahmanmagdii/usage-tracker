//! Claude Code usage provider.
//!
//! Claude Code has no local app-server equivalent, so this module reads the
//! OAuth session that Claude Code itself maintains (macOS Keychain first, the
//! `~/.claude/.credentials.json` fallback second) and asks Anthropic's own
//! usage endpoint for the same rate-limit windows the `/usage` screen shows.
//! Access is strictly read-only: the token is never refreshed or rewritten, so
//! Claude Code's session can never be invalidated by this app.

use std::{
    process::Stdio,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use serde_json::{json, Map, Value};
use tauri::{AppHandle, Emitter, Manager};
use tokio::{
    process::Command,
    sync::{Mutex, RwLock},
    time::Duration,
};

use crate::codex::process::ConnectionState;
use crate::tray::{self, most_cooked_window, tray_title};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";
const OAUTH_BETA_HEADER: &str = "oauth-2025-04-20";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeState {
    pub connection: ConnectionState,
    pub diagnostic: Option<String>,
    pub account: Option<Value>,
    pub rate_limits: Option<Value>,
    pub updated_at: Option<u64>,
}

impl Default for ClaudeState {
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

#[derive(Clone)]
pub struct ClaudeManager {
    app: AppHandle,
    state: Arc<RwLock<ClaudeState>>,
    client: reqwest::Client,
    refresh_lock: Arc<Mutex<()>>,
}

impl ClaudeManager {
    pub fn new(app: AppHandle) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent(concat!("usagebar/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("reqwest client construction is infallible with these options");
        Self {
            app,
            state: Arc::new(RwLock::new(ClaudeState::default())),
            client,
            refresh_lock: Arc::new(Mutex::new(())),
        }
    }

    pub async fn snapshot(&self) -> ClaudeState {
        self.state.read().await.clone()
    }

    pub async fn refresh(&self) -> Result<ClaudeState, String> {
        let _guard = self.refresh_lock.lock().await;
        let credentials = match load_credentials().await {
            Ok(Some(credentials)) => credentials,
            Ok(None) => {
                self.set_connection(
                    ConnectionState::CliNotFound,
                    Some("No Claude Code login was found on this Mac".into()),
                )
                .await;
                return Ok(self.snapshot().await);
            }
            Err(message) => {
                self.set_connection(ConnectionState::Error, Some(message.clone()))
                    .await;
                return Err(message);
            }
        };

        if credentials.is_expired(now_unix_millis()) {
            self.set_connection(
                ConnectionState::NotAuthenticated,
                Some("The Claude Code session has expired. Open Claude Code once to sign in again.".into()),
            )
            .await;
            return Ok(self.snapshot().await);
        }

        let response = self
            .client
            .get(USAGE_URL)
            .bearer_auth(&credentials.access_token)
            .header("anthropic-beta", OAUTH_BETA_HEADER)
            .send()
            .await
            .map_err(|error| format!("Claude usage request failed: {error}"));
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
                Some("Anthropic rejected the Claude Code session token. Open Claude Code once to sign in again.".into()),
            )
            .await;
            return Ok(self.snapshot().await);
        }
        if !status.is_success() {
            let message = format!("Claude usage endpoint returned HTTP {status}");
            self.set_connection(ConnectionState::Error, Some(message.clone()))
                .await;
            return Err(message);
        }

        let payload: Value = match response.json().await {
            Ok(payload) => payload,
            Err(error) => {
                let message = format!("Claude usage response was not valid JSON: {error}");
                self.set_connection(ConnectionState::Error, Some(message.clone()))
                    .await;
                return Err(message);
            }
        };

        let rate_limits = normalize_usage(&payload);
        {
            let mut state = self.state.write().await;
            state.connection = ConnectionState::Connected;
            state.diagnostic = None;
            state.account = Some(json!({
                "type": "oauth",
                "planType": credentials.subscription_type.clone()
            }));
            state.rate_limits = Some(rate_limits.clone());
            state.updated_at = Some(now_unix_millis() / 1_000);
            #[cfg(debug_assertions)]
            eprintln!(
                "claude usage: connected, windows={}",
                state
                    .rate_limits
                    .as_ref()
                    .and_then(|limits| limits.get("rateLimitsByLimitId"))
                    .and_then(Value::as_object)
                    .map(|map| map.len())
                    .unwrap_or(0)
            );
        }
        if let Some(alerts) = self.app.try_state::<crate::alerts::UsageAlerts>() {
            alerts.observe(&self.app, "Claude Code", &rate_limits).await;
        }
        let _ = tray::ensure_claude_tray(&self.app);
        self.emit_state().await;
        Ok(self.snapshot().await)
    }

    async fn set_connection(&self, connection: ConnectionState, diagnostic: Option<String>) {
        {
            let mut state = self.state.write().await;
            state.connection = connection;
            state.diagnostic = diagnostic;
        }
        if connection == ConnectionState::CliNotFound {
            if let Some(tray) = self.app.tray_by_id(tray::CLAUDE_TRAY_ID) {
                let _ = tray.set_visible(false);
            }
        }
        self.emit_state().await;
    }

    async fn emit_state(&self) {
        let state = self.snapshot().await;
        let _ = self.app.emit("claude://state", state);
        self.update_tray().await;
    }

    pub async fn update_tray(&self) {
        let state = self.state.read().await;
        let Some(tray) = self.app.tray_by_id(tray::CLAUDE_TRAY_ID) else {
            return;
        };
        let cooked = most_cooked_window(state.rate_limits.as_ref());
        let compact = self
            .app
            .try_state::<crate::prefs::PrefsStore>()
            .map(|prefs| prefs.get().compact_tray)
            .unwrap_or(false);
        let title = tray_title(
            cooked.map(|window| window.remaining),
            cooked.and_then(|window| window.resets_at),
            now_unix_millis() / 1_000,
            compact,
        );
        let _ = tray.set_title(Some(title.as_str()));
    }
}

struct Credentials {
    access_token: String,
    expires_at_ms: Option<f64>,
    subscription_type: Option<String>,
}

impl Credentials {
    fn is_expired(&self, now_ms: u64) -> bool {
        self.expires_at_ms
            .is_some_and(|expires_at| expires_at <= now_ms as f64)
    }
}

/// Reads Claude Code's stored OAuth session without ever modifying it. Both
/// stores are consulted and the freshest usable token wins, because either one
/// can lag behind the other depending on how Claude Code was last used.
async fn load_credentials() -> Result<Option<Credentials>, String> {
    let keychain = load_from_keychain().await?;
    let file = match load_from_file().await {
        Ok(file) => file,
        Err(_) if keychain.is_some() => None,
        Err(error) => return Err(error),
    };
    Ok(pick_credentials(keychain, file, now_unix_millis()))
}

fn pick_credentials(
    first: Option<Credentials>,
    second: Option<Credentials>,
    now_ms: u64,
) -> Option<Credentials> {
    match (first, second) {
        (Some(first), Some(second)) => {
            let first_usable = !first.is_expired(now_ms);
            let second_usable = !second.is_expired(now_ms);
            Some(match (first_usable, second_usable) {
                (true, false) => first,
                (false, true) => second,
                _ => {
                    let first_expiry = first.expires_at_ms.unwrap_or(0.0);
                    let second_expiry = second.expires_at_ms.unwrap_or(0.0);
                    if second_expiry > first_expiry {
                        second
                    } else {
                        first
                    }
                }
            })
        }
        (first, second) => first.or(second),
    }
}

async fn load_from_keychain() -> Result<Option<Credentials>, String> {
    let output = Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"])
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|error| format!("Keychain lookup failed to run: {error}"))?;
    if !output.status.success() {
        // Exit code 44 means the item does not exist; other failures (locked
        // keychain, denied access) also fall through to the file fallback.
        return Ok(None);
    }
    Ok(parse_credentials(&String::from_utf8_lossy(&output.stdout)))
}

async fn load_from_file() -> Result<Option<Credentials>, String> {
    let Some(home) = std::env::var_os("HOME") else {
        return Ok(None);
    };
    let path = std::path::PathBuf::from(home).join(".claude/.credentials.json");
    match tokio::fs::read_to_string(&path).await {
        Ok(contents) => Ok(parse_credentials(&contents)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("Could not read {}: {error}", path.display())),
    }
}

fn parse_credentials(raw: &str) -> Option<Credentials> {
    let value: Value = serde_json::from_str(raw.trim()).ok()?;
    let oauth = value.get("claudeAiOauth")?;
    let access_token = oauth.get("accessToken")?.as_str()?.to_owned();
    if access_token.is_empty() {
        return None;
    }
    Some(Credentials {
        access_token,
        expires_at_ms: oauth.get("expiresAt").and_then(Value::as_f64),
        subscription_type: oauth
            .get("subscriptionType")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

/// Reshapes the usage payload into the same `rateLimitsByLimitId` structure the
/// Codex App Server emits, so the tray and the renderer reuse one pipeline.
fn normalize_usage(raw: &Value) -> Value {
    let mut by_id = Map::new();
    if let Some(limits) = raw.get("limits").and_then(Value::as_array) {
        for limit in limits {
            let Some(entry) = normalize_limit_entry(limit) else {
                continue;
            };
            by_id.insert(entry.0, entry.1);
        }
    }
    if by_id.is_empty() {
        for (key, duration) in [("five_hour", 300.0), ("seven_day", 10_080.0)] {
            let Some(window) = raw.get(key) else { continue };
            let Some(used) = window.get("utilization").and_then(Value::as_f64) else {
                continue;
            };
            let kind = if key == "five_hour" { "primary" } else { "secondary" };
            by_id.insert(
                key.replace('_', "-"),
                json!({
                    "limitId": key.replace('_', "-"),
                    kind: window_snapshot(used, Some(duration), parse_reset_timestamp(window.get("resets_at"))),
                }),
            );
        }
    }
    json!({ "rateLimitsByLimitId": Value::Object(by_id) })
}

fn normalize_limit_entry(limit: &Value) -> Option<(String, Value)> {
    let used = limit
        .get("percent")
        .and_then(Value::as_f64)
        .or_else(|| limit.get("utilization").and_then(Value::as_f64))?;
    let kind = limit.get("kind").and_then(Value::as_str).unwrap_or("limit");
    let group = limit.get("group").and_then(Value::as_str).unwrap_or(kind);
    let scope_name = limit
        .pointer("/scope/model/display_name")
        .or_else(|| limit.pointer("/scope/surface/display_name"))
        .and_then(Value::as_str);

    let duration = if group == "session" || kind == "session" {
        Some(300.0)
    } else if group.contains("weekly") || kind.contains("weekly") {
        Some(10_080.0)
    } else if group.contains("daily") || kind.contains("daily") {
        Some(1_440.0)
    } else {
        None
    };
    let window_kind = if duration == Some(300.0) { "primary" } else { "secondary" };

    let mut id = kind.replace('_', "-");
    if let Some(name) = scope_name {
        id = format!("{id}-{}", name.to_lowercase().replace(' ', "-"));
    }
    // Scoped windows show the scope itself as the tile heading ("Fable"), so
    // the sub-line describes the window instead of repeating the model name.
    let limit_name = if scope_name.is_some() {
        duration.map(|duration| match duration {
            d if d == 10_080.0 => "Weekly limit".to_owned(),
            d if d == 1_440.0 => "Daily limit".to_owned(),
            _ => "Model limit".to_owned(),
        })
    } else {
        (kind == "weekly_all").then(|| "All models".to_owned())
    };

    let severity = limit.get("severity").and_then(Value::as_str);
    let reached = severity.is_some_and(|severity| {
        !matches!(severity, "normal" | "warning" | "notice")
    });

    let mut snapshot = Map::new();
    snapshot.insert("limitId".into(), Value::from(id.clone()));
    if let Some(name) = scope_name {
        snapshot.insert("windowLabel".into(), Value::from(name));
    }
    if let Some(name) = limit_name {
        snapshot.insert("limitName".into(), Value::from(name));
    }
    if reached {
        snapshot.insert(
            "rateLimitReachedType".into(),
            Value::from(severity.unwrap_or("limit_reached")),
        );
    }
    snapshot.insert(
        window_kind.into(),
        window_snapshot(used, duration, parse_reset_timestamp(limit.get("resets_at"))),
    );
    Some((id, Value::Object(snapshot)))
}

fn window_snapshot(used_percent: f64, duration_mins: Option<f64>, resets_at: Option<f64>) -> Value {
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

fn parse_reset_timestamp(value: Option<&Value>) -> Option<f64> {
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

fn now_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_limits_array_into_codex_shape() {
        let payload = json!({
            "five_hour": { "utilization": 5.0, "resets_at": "2026-08-15T20:50:00.437771+00:00" },
            "seven_day": { "utilization": 41.0, "resets_at": "2026-08-16T23:00:00.437793+00:00" },
            "limits": [
                { "kind": "session", "group": "session", "percent": 5, "severity": "normal",
                  "resets_at": "2026-08-15T20:50:00.437771+00:00", "scope": null, "is_active": false },
                { "kind": "weekly_all", "group": "weekly", "percent": 41, "severity": "normal",
                  "resets_at": "2026-08-16T23:00:00.437793+00:00", "scope": null, "is_active": false },
                { "kind": "weekly_scoped", "group": "weekly", "percent": 63, "severity": "normal",
                  "resets_at": "2026-08-16T23:00:00.437998+00:00",
                  "scope": { "model": { "id": null, "display_name": "Fable" }, "surface": null },
                  "is_active": true }
            ]
        });
        let normalized = normalize_usage(&payload);
        let by_id = normalized
            .get("rateLimitsByLimitId")
            .and_then(Value::as_object)
            .expect("normalized map");
        assert_eq!(by_id.len(), 3);

        let session = &by_id["session"];
        assert_eq!(session.pointer("/primary/usedPercent"), Some(&json!(5.0)));
        assert_eq!(
            session.pointer("/primary/windowDurationMins"),
            Some(&json!(300.0))
        );
        let session_reset = session
            .pointer("/primary/resetsAt")
            .and_then(Value::as_f64)
            .expect("session reset timestamp");
        assert_eq!(session_reset, 1_786_827_000.0);

        let scoped = &by_id["weekly-scoped-fable"];
        assert_eq!(scoped.pointer("/secondary/usedPercent"), Some(&json!(63.0)));
        assert_eq!(scoped.get("windowLabel"), Some(&json!("Fable")));
        assert_eq!(scoped.get("limitName"), Some(&json!("Weekly limit")));

        // The tray picks the scoped weekly window because it is the most cooked.
        let cooked = most_cooked_window(Some(&normalized)).expect("cooked window");
        assert!((cooked.remaining - 0.37).abs() < 1e-9);
    }

    #[test]
    fn falls_back_to_legacy_window_objects() {
        let payload = json!({
            "five_hour": { "utilization": 12.5, "resets_at": 1_700_000_000 },
            "seven_day": { "utilization": 80.0, "resets_at": null }
        });
        let normalized = normalize_usage(&payload);
        let by_id = normalized
            .get("rateLimitsByLimitId")
            .and_then(Value::as_object)
            .expect("normalized map");
        assert_eq!(by_id.len(), 2);
        assert_eq!(
            by_id["five-hour"].pointer("/primary/resetsAt"),
            Some(&json!(1_700_000_000.0))
        );
        assert_eq!(
            by_id["seven-day"].pointer("/secondary/usedPercent"),
            Some(&json!(80.0))
        );
    }

    #[test]
    fn severity_marks_reached_limits() {
        let payload = json!({
            "limits": [{ "kind": "session", "group": "session", "percent": 100,
                         "severity": "exceeded", "resets_at": null }]
        });
        let normalized = normalize_usage(&payload);
        assert_eq!(
            normalized.pointer("/rateLimitsByLimitId/session/rateLimitReachedType"),
            Some(&json!("exceeded"))
        );
    }

    #[test]
    fn picks_the_freshest_usable_credential_source() {
        let creds = |expires_at: f64| Credentials {
            access_token: format!("token-{expires_at}"),
            expires_at_ms: Some(expires_at),
            subscription_type: None,
        };
        // A valid source beats an expired one regardless of order.
        let picked = pick_credentials(Some(creds(500.0)), Some(creds(2_000.0)), 1_000).unwrap();
        assert_eq!(picked.expires_at_ms, Some(2_000.0));
        let picked = pick_credentials(Some(creds(2_000.0)), Some(creds(500.0)), 1_000).unwrap();
        assert_eq!(picked.expires_at_ms, Some(2_000.0));
        // Both expired: the fresher one wins (its message is more accurate).
        let picked = pick_credentials(Some(creds(500.0)), Some(creds(800.0)), 1_000).unwrap();
        assert_eq!(picked.expires_at_ms, Some(800.0));
        // Single or missing sources pass through.
        assert!(pick_credentials(None, Some(creds(500.0)), 1_000).is_some());
        assert!(pick_credentials(None, None, 1_000).is_none());
    }

    #[test]
    fn parses_credentials_and_expiry() {
        let credentials = parse_credentials(
            r#"{"claudeAiOauth":{"accessToken":"abc","expiresAt":1000,"subscriptionType":"max"}}"#,
        )
        .expect("credentials");
        assert_eq!(credentials.access_token, "abc");
        assert_eq!(credentials.subscription_type.as_deref(), Some("max"));
        assert!(credentials.is_expired(1_000));
        assert!(!credentials.is_expired(999));
        assert!(parse_credentials("{}").is_none());
        assert!(parse_credentials("not json").is_none());
    }
}
