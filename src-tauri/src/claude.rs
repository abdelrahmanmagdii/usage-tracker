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
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
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
use crate::tray::{self};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";
const OAUTH_BETA_HEADER: &str = "oauth-2025-04-20";
/// Exit status `security(1)` reports for errSecItemNotFound (-25300 truncated to
/// a shell exit code). It is the ONLY status that proves the login is absent;
/// every other failure — locked keychain, denied or auto-dismissed access
/// prompt, a subprocess killed mid-lookup — says nothing about whether Claude
/// Code is signed in, and used to be misread as "Claude Code is not installed".
const KEYCHAIN_ITEM_NOT_FOUND: i32 = 44;
/// Consecutive empty reads required before the menu-bar meter disappears, once
/// it has shown a number at least once.
const ABSENT_READS_BEFORE_HIDING: u32 = 2;

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
    /// How many consecutive reads found no stored login. Hiding the tray waits
    /// for a confirming second read, so a credential store caught mid-rewrite
    /// cannot blank the meter.
    absent_reads: Arc<AtomicU32>,
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
            absent_reads: Arc::new(AtomicU32::new(0)),
        }
    }

    pub async fn snapshot(&self) -> ClaudeState {
        self.state.read().await.clone()
    }

    pub async fn refresh(&self) -> Result<ClaudeState, String> {
        if !self
            .app
            .state::<crate::prefs::PrefsStore>()
            .get()
            .is_visible(crate::prefs::PROVIDER_CLAUDE)
        {
            return Ok(self.snapshot().await);
        }
        let _guard = self.refresh_lock.lock().await;
        let credentials = match load_credentials().await {
            CredentialRead::Found(credentials) => {
                self.absent_reads.store(0, Ordering::Relaxed);
                credentials
            }
            CredentialRead::Absent => {
                let reads = self.absent_reads.fetch_add(1, Ordering::Relaxed) + 1;
                let has_shown_usage = self.snapshot().await.updated_at.is_some();
                if hides_tray(has_shown_usage, reads) {
                    self.set_connection(
                        ConnectionState::CliNotFound,
                        Some("No Claude Code login was found on this Mac".into()),
                    )
                    .await;
                    return Ok(self.snapshot().await);
                }
                // Claude Code rewrites its keychain item and credentials file
                // when it rotates its own OAuth token, so a single empty read
                // right after a working session is far more likely a write race
                // than a sign-out. Keep the meter and let the next read decide.
                let message = "Claude Code's stored login could not be read".to_owned();
                self.set_connection(ConnectionState::Error, Some(message.clone()))
                    .await;
                return Err(message);
            }
            CredentialRead::Unavailable(message) => {
                // A store that could not be read proves nothing about whether
                // Claude Code is installed, so the meter keeps its last known
                // numbers and stays in the menu bar instead of vanishing.
                self.absent_reads.store(0, Ordering::Relaxed);
                self.set_connection(ConnectionState::Error, Some(message.clone()))
                    .await;
                return Err(message);
            }
        };

        if credentials.is_expired(now_unix_millis()) {
            self.set_connection(
                ConnectionState::NotAuthenticated,
                Some("The Claude Code login has expired. It refreshes the next time the `claude` command-line tool runs.".into()),
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
                Some("Anthropic rejected the stored Claude Code token. It refreshes the next time the `claude` command-line tool runs.".into()),
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
        self.emit_state().await;
        Ok(self.snapshot().await)
    }

    async fn set_connection(&self, connection: ConnectionState, diagnostic: Option<String>) {
        {
            let mut state = self.state.write().await;
            state.connection = connection;
            state.diagnostic = diagnostic;
        }
        // A missing Claude login no longer hides anything: the shared tray keeps
        // showing Codex, and Claude simply drops out of the title and menu until
        // it reappears. That is handled by refresh_unified_tray reading state.
        self.emit_state().await;
    }

    async fn emit_state(&self) {
        let state = self.snapshot().await;
        let _ = self.app.emit("claude://state", state);
        self.update_tray().await;
    }

    pub async fn update_tray(&self) {
        // Both providers share one menu-bar item; the coordinator reads both.
        tray::refresh_unified_tray(&self.app).await;
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

/// What one credential store (or both together) had to say. Keeping "there is
/// no login" apart from "the store could not be read" is what stops a passing
/// keychain hiccup from looking like an uninstalled Claude Code.
enum CredentialRead {
    Found(Credentials),
    /// The store was readable and holds no usable Claude Code login.
    Absent,
    /// The store could not be read this time; the answer is unknown. Carries a
    /// human-readable reason for the popover diagnostic.
    Unavailable(String),
}

/// Reads Claude Code's stored OAuth session without ever modifying it. Both
/// stores are consulted and the freshest usable token wins, because either one
/// can lag behind the other depending on how Claude Code was last used.
async fn load_credentials() -> CredentialRead {
    let keychain = load_from_keychain().await;
    let file = load_from_file().await;
    combine_reads(keychain, file, now_unix_millis())
}

/// Merges the two stores. A usable login from either one wins; failing that, a
/// read failure anywhere outranks "absent", because an unreadable store cannot
/// prove that the user is signed out.
fn combine_reads(first: CredentialRead, second: CredentialRead, now_ms: u64) -> CredentialRead {
    match (first, second) {
        (CredentialRead::Found(first), CredentialRead::Found(second)) => {
            CredentialRead::Found(freshest(first, second, now_ms))
        }
        (CredentialRead::Found(found), _) | (_, CredentialRead::Found(found)) => {
            CredentialRead::Found(found)
        }
        (CredentialRead::Unavailable(reason), _) | (_, CredentialRead::Unavailable(reason)) => {
            CredentialRead::Unavailable(reason)
        }
        _ => CredentialRead::Absent,
    }
}

fn freshest(first: Credentials, second: Credentials, now_ms: u64) -> Credentials {
    match (!first.is_expired(now_ms), !second.is_expired(now_ms)) {
        (true, false) => first,
        (false, true) => second,
        // Both usable or both expired: the later expiry is the more recently
        // issued token, and its diagnostic is the more accurate one.
        _ => {
            if second.expires_at_ms.unwrap_or(0.0) > first.expires_at_ms.unwrap_or(0.0) {
                second
            } else {
                first
            }
        }
    }
}

/// Whether an empty credential read should hide the menu-bar meter. A meter
/// that has never shown a number can hide at once — nothing is lost — but once
/// usage has been on screen, absence has to repeat: Claude Code rewrites both
/// credential stores while rotating its own token, and a read landing inside
/// that window used to make the icon disappear.
fn hides_tray(has_shown_usage: bool, consecutive_absent_reads: u32) -> bool {
    !has_shown_usage || consecutive_absent_reads >= ABSENT_READS_BEFORE_HIDING
}

/// Classifies a failing `security find-generic-password` run. Only
/// errSecItemNotFound means the login is absent.
fn classify_keychain_exit(code: Option<i32>) -> CredentialRead {
    match code {
        Some(KEYCHAIN_ITEM_NOT_FOUND) => CredentialRead::Absent,
        // Locked keychain (36/51), a denied or dismissed access prompt (128),
        // or anything else the tool reports: the lookup failed, so the meter
        // must keep whatever it was already showing.
        Some(code) => CredentialRead::Unavailable(format!(
            "The Claude Code login could not be read from the keychain (security exited {code})"
        )),
        // No exit code at all means the subprocess was signalled, e.g. killed
        // while another app held the keychain.
        None => CredentialRead::Unavailable(
            "The keychain lookup for the Claude Code login was interrupted".to_owned(),
        ),
    }
}

async fn load_from_keychain() -> CredentialRead {
    let output = Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"])
        .stdin(Stdio::null())
        .output()
        .await;
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            return CredentialRead::Unavailable(format!("Keychain lookup failed to run: {error}"))
        }
    };
    if !output.status.success() {
        return classify_keychain_exit(output.status.code());
    }
    match parse_credentials(&String::from_utf8_lossy(&output.stdout)) {
        Some(credentials) => CredentialRead::Found(credentials),
        // The item is readable but carries no usable token: signed out.
        None => CredentialRead::Absent,
    }
}

async fn load_from_file() -> CredentialRead {
    let Some(home) = std::env::var_os("HOME") else {
        return CredentialRead::Unavailable(
            "HOME is not set, so the Claude Code credentials file could not be located".to_owned(),
        );
    };
    let path = std::path::PathBuf::from(home).join(".claude/.credentials.json");
    match tokio::fs::read_to_string(&path).await {
        Ok(contents) => match parse_credentials(&contents) {
            Some(credentials) => CredentialRead::Found(credentials),
            None => CredentialRead::Absent,
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => CredentialRead::Absent,
        // Permissions, I/O errors, a half-written file: unknown, not absent.
        Err(error) => CredentialRead::Unavailable(format!("Could not read {}: {error}", path.display())),
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
    // The `limits` entries and the legacy `five_hour`/`seven_day` objects
    // describe the same windows, and either side can report `resets_at: null`
    // on its own. Carrying the legacy timestamp across avoids showing "reset
    // time unavailable" when the other half of the payload knows the answer.
    let session_fallback = parse_reset_timestamp(raw.pointer("/five_hour/resets_at"));
    let weekly_fallback = parse_reset_timestamp(raw.pointer("/seven_day/resets_at"));
    if let Some(limits) = raw.get("limits").and_then(Value::as_array) {
        for limit in limits {
            let Some(entry) = normalize_limit_entry(limit, session_fallback, weekly_fallback) else {
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

fn normalize_limit_entry(
    limit: &Value,
    session_fallback: Option<f64>,
    weekly_fallback: Option<f64>,
) -> Option<(String, Value)> {
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

    // "critical" means nearly out, not out: only an explicitly exceeded
    // severity (or a full window) counts as reached, so a 92% window is not
    // mislabeled "Limit reached".
    let severity = limit.get("severity").and_then(Value::as_str);
    let reached = used >= 100.0
        || severity.is_some_and(|severity| {
            matches!(severity, "exceeded" | "reached" | "exhausted" | "blocked")
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
    let resets_at = parse_reset_timestamp(limit.get("resets_at")).or(if duration == Some(300.0) {
        session_fallback
    } else if duration == Some(10_080.0) {
        weekly_fallback
    } else {
        None
    });
    let mut window = window_snapshot(used, duration, resets_at);
    if let Some(name) = scope_name {
        // Model-scoped windows are marked so the tray can treat them as
        // optional, and labeled so the menu-bar picker and tooltip can name
        // which window the number belongs to.
        if let Value::Object(map) = &mut window {
            map.insert("excludeFromTray".into(), Value::Bool(true));
            map.insert("windowLabel".into(), Value::from(name));
        }
    }
    snapshot.insert(window_kind.into(), window);
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
        assert_eq!(scoped.pointer("/secondary/excludeFromTray"), Some(&json!(true)));

        // The tray picker sees all three windows, labeled for the menu.
        let windows = crate::tray::collect_windows(Some(&normalized));
        let labels: Vec<&str> = windows.iter().map(|window| window.label.as_str()).collect();
        assert_eq!(labels, vec!["5-hour", "Weekly", "Fable"]);
        let auto = crate::tray::select_window(&windows, crate::prefs::TRAY_WINDOW_AUTO).expect("auto");
        assert!((auto.used_percent - 63.0).abs() < 1e-9);
        assert_eq!(auto.label, "Fable");
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
    fn a_critical_window_short_of_the_cap_is_not_reached() {
        // "critical" means nearly out; only an exceeded window is reached.
        let payload = json!({
            "limits": [{ "kind": "weekly_scoped", "group": "weekly", "percent": 92,
                         "severity": "critical", "resets_at": null,
                         "scope": { "model": { "display_name": "Fable" } } }]
        });
        let normalized = normalize_usage(&payload);
        assert_eq!(
            normalized.pointer("/rateLimitsByLimitId/weekly-scoped-fable/rateLimitReachedType"),
            None
        );
    }

    #[test]
    fn a_null_window_reset_falls_back_to_the_legacy_timestamp() {
        // The API can report resets_at on the legacy objects but not on the
        // matching `limits` entry, which showed up as "Reset time unavailable".
        let payload = json!({
            "five_hour": { "utilization": 8.0, "resets_at": "2026-08-16T07:59:59.515922+00:00" },
            "seven_day": { "utilization": 57.0, "resets_at": "2026-08-16T22:59:59.515938+00:00" },
            "limits": [
                { "kind": "session", "group": "session", "percent": 8, "resets_at": null },
                { "kind": "weekly_all", "group": "weekly", "percent": 57, "resets_at": null }
            ]
        });
        let normalized = normalize_usage(&payload);
        assert_eq!(
            normalized.pointer("/rateLimitsByLimitId/session/primary/resetsAt"),
            Some(&json!(1_786_867_199.0))
        );
        assert_eq!(
            normalized.pointer("/rateLimitsByLimitId/weekly-all/secondary/resetsAt"),
            Some(&json!(1_786_921_199.0))
        );
    }

    fn creds(expires_at: f64) -> Credentials {
        Credentials {
            access_token: format!("token-{expires_at}"),
            expires_at_ms: Some(expires_at),
            subscription_type: None,
        }
    }

    fn expiry(read: &CredentialRead) -> Option<f64> {
        match read {
            CredentialRead::Found(credentials) => credentials.expires_at_ms,
            _ => None,
        }
    }

    #[test]
    fn picks_the_freshest_usable_credential_source() {
        // A valid source beats an expired one regardless of order.
        assert_eq!(freshest(creds(500.0), creds(2_000.0), 1_000).expires_at_ms, Some(2_000.0));
        assert_eq!(freshest(creds(2_000.0), creds(500.0), 1_000).expires_at_ms, Some(2_000.0));
        // Both expired: the fresher one wins (its message is more accurate).
        assert_eq!(freshest(creds(500.0), creds(800.0), 1_000).expires_at_ms, Some(800.0));
    }

    #[test]
    fn only_err_sec_item_not_found_means_the_login_is_absent() {
        // 44 is errSecItemNotFound: Claude Code really has no keychain item.
        assert!(matches!(
            classify_keychain_exit(Some(44)),
            CredentialRead::Absent
        ));
        // A locked keychain (36), a denied prompt (51), a dismissed one (128)
        // and a signalled subprocess are all unknowns, never absence.
        for code in [36, 51, 128, 1] {
            let read = classify_keychain_exit(Some(code));
            let CredentialRead::Unavailable(reason) = read else {
                panic!("exit {code} must not be read as a missing login");
            };
            assert!(reason.contains(&code.to_string()));
        }
        assert!(matches!(
            classify_keychain_exit(None),
            CredentialRead::Unavailable(_)
        ));
    }

    #[test]
    fn an_unreadable_store_never_reports_a_sign_out() {
        let unavailable = || CredentialRead::Unavailable("keychain locked".to_owned());
        // A usable login from either store wins outright.
        assert_eq!(
            expiry(&combine_reads(unavailable(), CredentialRead::Found(creds(2_000.0)), 1_000)),
            Some(2_000.0)
        );
        assert_eq!(
            expiry(&combine_reads(
                CredentialRead::Found(creds(2_000.0)),
                CredentialRead::Found(creds(3_000.0)),
                1_000
            )),
            Some(3_000.0)
        );
        // One store failing outranks the other's emptiness: the tray must not
        // be hidden on the strength of a lookup that never completed.
        assert!(matches!(
            combine_reads(unavailable(), CredentialRead::Absent, 1_000),
            CredentialRead::Unavailable(_)
        ));
        assert!(matches!(
            combine_reads(CredentialRead::Absent, unavailable(), 1_000),
            CredentialRead::Unavailable(_)
        ));
        // Both stores readable and empty is the one true "no login here".
        assert!(matches!(
            combine_reads(CredentialRead::Absent, CredentialRead::Absent, 1_000),
            CredentialRead::Absent
        ));
    }

    #[test]
    fn hiding_the_tray_needs_a_confirmed_absence() {
        // Nothing has ever been shown: hide immediately, no icon is lost.
        assert!(hides_tray(false, 1));
        // A meter that has been showing usage survives one empty read (a
        // credential store caught mid-rewrite) and hides on the second.
        assert!(!hides_tray(true, 1));
        assert!(hides_tray(true, ABSENT_READS_BEFORE_HIDING));
        assert!(hides_tray(true, ABSENT_READS_BEFORE_HIDING + 5));
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

