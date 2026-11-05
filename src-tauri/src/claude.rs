//! Claude Code usage provider.
//!
//! Reads the OAuth session Claude Code already keeps (`~/.claude/.credentials.json`
//! and Keychain item `Claude Code-credentials`) and asks Anthropic's usage
//! endpoint for the same windows as `/usage`. Read-only: the token is never
//! refreshed or rewritten. Background polls never show a Keychain sheet; Retry
//! may prompt once if the file is stale and Always Allow has not been granted.
//!
//! Claude Code writes its Keychain item with `/usr/bin/security`, and rewrites
//! it on every token rotation, which drops any Always Allow granted to
//! UsageBar. Reading through the same tool stays on the item's own access list,
//! so rotations no longer lock the meter out. A cheap attribute-only lookup
//! (no secret, never a sheet) tells the poller when to re-read.

use std::{
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        Arc,
    },
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(any(target_os = "macos", test))]
use std::time::Duration;

use serde_json::{json, Map, Value};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{Mutex, RwLock};

use crate::codex::process::ConnectionState;
use crate::provider::{parse_reset_timestamp, window_snapshot};
use crate::tray::{self};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
#[cfg(target_os = "macos")]
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";
const OAUTH_BETA_HEADER: &str = "oauth-2025-04-20";
/// Consecutive empty reads required before the menu-bar meter disappears, once
/// it has shown a number at least once.
const ABSENT_READS_BEFORE_HIDING: u32 = 2;
/// While Claude Code is in use, the meter refreshes this often instead of
/// waiting out the five-minute idle cadence. Claude has no push channel like
/// the Codex App Server, so recent transcript writes stand in for one.
pub const ACTIVE_REFRESH_SECS: u64 = 60;
/// Opening the popover refreshes a reading older than this.
pub const OPEN_REFRESH_MAX_AGE_SECS: u64 = 30;
/// How often the transcript tree is rescanned in full. Between scans only the
/// most recently written transcript is checked, which keeps the 10-second
/// watchdog tick down to a single `stat`.
const FULL_ACTIVITY_SCAN_SECS: u64 = 30;
/// Bounds for honoring `Retry-After` on HTTP 429. A missing or unparseable
/// header waits out the steady cadence.
const RATE_LIMIT_DEFAULT_SECS: u64 = 4 * 60;
const RATE_LIMIT_MIN_SECS: u64 = 60;
const RATE_LIMIT_MAX_SECS: u64 = 30 * 60;
/// `security` exits with this when the item does not exist.
#[cfg(target_os = "macos")]
const SECURITY_CLI_NOT_FOUND: i32 = 44;
/// A trusted `security` read answers in milliseconds. Anything slower is an
/// access sheet waiting on the user, which a background poll must not leave up.
#[cfg(target_os = "macos")]
const SECURITY_CLI_TIMEOUT: Duration = Duration::from_secs(3);
/// Set when a `security` read was denied or timed out on a sheet, so polls stop
/// trying it until the item changes or the user presses Retry.
#[cfg(target_os = "macos")]
static SECURITY_CLI_BLOCKED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Claude reports the same shape as every other optional provider. The name
/// stays because commands and the demo fixtures refer to it.
pub type ClaudeState = crate::provider::ProviderState;

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
    /// Last seen modification stamp of the stored login (Keychain item and
    /// credentials file), so a token rotation refreshes the meter at once.
    login_stamp: Arc<Mutex<Option<String>>>,
    /// Unix seconds of the last usage request, successful or not, so opening
    /// the popover repeatedly does not re-send a request that just failed.
    last_request: Arc<AtomicU64>,
    /// Unix seconds before which no usage request is sent, set from an HTTP
    /// 429 so a faster cadence never turns into hammering a rate limit.
    rate_limited_until: Arc<AtomicU64>,
    activity: Arc<std::sync::Mutex<ActivityScan>>,
}

