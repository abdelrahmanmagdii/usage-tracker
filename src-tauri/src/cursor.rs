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
use tokio::sync::{Mutex, RwLock};

use crate::codex::process::ConnectionState;
use crate::provider::{
    finite_f64, http_client, now_unix_seconds, parse_reset_timestamp, rate_limits_map,
    window_snapshot, ProviderState,
};
use crate::tray;

const PERIOD_USAGE_URL: &str =
    "https://api2.cursor.sh/aiserver.v1.DashboardService/GetCurrentPeriodUsage";
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
            state: Arc::new(RwLock::new(crate::demo::initial_cursor())),
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
                    Some("No Cursor login was found on this computer".into()),
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
                        Some("No Cursor login was found on this computer".into()),
                    )
                    .await;
                    return Ok(self.snapshot().await);
                }
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
        return Err(
            "Cursor rejected the stored login. Sign in through the Cursor app, then retry.".into(),
        );
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
    #[cfg(target_os = "windows")]
    {
        return Some(
            std::path::PathBuf::from(std::env::var_os("APPDATA")?)
                .join("Cursor/User/globalStorage/state.vscdb"),
        );
    }
    #[cfg(not(target_os = "windows"))]
    let home = crate::platform::home_dir()?;
    #[cfg(not(target_os = "windows"))]
    Some(home.join("Library/Application Support/Cursor/User/globalStorage/state.vscdb"))
}

async fn load_access_token() -> CredentialRead {
    let Some(path) = state_db_path() else {
        return CredentialRead::Unavailable(
            "The home directory is unavailable, so Cursor's login database could not be located"
                .to_owned(),
        );
    };
    match tokio::fs::metadata(&path).await {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return CredentialRead::Absent
        }
        Err(error) => {
            return CredentialRead::Unavailable(format!(
                "Could not read {}: {error}",
                path.display()
            ))
        }
    }
    match tokio::task::spawn_blocking(move || read_cursor_token(&path)).await {
        Ok(read) => read,
        Err(error) => {
            CredentialRead::Unavailable(format!("Cursor login lookup was cancelled: {error}"))
        }
    }
}

fn sqlite_readonly_uri(path: &std::path::Path, immutable: bool) -> String {
    let encoded = path
        .to_string_lossy()
        .replace('\\', "/")
        .replace('%', "%25")
        .replace('#', "%23")
        .replace('?', "%3F")
        .replace(' ', "%20");
    if immutable {
        format!("file:{encoded}?mode=ro&immutable=1")
    } else {
        format!("file:{encoded}?mode=ro")
    }
}

fn read_cursor_token(path: &std::path::Path) -> CredentialRead {
    // Read through SQLite's normal locking first. Cursor keeps the database
    // open in WAL mode and writes a refreshed access token into the WAL;
    // `immutable=1` ignores the WAL, so it can hand back the token Cursor
    // already replaced, and the request then fails as a rejected login.
    // Immutable stays as the fallback for a database a read-only connection
    // cannot open normally (for example when the WAL index cannot be created).
    match query_cursor_token(&sqlite_readonly_uri(path, false)) {
        Ok(read) => read,
        Err(first) => match query_cursor_token(&sqlite_readonly_uri(path, true)) {
            Ok(read) => read,
            Err(_) => CredentialRead::Unavailable(first),
        },
    }
}

fn query_cursor_token(uri: &str) -> Result<CredentialRead, String> {
    let conn = rusqlite::Connection::open_with_flags(
        uri,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|error| format!("Cursor's login database could not be opened ({error})"))?;
    // Cursor may be mid-checkpoint; wait briefly rather than fail the poll.
    let _ = conn.busy_timeout(std::time::Duration::from_secs(2));
    let value: Result<rusqlite::types::Value, rusqlite::Error> = conn.query_row(
        "SELECT value FROM ItemTable WHERE key = 'cursorAuth/accessToken' LIMIT 1",
        [],
        |row| row.get(0),
    );
    match value {
        Ok(rusqlite::types::Value::Text(raw)) => Ok(token_read(&raw)),
        // VS Code declares `value` as a BLOB column. Builds store text, but
        // UTF-8 bytes are the same login and must not read as unreadable.
        Ok(rusqlite::types::Value::Blob(bytes)) => Ok(std::str::from_utf8(&bytes)
            .map(token_read)
            .unwrap_or(CredentialRead::Absent)),
        Ok(_) | Err(rusqlite::Error::QueryReturnedNoRows) => Ok(CredentialRead::Absent),
        Err(error) => Err(format!(
            "Cursor's login database could not be read ({error})"
        )),
    }
}