impl ClaudeManager {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            state: Arc::new(RwLock::new(crate::demo::initial_claude())),
            client: crate::provider::http_client(),
            refresh_lock: Arc::new(Mutex::new(())),
            absent_reads: Arc::new(AtomicU32::new(0)),
            login_stamp: Arc::new(Mutex::new(None)),
            last_request: Arc::new(AtomicU64::new(0)),
            rate_limited_until: Arc::new(AtomicU64::new(0)),
            activity: Arc::new(std::sync::Mutex::new(ActivityScan::default())),
        }
    }

    /// Whether Claude Code has been used since the meter last landed, so the
    /// watchdog should refresh at the active cadence. Cheap preconditions run
    /// first; the transcript check itself is one `stat` on most ticks.
    pub async fn active_refresh_due(
        &self,
        now: u64,
        updated_at: Option<u64>,
        last_attempt: u64,
        consecutive_failures: u32,
    ) -> bool {
        if crate::demo::enabled()
            || !active_lane_open(now, updated_at, last_attempt, consecutive_failures)
            || now < self.rate_limited_until.load(Ordering::Relaxed)
        {
            return false;
        }
        let Some(projects) = claude_config_dir().map(|dir| dir.join("projects")) else {
            return false;
        };
        let activity = self.activity.clone();
        let latest = tokio::task::spawn_blocking(move || {
            activity
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .latest(&projects, now)
        })
        .await
        .ok()
        .flatten();
        used_since_last_reading(latest, updated_at)
    }

    /// Refreshes only when the reading (or the last attempt) is older than
    /// `max_age_secs`. Used when the popover opens, so the numbers on screen
    /// are current without a request per click.
    pub async fn refresh_if_older_than(&self, max_age_secs: u64) -> Result<ClaudeState, String> {
        self.refresh_inner(false, Some(max_age_secs)).await
    }

    pub async fn snapshot(&self) -> ClaudeState {
        self.state.read().await.clone()
    }

    /// Whether Claude Code rewrote its stored login since the last check. Reads
    /// only attributes and file metadata, so it is cheap and never prompts. The
    /// first call records a baseline and reports no change.
    pub async fn login_changed(&self) -> bool {
        // Demo mode must not touch the keychain or the credentials file.
        if crate::demo::enabled() {
            return false;
        }
        let Ok(stamp) = tokio::task::spawn_blocking(login_stamp).await else {
            return false;
        };
        let mut last = self.login_stamp.lock().await;
        let changed = last.as_ref().is_some_and(|previous| *previous != stamp);
        *last = Some(stamp);
        #[cfg(target_os = "macos")]
        if changed {
            SECURITY_CLI_BLOCKED.store(false, Ordering::Relaxed);
        }
        changed
    }

    pub async fn refresh(&self) -> Result<ClaudeState, String> {
        self.refresh_inner(false, None).await
    }

    /// User-initiated refresh (Retry). May show a Keychain sheet once if the
    /// CLI file is stale and macOS has not already granted Always Allow.
    pub async fn refresh_from_user(&self) -> Result<ClaudeState, String> {
        self.refresh_inner(true, None).await
    }

    async fn refresh_inner(
        &self,
        allow_keychain_prompt: bool,
        max_age_secs: Option<u64>,
    ) -> Result<ClaudeState, String> {
        if crate::demo::enabled() {
            return Ok(self.snapshot().await);
        }
        if !self
            .app
            .state::<crate::prefs::PrefsStore>()
            .get()
            .is_visible(crate::prefs::PROVIDER_CLAUDE)
        {
            return Ok(self.snapshot().await);
        }
        let _guard = self.refresh_lock.lock().await;
        // Checked under the lock: a watchdog refresh that just finished makes
        // a queued popover-open refresh redundant.
        if let Some(max_age) = max_age_secs {
            let newest = self
                .snapshot()
                .await
                .updated_at
                .unwrap_or(0)
                .max(self.last_request.load(Ordering::Relaxed));
            if now_unix_seconds().saturating_sub(newest) < max_age {
                return Ok(self.snapshot().await);
            }
        }
        // Anthropic asked us to back off. The last numbers and the
        // rate-limit diagnostic stay on screen until the wait is over.
        if now_unix_seconds() < self.rate_limited_until.load(Ordering::Relaxed) {
            return Ok(self.snapshot().await);
        }
        let credentials = match load_credentials(allow_keychain_prompt).await {
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
                        Some("No Claude Code login was found on this computer".into()),
                    )
                    .await;
                    return Ok(self.snapshot().await);
                }
                // Claude Code rewrites its keychain item and credentials file
                // when it rotates its own OAuth token, so a single empty read
                // right after a working session is far more likely a write race
                // than a sign-out. Keep the last numbers. Do not show an error.
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
                        Some("No Claude Code login was found on this computer".into()),
                    )
                    .await;
                    return Ok(self.snapshot().await);
                }
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
                Some("Claude Code's login expired. Run `claude` once in a terminal to renew it; the meter picks it up within seconds.".into()),
            )
            .await;
            return Ok(self.snapshot().await);
        }

        self.last_request
            .store(now_unix_seconds(), Ordering::Relaxed);
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
                Some("Anthropic rejected the stored Claude Code token. Run `claude` once in a terminal to renew it; the meter picks it up within seconds.".into()),
            )
            .await;
            return Ok(self.snapshot().await);
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let wait = rate_limit_wait_secs(
                response
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|value| value.to_str().ok()),
            );
            self.rate_limited_until
                .store(now_unix_seconds().saturating_add(wait), Ordering::Relaxed);
            let message = "Anthropic is rate-limiting usage requests right now; UsageBar will try again shortly".to_owned();
            self.set_connection(ConnectionState::Error, Some(message.clone()))
                .await;
            return Err(message);
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
            if connection == ConnectionState::CliNotFound {
                state.account = None;
                state.rate_limits = None;
                state.updated_at = None;
            }
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

/// Reads Claude Code's stored OAuth session without ever modifying it. The
/// credentials file and Keychain are both consulted; a fresh token in either
/// store wins. Background refreshes never show a Keychain sheet. A user Retry
/// may prompt once if the file is stale and Always Allow has not been granted.
async fn load_credentials(allow_keychain_prompt: bool) -> CredentialRead {
    let file = load_from_file().await;
    let keychain = load_from_keychain(allow_keychain_prompt).await;
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

/// Classifies a failing Security.framework lookup, using the same rule as the
/// other Keychain-backed providers (`provider::keychain_login_absent`):
/// item-not-found (-25300), and a background poll whose suppressed sheet
/// macOS reports as a cancel (-128) or as interaction-not-allowed (-25308),
/// all count as no usable login and can hide the meter. Note that -25308 is
/// also what a locked keychain returns while interaction is disabled, so a
/// locked keychain reads as absent here too; `hides_tray` still asks for a
/// confirming second read before a meter that has shown usage disappears.
/// Every other status (an authorization failure, say) is an unknown that
/// keeps the last meter.
#[cfg(any(target_os = "macos", test))]
fn classify_keychain_status(code: i32) -> CredentialRead {
    if crate::provider::keychain_login_absent(code) {
        return CredentialRead::Absent;
    }
    CredentialRead::Unavailable(format!(
        "The Claude Code login could not be read from the keychain (status {code})"
    ))
}

#[cfg(target_os = "macos")]
fn read_keychain_password(allow_prompt: bool) -> CredentialRead {
    // A Retry after a blocked CLI read goes straight to the framework lookup,
    // whose sheet can grant UsageBar its own Always Allow.
    if !(allow_prompt && SECURITY_CLI_BLOCKED.load(Ordering::Relaxed)) {
        if let Some(read) = read_with_security_cli() {
            return read;
        }
    }
    read_with_security_framework(allow_prompt)
}

/// Reads the item the way Claude Code writes it, through `/usr/bin/security`,
/// which the item's access list already trusts. `None` means the tool could not
/// answer without a sheet, so the caller falls back to the framework lookup.
#[cfg(target_os = "macos")]
fn read_with_security_cli() -> Option<CredentialRead> {
    use std::process::{Command, Stdio};
    if SECURITY_CLI_BLOCKED.load(Ordering::Relaxed) {
        return None;
    }
    let mut child = Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < SECURITY_CLI_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(20));
            }
            // Still waiting means an access sheet is up; killing the request
            // takes the sheet down with it.
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                SECURITY_CLI_BLOCKED.store(true, Ordering::Relaxed);
                return None;
            }
        }
    };
    if status.code() == Some(SECURITY_CLI_NOT_FOUND) {
        return Some(CredentialRead::Absent);
    }
    if !status.success() {
        SECURITY_CLI_BLOCKED.store(true, Ordering::Relaxed);
        return None;
    }
    let mut output = String::new();
    std::io::Read::read_to_string(child.stdout.as_mut()?, &mut output).ok()?;
    Some(match parse_security_output(&output) {
        Some(credentials) => CredentialRead::Found(credentials),
        None => CredentialRead::Absent,
    })
}