fn token_read(raw: &str) -> CredentialRead {
    match parse_stored_token(raw) {
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
    let limit = finite_f64(plan.get("limit")).filter(|limit| *limit > 0.0)?;
    // Protobuf JSON drops zero-valued fields, so a fresh billing cycle sends
    // the plan's `limit` with no spend at all. That is 0% used, not "no meter".
    let used = finite_f64(plan.get("totalSpend"))
        .or_else(|| finite_f64(plan.get("includedSpend")))
        .unwrap_or(0.0);
    Some((used / limit) * 100.0)
}

fn normalize_legacy_usage(raw: &Value) -> Option<Value> {
    let gpt4 = raw.get("gpt-4")?;
    let used = finite_f64(gpt4.get("numRequests"))?;
    let limit = finite_f64(gpt4.get("maxRequestUsage")).filter(|limit| *limit > 0.0)?;
    let resets_at =
        parse_reset_timestamp(raw.get("startOfMonth")).map(|start| start + 30.0 * 86_400.0);
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
        assert_eq!(
            parse_stored_token("\"abc.def.ghi\"\n").as_deref(),
            Some("abc.def.ghi")
        );
        assert_eq!(
            parse_stored_token("abc.def.ghi").as_deref(),
            Some("abc.def.ghi")
        );
        assert!(parse_stored_token("   ").is_none());
    }

    #[test]
    fn encodes_spaces_in_the_sqlite_uri() {
        let path = std::path::Path::new("/Users/me/Library/Application Support/Cursor/state.vscdb");
        assert_eq!(
            sqlite_readonly_uri(path, true),
            "file:/Users/me/Library/Application%20Support/Cursor/state.vscdb?mode=ro&immutable=1"
        );
    }

    #[test]
    fn normalizes_windows_paths_in_sqlite_uris() {
        let path = std::path::Path::new(r"C:\Users\Example User\Cursor\state.vscdb");
        assert_eq!(
            sqlite_readonly_uri(path, true),
            "file:C:/Users/Example%20User/Cursor/state.vscdb?mode=ro&immutable=1"
        );
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
        assert_eq!(
            by_id["plan"].pointer("/primary/usedPercent"),
            Some(&json!(41.0))
        );
        assert_eq!(by_id["plan"].get("windowLabel"), Some(&json!("Monthly")));
        assert_eq!(
            by_id["auto"].pointer("/secondary/usedPercent"),
            Some(&json!(12.0))
        );
        let windows = crate::tray::collect_windows(Some(&normalized));
        let labels: Vec<&str> = windows.iter().map(|window| window.label.as_str()).collect();
        assert!(labels.contains(&"Monthly"));
        assert!(labels.contains(&"Auto"));
        assert!(labels.contains(&"API"));
    }

    fn temp_db(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "usagebar-cursor-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir.join("state.vscdb")
    }

    #[test]
    fn reads_a_token_cursor_just_rotated_into_the_wal() {
        let path = temp_db("wal");
        // Cursor keeps the database open in WAL mode with the newest rows
        // still in the WAL. Disable auto-checkpointing to hold them there.
        let writer = rusqlite::Connection::open(&path).expect("open");
        writer
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                 PRAGMA wal_autocheckpoint=0;
                 CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB);
                 INSERT INTO ItemTable VALUES ('cursorAuth/accessToken', 'old-token');
                 PRAGMA wal_checkpoint(TRUNCATE);
                 INSERT INTO ItemTable VALUES ('cursorAuth/accessToken', 'new-token');",
            )
            .expect("seed");
        match read_cursor_token(&path) {
            CredentialRead::Found(token) => assert_eq!(token, "new-token"),
            _ => panic!("token not found"),
        }
        drop(writer);
        let _ = std::fs::remove_dir_all(path.parent().expect("dir"));
    }

    #[test]
    fn a_missing_token_row_is_absent_and_a_blob_token_is_read() {
        let path = temp_db("blob");
        let writer = rusqlite::Connection::open(&path).expect("open");
        writer
            .execute_batch(
                "CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB);",
            )
            .expect("schema");
        assert!(matches!(read_cursor_token(&path), CredentialRead::Absent));
        writer
            .execute(
                "INSERT INTO ItemTable VALUES ('cursorAuth/accessToken', ?1)",
                [b"\"blob-token\"".to_vec()],
            )
            .expect("insert");
        match read_cursor_token(&path) {
            CredentialRead::Found(token) => assert_eq!(token, "blob-token"),
            _ => panic!("blob token not read"),
        }
        drop(writer);
        let _ = std::fs::remove_dir_all(path.parent().expect("dir"));
    }

    #[test]
    fn reads_connect_millisecond_strings_for_the_billing_cycle() {
        // GetCurrentPeriodUsage sends int64 epochs as protobuf-JSON strings.
        let payload = json!({
            "billingCycleStart": "1785542400000",
            "billingCycleEnd": "1788220800000",
            "planUsage": { "totalSpend": 1250, "limit": 2000, "autoPercentUsed": 3.5 }
        });
        let normalized = normalize_usage(&payload).expect("windows");
        let plan = &normalized["rateLimitsByLimitId"]["plan"]["primary"];
        assert_eq!(plan.get("usedPercent"), Some(&json!(62.5)));
        assert_eq!(plan.get("resetsAt"), Some(&json!(1_788_220_800.0)));
        // 31 days, from the real cycle rather than the 30-day default.
        assert_eq!(plan.get("windowDurationMins"), Some(&json!(44_640.0)));
    }

    #[test]
    fn timestamps_accept_seconds_milliseconds_and_rfc3339() {
        assert_eq!(
            parse_reset_timestamp(Some(&json!(1_788_220_800))),
            Some(1_788_220_800.0)
        );
        assert_eq!(
            parse_reset_timestamp(Some(&json!(1_788_220_800_000_i64))),
            Some(1_788_220_800.0)
        );
        assert_eq!(
            parse_reset_timestamp(Some(&json!("2026-09-01T00:00:00.000Z"))),
            Some(1_788_220_800.0)
        );
        assert_eq!(parse_reset_timestamp(Some(&json!("0"))), None);
        assert_eq!(parse_reset_timestamp(Some(&json!("NaN"))), None);
        assert_eq!(parse_reset_timestamp(Some(&json!("soon"))), None);
        assert_eq!(parse_reset_timestamp(None), None);
    }

    #[test]
    fn a_fresh_cycle_without_spend_fields_is_zero_percent() {
        // Protobuf JSON omits zero spend; only the allowance remains.
        let payload = json!({
            "billingCycleStart": "1785542400000",
            "billingCycleEnd": "1788220800000",
            "planUsage": { "limit": 2000, "remaining": 2000 }
        });
        let normalized = normalize_usage(&payload).expect("windows");
        assert_eq!(
            normalized.pointer("/rateLimitsByLimitId/plan/primary/usedPercent"),
            Some(&json!(0.0))
        );
        assert!(normalized
            .pointer("/rateLimitsByLimitId/plan/rateLimitReachedType")
            .is_none());
    }

    #[test]
    fn non_finite_percentages_are_ignored() {
        let payload = json!({
            "planUsage": { "totalPercentUsed": "NaN", "totalSpend": 500, "limit": 2000 }
        });
        let normalized = normalize_usage(&payload).expect("windows");
        assert_eq!(
            normalized.pointer("/rateLimitsByLimitId/plan/primary/usedPercent"),
            Some(&json!(25.0))
        );
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