/// `security -w` prints a printable password as-is and anything else as hex.
#[cfg(any(target_os = "macos", test))]
fn parse_security_output(output: &str) -> Option<Credentials> {
    let trimmed = output.trim();
    parse_credentials(trimmed).or_else(|| {
        let bytes = decode_hex(trimmed)?;
        parse_credentials(&String::from_utf8_lossy(&bytes))
    })
}

#[cfg(any(target_os = "macos", test))]
fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(text.get(index..index + 2)?, 16).ok())
        .collect()
}

/// A stamp that changes whenever Claude Code rewrites its login: the Keychain
/// item's modification date plus the credentials file's mtime. Neither read
/// touches the secret, so it never shows a sheet.
fn login_stamp() -> String {
    let file = credentials_file_path()
        .and_then(|path| std::fs::metadata(path).ok())
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|age| age.as_millis().to_string())
        .unwrap_or_default();
    format!("{}|{file}", keychain_item_stamp())
}

#[cfg(target_os = "macos")]
fn keychain_item_stamp() -> String {
    let Ok(output) = std::process::Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", KEYCHAIN_SERVICE])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
    else {
        return String::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find(|line| line.trim_start().starts_with("\"mdat\""))
        .map(|line| line.trim().to_owned())
        .unwrap_or_default()
}

#[cfg(not(target_os = "macos"))]
fn keychain_item_stamp() -> String {
    String::new()
}

#[cfg(target_os = "macos")]
fn read_with_security_framework(allow_prompt: bool) -> CredentialRead {
    use security_framework::item::{ItemClass, ItemSearchOptions, SearchResult};
    use security_framework::os::macos::keychain::SecKeychain;
    // Background polls must not pop the "Claude Code credentials" sheet.
    // Retry/user refresh leaves interaction on so Always Allow can be granted.
    let _no_prompt = if allow_prompt {
        None
    } else {
        SecKeychain::disable_user_interaction().ok()
    };
    let mut opts = ItemSearchOptions::new();
    opts.class(ItemClass::generic_password())
        .service(KEYCHAIN_SERVICE)
        .load_data(true);
    if !allow_prompt {
        opts.skip_authenticated_items(true);
    }
    match opts.search() {
        Ok(results) => {
            for result in results {
                let SearchResult::Data(bytes) = result else {
                    continue;
                };
                return match parse_credentials(&String::from_utf8_lossy(&bytes)) {
                    Some(credentials) => CredentialRead::Found(credentials),
                    None => CredentialRead::Absent,
                };
            }
            CredentialRead::Absent
        }
        Err(error) => classify_keychain_status(error.code()),
    }
}

#[cfg(not(target_os = "macos"))]
fn read_keychain_password(_allow_prompt: bool) -> CredentialRead {
    CredentialRead::Absent
}

async fn load_from_keychain(allow_prompt: bool) -> CredentialRead {
    match tokio::task::spawn_blocking(move || read_keychain_password(allow_prompt)).await {
        Ok(read) => read,
        Err(error) => {
            CredentialRead::Unavailable(format!("Keychain lookup was cancelled: {error}"))
        }
    }
}

fn claude_config_dir() -> Option<std::path::PathBuf> {
    if let Some(directory) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|value| !value.is_empty())
    {
        return Some(std::path::PathBuf::from(directory));
    }
    Some(crate::platform::home_dir()?.join(".claude"))
}

fn credentials_file_path() -> Option<std::path::PathBuf> {
    Some(claude_config_dir()?.join(".credentials.json"))
}

/// The cheap half of the active-use check: the meter is healthy (failures go
/// through the normal backoff instead), has a reading to compare against,
/// and the active cadence has elapsed since the last attempt.
fn active_lane_open(
    now: u64,
    updated_at: Option<u64>,
    last_attempt: u64,
    consecutive_failures: u32,
) -> bool {
    consecutive_failures == 0
        && updated_at.is_some()
        && now.saturating_sub(last_attempt) >= ACTIVE_REFRESH_SECS
}

/// Whether Claude Code wrote a transcript at or after the last reading. The
/// same second counts: a write that lands while the request is in flight may
/// not be reflected in the numbers yet, and it costs at most one extra fetch.
fn used_since_last_reading(latest_activity: Option<u64>, updated_at: Option<u64>) -> bool {
    match (latest_activity, updated_at) {
        (Some(activity), Some(updated)) => activity >= updated,
        _ => false,
    }
}

/// Seconds to wait after HTTP 429. Only the delta-seconds form of
/// `Retry-After` is read; anything else waits out the steady cadence.
fn rate_limit_wait_secs(retry_after: Option<&str>) -> u64 {
    retry_after
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(RATE_LIMIT_DEFAULT_SECS)
        .clamp(RATE_LIMIT_MIN_SECS, RATE_LIMIT_MAX_SECS)
}

/// Tracks the newest Claude Code transcript write. Claude Code appends to
/// `projects/<project>/<session>.jsonl` throughout every turn, so its mtime is
/// a local, credential-free signal that usage just moved.
#[derive(Default)]
struct ActivityScan {
    /// The most recently written transcript seen by the last full scan.
    hot: Option<std::path::PathBuf>,
    newest: Option<u64>,
    scanned_at: u64,
}

impl ActivityScan {
    fn latest(&mut self, projects: &std::path::Path, now: u64) -> Option<u64> {
        let quick =
            self.hot.is_some() && now.saturating_sub(self.scanned_at) < FULL_ACTIVITY_SCAN_SECS;
        if quick {
            if let Some(modified) = self
                .hot
                .as_deref()
                .and_then(|path| modified_secs(path, now))
            {
                self.newest = self.newest.max(Some(modified));
            }
            return self.newest;
        }
        let (hot, newest) = newest_transcript(projects, now);
        self.hot = hot;
        self.newest = newest;
        self.scanned_at = now;
        self.newest
    }
}

/// A file's mtime in unix seconds. A stamp from the future (clock skew, a
/// restored backup) is ignored, or it would read as activity forever.
fn modified_secs(path: &std::path::Path, now: u64) -> Option<u64> {
    let secs = std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs();
    (secs <= now.saturating_add(5)).then_some(secs)
}

/// The newest `*.jsonl` directly inside each project folder. Subagent
/// transcripts nest deeper, but their parent session file is written too.
fn newest_transcript(
    projects: &std::path::Path,
    now: u64,
) -> (Option<std::path::PathBuf>, Option<u64>) {
    let mut best: (Option<std::path::PathBuf>, Option<u64>) = (None, None);
    let Ok(project_dirs) = std::fs::read_dir(projects) else {
        return best;
    };
    for project in project_dirs.flatten() {
        if !project.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let Ok(files) = std::fs::read_dir(project.path()) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            if path.extension().is_none_or(|ext| ext != "jsonl") {
                continue;
            }
            let Some(modified) = modified_secs(&path, now) else {
                continue;
            };
            if best.1.is_none_or(|newest| modified > newest) {
                best = (Some(path), Some(modified));
            }
        }
    }
    best
}

/// Opening the popover shows current Claude numbers: a reading older than
/// `OPEN_REFRESH_MAX_AGE_SECS` is refreshed in the background, and the
/// renderer picks it up from the `claude://state` event.
pub fn refresh_on_open(app: &AppHandle) {
    let Some(manager) = app
        .try_state::<ClaudeManager>()
        .map(|state| state.inner().clone())
    else {
        return;
    };
    tauri::async_runtime::spawn(async move {
        let _ = manager
            .refresh_if_older_than(OPEN_REFRESH_MAX_AGE_SECS)
            .await;
    });
}

async fn load_from_file() -> CredentialRead {
    let Some(path) = credentials_file_path() else {
        return CredentialRead::Unavailable(
            "The home directory is unavailable, so the Claude Code credentials file could not be located".to_owned(),
        );
    };
    match tokio::fs::read_to_string(&path).await {
        Ok(contents) => match parse_credentials(&contents) {
            Some(credentials) => CredentialRead::Found(credentials),
            None => CredentialRead::Absent,
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => CredentialRead::Absent,
        // Permissions, I/O errors, a half-written file: unknown, not absent.
        Err(error) => {
            CredentialRead::Unavailable(format!("Could not read {}: {error}", path.display()))
        }
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
            let Some(entry) = normalize_limit_entry(limit, session_fallback, weekly_fallback)
            else {
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
            let kind = if key == "five_hour" {
                "primary"
            } else {
                "secondary"
            };
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
    let window_kind = if duration == Some(300.0) {
        "primary"
    } else {
        "secondary"
    };

    let mut id = kind.replace('_', "-");
    if let Some(name) = scope_name {
        id = format!("{id}-{}", name.to_lowercase().replace(' ', "-"));
    }
    // Scoped windows show the scope itself as the tile heading ("Fable"), so
    // the sub-line describes the window instead of repeating the model name.
    let limit_name = if scope_name.is_some() {
        duration.map(|duration| match duration {
            10_080.0 => "Weekly limit".to_owned(),
            1_440.0 => "Daily limit".to_owned(),
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

fn now_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn now_unix_seconds() -> u64 {
    now_unix_millis() / 1_000
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
        assert_eq!(
            scoped.pointer("/secondary/excludeFromTray"),
            Some(&json!(true))
        );

        // The tray picker sees all three windows, labeled for the menu.
        let windows = crate::tray::collect_windows(Some(&normalized));
        let labels: Vec<&str> = windows.iter().map(|window| window.label.as_str()).collect();
        assert_eq!(labels, vec!["5-hour", "Weekly", "Fable"]);
        let auto =
            crate::tray::select_window(&windows, crate::prefs::TRAY_WINDOW_AUTO).expect("auto");
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
        assert_eq!(
            freshest(creds(500.0), creds(2_000.0), 1_000).expires_at_ms,
            Some(2_000.0)
        );
        assert_eq!(
            freshest(creds(2_000.0), creds(500.0), 1_000).expires_at_ms,
            Some(2_000.0)
        );
        // Both expired: the fresher one wins (its message is more accurate).
        assert_eq!(
            freshest(creds(500.0), creds(800.0), 1_000).expires_at_ms,
            Some(800.0)
        );
    }

    #[test]
    fn a_missing_or_suppressed_keychain_login_is_absent() {
        // -25300 item not found, -128 cancelled/suppressed prompt,
        // -25308 interaction not allowed: the user has no usable login.
        for code in [-25300, -128, -25308] {
            assert!(
                matches!(classify_keychain_status(code), CredentialRead::Absent),
                "status {code} must hide the meter"
            );
        }
        // Auth failure is not "this tool is missing".
        assert!(matches!(
            classify_keychain_status(-25293),
            CredentialRead::Unavailable(_)
        ));
        // Claude follows the shared rule exactly, so the two cannot drift.
        for code in [-25300, -25308, -25293, -25291, -128, -34018, 0, 1] {
            assert_eq!(
                matches!(classify_keychain_status(code), CredentialRead::Absent),
                crate::provider::keychain_login_absent(code),
                "status {code}"
            );
        }
    }

    #[test]
    fn an_unreadable_store_never_reports_a_sign_out() {
        let unavailable = || CredentialRead::Unavailable("keychain locked".to_owned());
        // A usable login from either store wins outright.
        assert_eq!(
            expiry(&combine_reads(
                unavailable(),
                CredentialRead::Found(creds(2_000.0)),
                1_000
            )),
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

    #[test]
    fn parses_plain_and_hex_security_output() {
        let json = r#"{"claudeAiOauth":{"accessToken":"abc","expiresAt":1000}}"#;
        let plain = parse_security_output(&format!("{json}\n")).expect("plain");
        assert_eq!(plain.access_token, "abc");

        let hex: String = json.bytes().map(|byte| format!("{byte:02x}")).collect();
        let decoded = parse_security_output(&hex).expect("hex");
        assert_eq!(decoded.access_token, "abc");

        assert!(parse_security_output("").is_none());
        assert!(parse_security_output("abc").is_none());
    }

    #[test]
    fn the_active_lane_needs_a_healthy_meter_and_the_active_spacing() {
        let now = 1_000_000;
        assert!(active_lane_open(
            now,
            Some(now - 90),
            now - ACTIVE_REFRESH_SECS,
            0
        ));
        // Too soon after the last attempt.
        assert!(!active_lane_open(now, Some(now - 90), now - 10, 0));
        // Failures stay in the normal backoff instead of the fast lane.
        assert!(!active_lane_open(now, Some(now - 90), now - 600, 1));
        // Nothing to compare against yet: the startup refresh owns that case.
        assert!(!active_lane_open(now, None, now - 600, 0));
    }

    #[test]
    fn only_a_transcript_write_since_the_last_reading_counts_as_use() {
        assert!(used_since_last_reading(Some(200), Some(100)));
        assert!(used_since_last_reading(Some(100), Some(100)));
        assert!(!used_since_last_reading(Some(99), Some(100)));
        assert!(!used_since_last_reading(None, Some(100)));
        assert!(!used_since_last_reading(Some(200), None));
    }

    #[test]
    fn rate_limit_waits_follow_retry_after_within_bounds() {
        assert_eq!(rate_limit_wait_secs(Some("120")), 120);
        assert_eq!(rate_limit_wait_secs(Some(" 90 ")), 90);
        assert_eq!(rate_limit_wait_secs(Some("1")), RATE_LIMIT_MIN_SECS);
        assert_eq!(rate_limit_wait_secs(Some("86400")), RATE_LIMIT_MAX_SECS);
        assert_eq!(
            rate_limit_wait_secs(Some("Wed, 21 Oct 2026 07:28:00 GMT")),
            RATE_LIMIT_DEFAULT_SECS
        );
        assert_eq!(rate_limit_wait_secs(None), RATE_LIMIT_DEFAULT_SECS);
    }

    #[test]
    fn activity_scan_finds_the_newest_transcript_and_rechecks_it_cheaply() {
        let root = std::env::temp_dir().join(format!(
            "usagebar-activity-{}-{}",
            std::process::id(),
            now_unix_millis()
        ));
        let projects = root.join("projects");
        let alpha = projects.join("-Users-me-alpha");
        let beta = projects.join("-Users-me-beta");
        std::fs::create_dir_all(&alpha).expect("alpha dir");
        std::fs::create_dir_all(beta.join("session").join("subagents")).expect("beta dir");
        let old = alpha.join("old.jsonl");
        let live = beta.join("live.jsonl");
        std::fs::write(&old, "{}\n").expect("old transcript");
        std::fs::write(&live, "{}\n").expect("live transcript");
        std::fs::write(alpha.join("notes.txt"), "ignored").expect("other file");
        let set_mtime = |path: &std::path::Path, secs: u64| {
            std::fs::File::options()
                .append(true)
                .open(path)
                .expect("open")
                .set_modified(UNIX_EPOCH + Duration::from_secs(secs))
                .expect("set mtime");
        };
        set_mtime(&old, 1_000);
        set_mtime(&live, 2_000);

        let now = 10_000;
        let mut scan = ActivityScan::default();
        assert_eq!(scan.latest(&projects, now), Some(2_000));
        assert_eq!(scan.hot.as_deref(), Some(live.as_path()));

        // Between full scans only the hot file is checked.
        set_mtime(&live, 3_000);
        assert_eq!(scan.latest(&projects, now + 1), Some(3_000));
        set_mtime(&old, 4_000);
        assert_eq!(scan.latest(&projects, now + 2), Some(3_000));
        // The next full scan picks up the other project.
        assert_eq!(
            scan.latest(&projects, now + FULL_ACTIVITY_SCAN_SECS),
            Some(4_000)
        );
        assert_eq!(scan.hot.as_deref(), Some(old.as_path()));

        // A stamp from the future is not activity.
        set_mtime(&live, now + 3_600);
        set_mtime(&old, 1_000);
        assert_eq!(newest_transcript(&projects, now).1, Some(1_000));

        // No projects folder (Claude Code never ran) means no activity.
        assert_eq!(newest_transcript(&root.join("missing"), now), (None, None));
        let _ = std::fs::remove_dir_all(&root);
    }
}
