//! Google Antigravity usage provider.
//!
//! The desktop app, the IDE, and the `agy` CLI share one account quota, and
//! each serves it on a local language-server port while it is running. UsageBar
//! asks that port first, so a sign-in done in the Antigravity app is enough.
//!
//! When nothing is running, it falls back to a still-valid access token
//! Antigravity already keeps (Keychain item `gemini` / `antigravity`, then
//! `~/.gemini/antigravity-cli/antigravity-oauth-token`) and asks Cloud Code
//! for the same windows. UsageBar does not refresh or write that token. No
//! running app and no usable token means the meter stays hidden. A missing
//! tool is not an error. Background polls never show a Keychain sheet.

use std::sync::Arc;

use base64::Engine;
use serde_json::{json, Map, Value};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{Mutex, RwLock};

use crate::codex::process::ConnectionState;
use crate::provider::{
    finite_f64, http_client, now_unix_seconds, parse_reset_timestamp, rate_limits_map,
    window_snapshot, ProviderState,
};
use crate::tray;

const QUOTA_URL: &str =
    "https://daily-cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary";
const LOCAL_STATUS_RPC: &str = "exa.language_server_pb.LanguageServerService/GetUserStatus";
const LOCAL_QUOTA_RPC: &str =
    "exa.language_server_pb.LanguageServerService/RetrieveUserQuotaSummary";
const KEYCHAIN_SERVICE: &str = "gemini";
const KEYCHAIN_ACCOUNT: &str = "antigravity";
const KEYRING_BASE64_PREFIX: &[u8] = b"go-keyring-base64:";
const FIVE_HOUR_MINS: f64 = 300.0;
const WEEKLY_MINS: f64 = 10_080.0;
/// Refresh a minute before expiry so a poll does not race the token out.
const EXPIRY_SKEW_SECS: f64 = 60.0;

#[derive(Clone)]
pub struct AntigravityManager {
    app: AppHandle,
    state: Arc<RwLock<ProviderState>>,
    client: reqwest::Client,
    refresh_lock: Arc<Mutex<()>>,
    token_cache: Arc<Mutex<Option<CachedAccessToken>>>,
    last_local_endpoint: Arc<Mutex<Option<LocalEndpoint>>>,
}

#[derive(Clone)]
struct CachedAccessToken {
    access_token: String,
    expires_at: f64,
}

impl AntigravityManager {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            state: Arc::new(RwLock::new(crate::demo::initial_antigravity())),
            client: http_client(),
            refresh_lock: Arc::new(Mutex::new(())),
            token_cache: Arc::new(Mutex::new(None)),
            last_local_endpoint: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn snapshot(&self) -> ProviderState {
        self.state.read().await.clone()
    }

    pub async fn should_poll_local_launch(&self) -> bool {
        if crate::demo::enabled() {
            return false;
        }
        if !self
            .app
            .state::<crate::prefs::PrefsStore>()
            .get()
            .is_visible(crate::prefs::PROVIDER_ANTIGRAVITY)
        {
            return false;
        }
        let Some(newest) = newest_logged_endpoint() else {
            return false;
        };
        self.last_local_endpoint.lock().await.as_ref() != Some(&newest)
    }

    pub async fn refresh(&self) -> Result<ProviderState, String> {
        self.refresh_with_prompt(false).await
    }

    /// User-initiated refresh (Retry). May show a Keychain sheet once.
    pub async fn refresh_from_user(&self) -> Result<ProviderState, String> {
        self.refresh_with_prompt(true).await
    }

    async fn refresh_with_prompt(
        &self,
        allow_keychain_prompt: bool,
    ) -> Result<ProviderState, String> {
        if crate::demo::enabled() {
            return Ok(self.snapshot().await);
        }
        if !self
            .app
            .state::<crate::prefs::PrefsStore>()
            .get()
            .is_visible(crate::prefs::PROVIDER_ANTIGRAVITY)
        {
            return Ok(self.snapshot().await);
        }

        let _guard = self.refresh_lock.lock().await;
        *self.last_local_endpoint.lock().await = newest_logged_endpoint();
        if let Some(local) = fetch_local_usage().await {
            self.publish(&local.plan, local.payload).await?;
            return Ok(self.snapshot().await);
        }

        // The app and the CLI are closed. No login hides the meter. An
        // unreadable store keeps a meter that is already showing, and can
        // still use a cached token. Users do not have to run every tool.
        let stored = load_credentials(allow_keychain_prompt).await;
        let has_cached = self.cached_usable_token().await.is_some();
        let has_shown = self.snapshot().await.updated_at.is_some();
        let access_token = match login_action(stored, has_cached, has_shown) {
            LoginAction::Hide => {
                self.hide_missing().await;
                return Ok(self.snapshot().await);
            }
            LoginAction::KeepLast => return Ok(self.snapshot().await),
            LoginAction::UseCached => match self.cached_usable_token().await {
                Some(token) => token,
                None if has_shown => return Ok(self.snapshot().await),
                None => {
                    self.hide_missing().await;
                    return Ok(self.snapshot().await);
                }
            },
            LoginAction::UseStored(token) => match self.resolve_access_token(&token).await {
                Ok(access_token) => access_token,
                Err(_) => {
                    self.hide_missing().await;
                    return Ok(self.snapshot().await);
                }
            },
        };

        let payload = match self.fetch_quota(&access_token).await {
            Ok(payload) => payload,
            Err(QuotaFetch::Unauthorized) => {
                self.hide_missing().await;
                return Ok(self.snapshot().await);
            }
            Err(QuotaFetch::Failed(message)) => {
                if self.snapshot().await.updated_at.is_none() {
                    self.hide_missing().await;
                    return Ok(self.snapshot().await);
                }
                self.set_connection(ConnectionState::Error, Some(message.clone()))
                    .await;
                return Err(message);
            }
        };

        self.publish("Antigravity", payload).await?;
        Ok(self.snapshot().await)
    }

    async fn publish(&self, plan: &str, payload: Value) -> Result<(), String> {
        let Some(normalized) = normalize_usage(&payload) else {
            let message = "Antigravity did not report any quota windows".to_owned();
            self.set_connection(ConnectionState::Error, Some(message.clone()))
                .await;
            return Err(message);
        };

        {
            let mut state = self.state.write().await;
            state.connection = ConnectionState::Connected;
            state.diagnostic = None;
            state.account = Some(json!({
                "type": "oauth",
                "planType": plan,
            }));
            state.rate_limits = Some(normalized.rate_limits.clone());
            state.updated_at = Some(now_unix_seconds());
        }
        if let Some(alerts) = self.app.try_state::<crate::alerts::UsageAlerts>() {
            alerts
                .observe(&self.app, "Antigravity", &normalized.rate_limits)
                .await;
        }
        self.emit_state().await;
        Ok(())
    }

    async fn resolve_access_token(&self, token: &OAuthToken) -> Result<String, String> {
        let now = now_unix_seconds() as f64;
        {
            let cache = self.token_cache.lock().await;
            if let Some(cached) = cache.as_ref() {
                if cached.expires_at - EXPIRY_SKEW_SECS > now {
                    return Ok(cached.access_token.clone());
                }
            }
        }
        if !token.is_expired(now) {
            self.store_cached_token(token.access_token.clone(), token.expires_at)
                .await;
            return Ok(token.access_token.clone());
        }
        Err("Antigravity token is not usable".to_owned())
    }

    async fn cached_usable_token(&self) -> Option<String> {
        let now = now_unix_seconds() as f64;
        let cache = self.token_cache.lock().await;
        cache.as_ref().and_then(|cached| {
            (cached.expires_at - EXPIRY_SKEW_SECS > now).then(|| cached.access_token.clone())
        })
    }

    /// Drop the meter. No running Antigravity session and no usable token.
    async fn hide_missing(&self) {
        *self.token_cache.lock().await = None;
        {
            let mut state = self.state.write().await;
            crate::provider::conceal_provider(&mut state, None);
        }
        self.emit_state().await;
    }

    async fn store_cached_token(&self, access_token: String, expires_at: Option<f64>) {
        let expires_at = expires_at.unwrap_or(now_unix_seconds() as f64 + 3_600.0);
        *self.token_cache.lock().await = Some(CachedAccessToken {
            access_token,
            expires_at,
        });
    }

    async fn fetch_quota(&self, access_token: &str) -> Result<Value, QuotaFetch> {
        let response = self
            .client
            .post(QUOTA_URL)
            .bearer_auth(access_token)
            .header("Content-Type", "application/json")
            .json(&json!({}))
            .send()
            .await
            .map_err(|error| {
                QuotaFetch::Failed(format!("Antigravity usage request failed: {error}"))
            })?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(QuotaFetch::Unauthorized);
        }
        if !status.is_success() {
            return Err(QuotaFetch::Failed(format!(
                "Antigravity usage endpoint returned HTTP {status}"
            )));
        }
        response.json().await.map_err(|error| {
            QuotaFetch::Failed(format!(
                "Antigravity usage response was not valid JSON: {error}"
            ))
        })
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
        let _ = self.app.emit("antigravity://state", state);
        tray::refresh_unified_tray(&self.app).await;
    }
}

struct LocalUsage {
    plan: String,
    payload: Value,
}

/// Asks a running Antigravity app, IDE, VS Code extension, or `agy` session
/// for quota. The app login never leaves the machine: the language server is
/// already signed in.
async fn fetch_local_usage() -> Option<LocalUsage> {
    let client = local_http_client();
    let logged = logged_endpoints();
    if let Some(usage) = probe_endpoints(&client, &logged).await {
        return Some(usage);
    }
    let fallback: Vec<_> = local_server_endpoints()
        .into_iter()
        .filter(|endpoint| !logged.contains(endpoint))
        .collect();
    probe_endpoints(&client, &fallback).await
}

async fn probe_endpoints(
    client: &reqwest::Client,
    endpoints: &[LocalEndpoint],
) -> Option<LocalUsage> {
    for endpoint in endpoints {
        let csrf = match &endpoint.csrf {
            Some(token) => Some(token.clone()),
            None => fetch_local_csrf(client, &endpoint.base).await,
        };
        let Some(status) =
            post_local_rpc(client, &endpoint.base, csrf.as_deref(), LOCAL_STATUS_RPC).await
        else {
            continue;
        };
        let Some(quota) =
            post_local_rpc(client, &endpoint.base, csrf.as_deref(), LOCAL_QUOTA_RPC).await
        else {
            continue;
        };
        if normalize_usage(&quota).is_none() {
            continue;
        }
        return Some(LocalUsage {
            plan: plan_from_status(&status),
            payload: quota,
        });
    }
    None
}

/// Antigravity's gRPC listener uses a short-lived self-signed certificate.
/// This client is used only with hard-coded loopback hosts discovered from a
/// local Antigravity process; the normal Cloud Code client still validates TLS.
/// Only the Windows probe tries HTTPS listeners, so macOS keeps validation on.
fn local_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_millis(250))
        .timeout(std::time::Duration::from_secs(3))
        .user_agent(concat!("usagebar/", env!("CARGO_PKG_VERSION")))
        .danger_accept_invalid_certs(cfg!(target_os = "windows"))
        .no_proxy()
        .build()
        .expect("local reqwest client construction is infallible with these options")
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LocalEndpoint {
    base: String,
    csrf: Option<String>,
}

async fn fetch_local_csrf(client: &reqwest::Client, base: &str) -> Option<String> {
    let response = client
        .get(base)
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await
        .ok()?;
    let text = response.text().await.ok()?;
    let token = text.split("csrfToken\":\"").nth(1)?.split('"').next()?;
    (!token.is_empty()).then(|| token.to_owned())
}

async fn post_local_rpc(
    client: &reqwest::Client,
    base: &str,
    csrf: Option<&str>,
    rpc: &str,
) -> Option<Value> {
    let mut request = client
        .post(format!("{base}/{rpc}"))
        .header("Content-Type", "application/json")
        .json(&json!({}))
        .timeout(std::time::Duration::from_secs(3));
    if let Some(token) = csrf {
        request = request.header("x-codeium-csrf-token", token);
    }
    let response = request.send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.json().await.ok()
}

fn plan_from_status(value: &Value) -> String {
    value
        .pointer("/userStatus/userTier/name")
        .or_else(|| value.pointer("/userStatus/userTier/description"))
        .or_else(|| value.pointer("/userStatus/planStatus/planInfo/planName"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("Antigravity")
        .to_owned()
}

#[cfg(not(target_os = "windows"))]
fn logged_endpoints() -> Vec<LocalEndpoint> {
    Vec::new()
}

#[cfg(target_os = "windows")]
fn logged_endpoints() -> Vec<LocalEndpoint> {
    windows_logged_endpoints()
}

fn newest_logged_endpoint() -> Option<LocalEndpoint> {
    logged_endpoints().into_iter().next()
}

#[cfg(not(target_os = "windows"))]
fn local_server_endpoints() -> Vec<LocalEndpoint> {
    let Ok(output) = std::process::Command::new("lsof")
        .args(["-nP", "-iTCP", "-sTCP:LISTEN", "-F", "pcn"])
        .output()
    else {
        return Vec::new();
    };
    let Ok(text) = String::from_utf8(output.stdout) else {
        return Vec::new();
    };
    parse_lsof_bases(&text)
        .into_iter()
        .map(|base| LocalEndpoint { base, csrf: None })
        .collect()
}

#[cfg(target_os = "windows")]
fn local_server_endpoints() -> Vec<LocalEndpoint> {
    let ports = windows_listener_ports();
    let mut endpoints = Vec::new();

    // Keep compatibility with the standalone app and older IDE builds that
    // expose the token from their loopback root. Newer IDE and VS Code
    // extension builds are handled above from their private launch logs. HTTPS
    // is tried before HTTP because Antigravity labels its primary RPC listener
    // as HTTPS.
    for port in ports {
        for scheme in ["https", "http"] {
            let endpoint = LocalEndpoint {
                base: format!("{scheme}://127.0.0.1:{port}"),
                csrf: None,
            };
            if !endpoints.contains(&endpoint) {
                endpoints.push(endpoint);
            }
        }
    }
    endpoints
}

#[cfg(target_os = "windows")]
fn windows_listener_ports() -> Vec<u16> {
    // Query listener ownership locally without exposing process arguments.
    use std::os::windows::process::CommandExt;
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command",
            "$ids = @(Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.ProcessName -match '^(language_server.*|antigravity|agy)$' } | Select-Object -ExpandProperty Id); if ($ids.Count -gt 0) { Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue | Where-Object { $ids -contains $_.OwningProcess } | Select-Object -ExpandProperty LocalPort -Unique }"])
        .creation_flags(0x08000000)
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse::<u16>().ok())
        .filter(|port| *port != 0)
        .collect()
}

/// Antigravity IDE 2.5+ and the VS Code Antigravity extension protect local
/// RPC listeners with a CSRF token. Both record that token and the paired HTTP
/// port in their private per-session launch logs under `%APPDATA%`. Reading
/// these is equivalent to reading the token from the old loopback landing
/// page, and it never leaves this process.
#[cfg(target_os = "windows")]
fn windows_logged_endpoints() -> Vec<LocalEndpoint> {
    let Some(app_data) = std::env::var_os("APPDATA") else {
        return Vec::new();
    };
    let mut logs = collect_windows_launch_logs(std::path::Path::new(&app_data));
    logs.sort_by_key(|path| {
        std::fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .ok()
    });
    logs.reverse();

    let mut endpoints = Vec::new();
    for path in logs.into_iter().take(12) {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let Some(endpoint) = parse_language_server_log(&text) else {
            continue;
        };
        if !endpoints.contains(&endpoint) {
            endpoints.push(endpoint);
        }
    }
    endpoints
}

#[cfg(any(target_os = "windows", test))]
fn collect_windows_launch_logs(app_data: &std::path::Path) -> Vec<std::path::PathBuf> {
    const HOSTS: [&str; 5] = [
        "Antigravity IDE",
        "Code",
        "Code - Insiders",
        "Cursor",
        "VSCodium",
    ];
    let mut logs = Vec::new();
    for host in HOSTS {
        let Ok(sessions) = std::fs::read_dir(app_data.join(host).join("logs")) else {
            continue;
        };
        for session in sessions.filter_map(Result::ok) {
            let session_path = session.path();
            let ls_main = session_path.join("ls-main.log");
            if ls_main.is_file() {
                logs.push(ls_main);
            }
            let Ok(entries) = std::fs::read_dir(&session_path) else {
                continue;
            };
            for window in entries.filter_map(Result::ok) {
                if !window.file_name().to_string_lossy().starts_with("window") {
                    continue;
                }
                let Ok(exthost_entries) = std::fs::read_dir(window.path().join("exthost")) else {
                    continue;
                };
                for output_dir in exthost_entries.filter_map(Result::ok) {
                    let dir_name = output_dir.file_name();
                    let dir_name = dir_name.to_string_lossy();
                    if !dir_name.starts_with("output_logging_") && !dir_name.starts_with("google.")
                    {
                        continue;
                    }
                    let Ok(files) = std::fs::read_dir(output_dir.path()) else {
                        continue;
                    };
                    for file in files.filter_map(Result::ok) {
                        let path = file.path();
                        if is_antigravity_extension_log(&path) && path.is_file() {
                            logs.push(path);
                        }
                    }
                }
            }
        }
    }
    logs
}

#[cfg(any(target_os = "windows", test))]
fn is_antigravity_extension_log(path: &std::path::Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let lower = name.to_ascii_lowercase();
    lower == "antigravity.log" || lower.ends_with("-antigravity.log")
}

#[cfg(any(target_os = "windows", test))]
fn parse_language_server_log(text: &str) -> Option<LocalEndpoint> {
    let csrf = text.lines().rev().find_map(parse_log_csrf)?;
    let port = text.lines().rev().find_map(parse_log_http_port)?;
    Some(LocalEndpoint {
        base: format!("http://127.0.0.1:{port}"),
        csrf: Some(csrf),
    })
}

#[cfg(any(target_os = "windows", test))]
fn parse_log_csrf(line: &str) -> Option<String> {
    let mut words = line.split_whitespace();
    while let Some(word) = words.next() {
        if word == "--csrf_token" {
            return words
                .next()
                .filter(|token| !token.is_empty())
                .map(str::to_owned);
        }
        if let Some(token) = word.strip_prefix("--csrf_token=") {
            if !token.is_empty() {
                return Some(token.to_owned());
            }
        }
    }
    None
}

#[cfg(any(target_os = "windows", test))]
fn parse_log_http_port(line: &str) -> Option<u16> {
    if let Some((_, rest)) = line.split_once("listening on random port at ") {
        if rest.contains(" for HTTP") && !rest.contains(" for HTTPS") {
            return rest.split_whitespace().next()?.parse::<u16>().ok();
        }
    }
    if let Some((_, rest)) = line.split_once("Server at http://127.0.0.1:") {
        if rest.contains(" is READY") {
            return rest.split_whitespace().next()?.parse::<u16>().ok();
        }
    }
    let mut words = line.split_whitespace();
    while let Some(word) = words.next() {
        if word == "--hub-port" {
            return words.next()?.parse::<u16>().ok();
        }
        if let Some(port) = word.strip_prefix("--hub-port=") {
            return port.parse::<u16>().ok();
        }
    }
    None
}

fn parse_lsof_bases(output: &str) -> Vec<String> {
    let mut language_server = Vec::new();
    let mut other = Vec::new();
    let mut current = ProcessKind::Other;
    for line in output.lines() {
        let Some(rest) = line.get(1..) else {
            continue;
        };
        match line.as_bytes().first() {
            Some(b'p') => current = ProcessKind::Other,
            Some(b'c') => current = process_kind(rest),
            Some(b'n') => {
                let Some(base) = loopback_base(rest) else {
                    continue;
                };
                let bucket = match current {
                    ProcessKind::LanguageServer => &mut language_server,
                    ProcessKind::Antigravity => &mut other,
                    ProcessKind::Other => continue,
                };
                if !bucket.contains(&base) {
                    bucket.push(base);
                }
            }
            _ => {}
        }
    }
    language_server.append(&mut other);
    language_server
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProcessKind {
    LanguageServer,
    Antigravity,
    Other,
}

fn process_kind(command: &str) -> ProcessKind {
    let command = command.trim().to_ascii_lowercase();
    if command.contains("language_server") {
        ProcessKind::LanguageServer
    } else if command == "agy" || command == "antigravity" {
        ProcessKind::Antigravity
    } else {
        ProcessKind::Other
    }
}

fn loopback_base(address: &str) -> Option<String> {
    let (host, port) = split_host_port(address)?;
    let port: u16 = port.parse().ok()?;
    let host = host.trim_matches(|character| character == '[' || character == ']');
    let loopback = host.is_empty()
        || host == "*"
        || host == "127.0.0.1"
        || host == "::1"
        || host.eq_ignore_ascii_case("localhost");
    loopback.then(|| format!("http://127.0.0.1:{port}"))
}

fn split_host_port(address: &str) -> Option<(&str, &str)> {
    if let Some(rest) = address.strip_prefix('[') {
        let (host, port) = rest.split_once("]:")?;
        return Some((host, port));
    }
    let (host, port) = address.rsplit_once(':')?;
    Some((host, port))
}

enum QuotaFetch {
    Unauthorized,
    Failed(String),
}

struct OAuthToken {
    access_token: String,
    /// Parsed so tests can see the stored shape. UsageBar does not refresh with it.
    #[allow(dead_code)]
    refresh_token: Option<String>,
    expires_at: Option<f64>,
}

impl OAuthToken {
    fn is_expired(&self, now: f64) -> bool {
        match self.expires_at {
            Some(expires_at) => expires_at - EXPIRY_SKEW_SECS <= now,
            None => false,
        }
    }
}

enum CredentialRead {
    Found(OAuthToken),
    Absent,
    Unavailable(String),
}

enum LoginAction {
    Hide,
    KeepLast,
    UseCached,
    UseStored(OAuthToken),
}

/// No stored login hides the meter. An unreadable store keeps the last
/// reading, and prefers a still-valid cached token over hiding.
fn login_action(
    read: CredentialRead,
    has_cached_token: bool,
    has_shown_usage: bool,
) -> LoginAction {
    match read {
        CredentialRead::Found(token) => LoginAction::UseStored(token),
        CredentialRead::Absent => LoginAction::Hide,
        CredentialRead::Unavailable(_) if has_cached_token => LoginAction::UseCached,
        CredentialRead::Unavailable(_) if has_shown_usage => LoginAction::KeepLast,
        CredentialRead::Unavailable(_) => LoginAction::Hide,
    }
}

fn classify_keychain_status(code: i32) -> CredentialRead {
    if crate::provider::keychain_login_absent(code) {
        return CredentialRead::Absent;
    }
    CredentialRead::Unavailable(format!(
        "The Antigravity login could not be read from the keychain (status {code})"
    ))
}

#[cfg(target_os = "macos")]
fn read_keychain_password(allow_prompt: bool) -> CredentialRead {
    use security_framework::item::{ItemClass, ItemSearchOptions, SearchResult};
    use security_framework::os::macos::keychain::SecKeychain;
    let _no_prompt = if allow_prompt {
        None
    } else {
        SecKeychain::disable_user_interaction().ok()
    };
    let mut opts = ItemSearchOptions::new();
    opts.class(ItemClass::generic_password())
        .service(KEYCHAIN_SERVICE)
        .account(KEYCHAIN_ACCOUNT)
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
                return match parse_oauth_bytes(&bytes) {
                    Some(token) => CredentialRead::Found(token),
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

fn token_file_path() -> Option<std::path::PathBuf> {
    let home = crate::platform::home_dir()?;
    Some(std::path::PathBuf::from(home).join(".gemini/antigravity-cli/antigravity-oauth-token"))
}

async fn load_from_file() -> CredentialRead {
    let Some(path) = token_file_path() else {
        return CredentialRead::Unavailable(
            "The home directory is unavailable, so the Antigravity token file could not be located"
                .to_owned(),
        );
    };
    match tokio::fs::read(&path).await {
        Ok(bytes) => match parse_oauth_bytes(&bytes) {
            Some(token) => CredentialRead::Found(token),
            None => CredentialRead::Absent,
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => CredentialRead::Absent,
        Err(error) => {
            CredentialRead::Unavailable(format!("Could not read {}: {error}", path.display()))
        }
    }
}

async fn load_credentials(allow_keychain_prompt: bool) -> CredentialRead {
    let file = load_from_file().await;
    let keychain = load_from_keychain(allow_keychain_prompt).await;
    combine_reads(keychain, file, now_unix_seconds() as f64)
}

fn combine_reads(first: CredentialRead, second: CredentialRead, now: f64) -> CredentialRead {
    match (first, second) {
        (CredentialRead::Found(first), CredentialRead::Found(second)) => {
            CredentialRead::Found(freshest(first, second, now))
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

fn freshest(first: OAuthToken, second: OAuthToken, now: f64) -> OAuthToken {
    match (!first.is_expired(now), !second.is_expired(now)) {
        (true, false) => first,
        (false, true) => second,
        _ => {
            if second.expires_at.unwrap_or(0.0) > first.expires_at.unwrap_or(0.0) {
                second
            } else {
                first
            }
        }
    }
}

fn parse_oauth_bytes(bytes: &[u8]) -> Option<OAuthToken> {
    let payload = decode_keyring_payload(bytes)?;
    parse_oauth_value(&payload)
}

fn decode_keyring_payload(bytes: &[u8]) -> Option<Value> {
    let trimmed = trim_bytes(bytes);
    if let Some(rest) = trimmed.strip_prefix(KEYRING_BASE64_PREFIX) {
        let decoded = decode_base64(rest)?;
        return serde_json::from_slice(&decoded).ok();
    }
    if let Ok(value) = serde_json::from_slice::<Value>(trimmed) {
        return Some(value);
    }
    let decoded = decode_base64(trimmed)?;
    serde_json::from_slice(&decoded).ok()
}

fn decode_base64(input: &[u8]) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(input).ok()?.trim();
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(text))
        .ok()
}

fn trim_bytes(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(0);
    let end = bytes
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map(|index| index + 1)
        .unwrap_or(0);
    &bytes[start..end]
}

fn parse_oauth_value(value: &Value) -> Option<OAuthToken> {
    let token = value.get("token").unwrap_or(value);
    let access_token = token
        .get("access_token")
        .or_else(|| token.get("accessToken"))
        .and_then(Value::as_str)
        .and_then(non_empty)?;
    let refresh_token = token
        .get("refresh_token")
        .or_else(|| token.get("refreshToken"))
        .and_then(Value::as_str)
        .and_then(non_empty);
    let expires_at = token
        .get("expiry")
        .or_else(|| token.get("expires_at"))
        .or_else(|| token.get("expiresAt"))
        .and_then(parse_expiry);
    Some(OAuthToken {
        access_token,
        refresh_token,
        expires_at,
    })
}

fn parse_expiry(value: &Value) -> Option<f64> {
    if let Some(number) = finite_f64(Some(value)) {
        if number > 10_000_000_000.0 {
            return Some(number / 1_000.0);
        }
        return Some(number);
    }
    parse_reset_timestamp(Some(value))
}

fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

struct NormalizedUsage {
    rate_limits: Value,
}

fn normalize_usage(raw: &Value) -> Option<NormalizedUsage> {
    let groups = extract_groups(raw)?;
    let mut entries = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for group in groups {
        let group_meta = classify_group(group);
        let Some(buckets) = group.get("buckets").and_then(Value::as_array) else {
            continue;
        };
        for bucket in buckets {
            if bucket_disabled(bucket) {
                continue;
            }
            let Some(period) = bucket_period(bucket) else {
                continue;
            };
            let Some(remaining) = remaining_fraction(bucket) else {
                continue;
            };
            let (id, limit_name, window_label, duration, kind) = slot_for(&group_meta, period);
            if !seen.insert(id.clone()) {
                continue;
            }
            let used = ((1.0 - remaining) * 100.0).clamp(0.0, 100.0);
            let resets_at = parse_reset_timestamp(
                bucket
                    .get("resetTime")
                    .or_else(|| bucket.get("reset_time"))
                    .or_else(|| bucket.get("resetAt"))
                    .or_else(|| bucket.get("reset_at")),
            );
            entries.push(limit_entry(
                &id,
                &window_label,
                &limit_name,
                kind,
                used,
                duration,
                resets_at,
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

fn extract_groups(raw: &Value) -> Option<&Vec<Value>> {
    raw.get("groups")
        .and_then(Value::as_array)
        .or_else(|| {
            raw.get("response")
                .and_then(|value| value.get("groups"))
                .and_then(Value::as_array)
        })
        .or_else(|| {
            raw.get("summary")
                .and_then(|value| value.get("groups"))
                .and_then(Value::as_array)
        })
}

struct GroupMeta {
    id: String,
    name: String,
}

fn classify_group(group: &Value) -> GroupMeta {
    let bucket_hint = group
        .get("buckets")
        .and_then(Value::as_array)
        .and_then(|buckets| buckets.first())
        .and_then(|bucket| {
            bucket
                .get("bucketId")
                .or_else(|| bucket.get("id"))
                .and_then(Value::as_str)
        })
        .unwrap_or_default();
    let display = group
        .get("displayName")
        .or_else(|| group.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let haystack = format!("{bucket_hint} {display}").to_ascii_lowercase();
    if haystack.contains("argon") {
        return GroupMeta {
            id: "argon".into(),
            name: "Gemini 4 Argon".into(),
        };
    }
    if haystack.contains("gemini") {
        return GroupMeta {
            id: "gemini".into(),
            name: "Gemini".into(),
        };
    }
    if haystack.contains("claude") || haystack.contains("gpt") || haystack.contains("3p") {
        return GroupMeta {
            id: "3p".into(),
            name: "Claude and GPT".into(),
        };
    }
    let slug = display
        .split_whitespace()
        .next()
        .unwrap_or("models")
        .to_ascii_lowercase();
    GroupMeta {
        id: slug,
        name: if display.is_empty() {
            "Antigravity".into()
        } else {
            display.to_owned()
        },
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Period {
    FiveHour,
    Weekly,
}

fn bucket_period(bucket: &Value) -> Option<Period> {
    let haystack = [
        bucket.get("bucketId").and_then(Value::as_str),
        bucket.get("id").and_then(Value::as_str),
        bucket.get("displayName").and_then(Value::as_str),
        bucket.get("name").and_then(Value::as_str),
        bucket.get("window").and_then(Value::as_str),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ")
    .to_ascii_lowercase();
    if haystack.contains("week") || haystack.contains("7d") {
        return Some(Period::Weekly);
    }
    if haystack.contains("5h")
        || haystack.contains("5-hour")
        || haystack.contains("five hour")
        || haystack.contains("session")
        || haystack.contains("hour")
    {
        return Some(Period::FiveHour);
    }
    None
}

fn bucket_disabled(bucket: &Value) -> bool {
    match bucket.get("disabled") {
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_f64().is_some_and(|value| value != 0.0),
        Some(Value::String(text)) => {
            matches!(text.trim().to_ascii_lowercase().as_str(), "true" | "1")
        }
        _ => false,
    }
}

fn remaining_fraction(bucket: &Value) -> Option<f64> {
    for key in ["remainingFraction", "remaining_fraction"] {
        if let Some(value) = finite_f64(bucket.get(key)).filter(valid_fraction) {
            return Some(value);
        }
    }
    let remaining = bucket.get("remaining")?;
    if let Some(value) = finite_f64(remaining.get("remainingFraction"))
        .or_else(|| finite_f64(remaining.get("remaining_fraction")))
        .filter(valid_fraction)
    {
        return Some(value);
    }
    if remaining.get("case").and_then(Value::as_str) == Some("remainingFraction") {
        return finite_f64(remaining.get("value")).filter(valid_fraction);
    }
    None
}

fn valid_fraction(value: &f64) -> bool {
    value.is_finite() && (0.0..=1.0).contains(value)
}

fn slot_for(group: &GroupMeta, period: Period) -> (String, String, String, f64, &'static str) {
    match period {
        Period::FiveHour => (
            format!("{}-5h", group.id),
            group.name.clone(),
            format!("{} 5-hour", group.name),
            FIVE_HOUR_MINS,
            "primary",
        ),
        Period::Weekly => (
            format!("{}-weekly", group.id),
            group.name.clone(),
            format!("{} weekly", group.name),
            WEEKLY_MINS,
            "secondary",
        ),
    }
}

fn limit_entry(
    id: &str,
    window_label: &str,
    limit_name: &str,
    kind: &str,
    used: f64,
    duration: f64,
    resets_at: Option<f64>,
) -> (String, Value) {
    let mut snapshot = Map::new();
    snapshot.insert("limitId".into(), Value::from(id));
    snapshot.insert("windowLabel".into(), Value::from(window_label));
    snapshot.insert("limitName".into(), Value::from(limit_name));
    if used >= 100.0 {
        snapshot.insert("rateLimitReachedType".into(), Value::from("limit_reached"));
    }
    snapshot.insert(
        kind.into(),
        window_snapshot(used, Some(duration), resets_at),
    );
    (id.to_owned(), Value::Object(snapshot))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_wrapped_golang_token_and_rfc3339_expiry() {
        let raw = json!({
            "token": {
                "access_token": "ya29.abc",
                "refresh_token": "1//xyz",
                "expiry": "2099-01-01T00:00:00Z"
            }
        });
        let token = parse_oauth_value(&raw).expect("token");
        assert_eq!(token.access_token, "ya29.abc");
        assert_eq!(token.refresh_token.as_deref(), Some("1//xyz"));
        assert!(token.expires_at.expect("expiry") > 1_000_000_000.0);
        assert!(!token.is_expired(1_700_000_000.0));
    }

    #[test]
    fn reads_flat_token_and_go_keyring_wrapper() {
        let flat = json!({
            "access_token": "abc",
            "refreshToken": "def",
            "expiresAt": 1_773_331_200_000_i64
        });
        let token = parse_oauth_value(&flat).expect("flat");
        assert_eq!(token.access_token, "abc");
        assert_eq!(token.refresh_token.as_deref(), Some("def"));
        assert_eq!(token.expires_at, Some(1_773_331_200.0));

        let json = br#"{"token":{"access_token":"from-b64","refresh_token":"r"}}"#;
        let wrapped = [
            KEYRING_BASE64_PREFIX,
            base64::engine::general_purpose::STANDARD
                .encode(json)
                .as_bytes(),
        ]
        .concat();
        let parsed = parse_oauth_bytes(&wrapped).expect("b64");
        assert_eq!(parsed.access_token, "from-b64");
        assert_eq!(parsed.refresh_token.as_deref(), Some("r"));
    }

    #[test]
    fn empty_or_garbage_credentials_are_absent() {
        assert!(parse_oauth_bytes(b"").is_none());
        assert!(parse_oauth_bytes(b"not-json").is_none());
        assert!(parse_oauth_value(&json!({ "token": { "access_token": "" } })).is_none());
    }

    #[test]
    fn normalizes_gemini_and_third_party_windows() {
        let payload = json!({
            "groups": [
                {
                    "displayName": "Gemini Models",
                    "buckets": [
                        {
                            "bucketId": "gemini-weekly",
                            "window": "weekly",
                            "resetTime": "2026-08-29T12:08:59Z",
                            "remainingFraction": 0.583
                        },
                        {
                            "bucketId": "gemini-5h",
                            "window": "5h",
                            "resetTime": "2026-08-27T15:45:28Z",
                            "remainingFraction": 0.853
                        }
                    ]
                },
                {
                    "displayName": "Claude and GPT models",
                    "buckets": [
                        {
                            "bucketId": "3p-weekly",
                            "displayName": "Weekly Limit Remaining",
                            "remainingFraction": 0.886
                        },
                        {
                            "bucketId": "3p-5h",
                            "window": "5h",
                            "remainingFraction": 1.0
                        }
                    ]
                },
                {
                    "displayName": "Gemini 4 Argon",
                    "buckets": [
                        {
                            "bucketId": "gemini-argon-wtu-weekly",
                            "displayName": "Gemini 4 Argon Weekly Limit",
                            "window": "weekly",
                            "remainingFraction": 0.9
                        }
                    ]
                }
            ]
        });
        let normalized = normalize_usage(&payload).expect("windows");
        let by_id = normalized
            .rate_limits
            .get("rateLimitsByLimitId")
            .and_then(Value::as_object)
            .expect("map");
        assert_eq!(by_id.len(), 5);
        let gemini_5h = by_id["gemini-5h"]
            .pointer("/primary/usedPercent")
            .and_then(Value::as_f64)
            .expect("gemini 5h");
        assert!((gemini_5h - 14.7).abs() < 0.05);
        assert_eq!(
            by_id["gemini-5h"].pointer("/primary/windowDurationMins"),
            Some(&json!(300.0))
        );
        let gemini_weekly = by_id["gemini-weekly"]
            .pointer("/secondary/usedPercent")
            .and_then(Value::as_f64)
            .expect("gemini weekly");
        assert!((gemini_weekly - 41.7).abs() < 0.05);
        assert_eq!(
            by_id["gemini-weekly"].get("windowLabel"),
            Some(&json!("Gemini weekly"))
        );
        assert_eq!(
            by_id["3p-5h"].pointer("/primary/usedPercent"),
            Some(&json!(0.0))
        );
        assert_eq!(
            by_id["3p-weekly"].get("limitName"),
            Some(&json!("Claude and GPT"))
        );
        let argon_weekly = by_id["argon-weekly"]
            .pointer("/secondary/usedPercent")
            .and_then(Value::as_f64)
            .expect("argon weekly");
        assert!((argon_weekly - 10.0).abs() < 0.05);
        assert_eq!(
            by_id["argon-weekly"].get("limitName"),
            Some(&json!("Gemini 4 Argon"))
        );
        let windows = crate::tray::collect_windows(Some(&normalized.rate_limits));
        let labels: Vec<&str> = windows.iter().map(|window| window.label.as_str()).collect();
        assert_eq!(
            labels,
            vec![
                "Claude and GPT 5-hour",
                "Gemini 5-hour",
                "Claude and GPT weekly",
                "Gemini 4 Argon weekly",
                "Gemini weekly"
            ]
        );
    }

    #[test]
    fn reads_nested_groups_and_remaining_shapes() {
        let payload = json!({
            "response": {
                "groups": [
                    {
                        "name": "Gemini",
                        "buckets": [
                            {
                                "id": "gemini-5h",
                                "remaining": { "case": "remainingFraction", "value": 0.4 },
                                "reset_time": "2026-08-27T15:45:28Z"
                            },
                            {
                                "bucketId": "weekly-pool",
                                "window": "7d",
                                "remaining": { "remaining_fraction": 0.25 }
                            }
                        ]
                    }
                ]
            }
        });
        let normalized = normalize_usage(&payload).expect("windows");
        let by_id = normalized
            .rate_limits
            .get("rateLimitsByLimitId")
            .and_then(Value::as_object)
            .expect("map");
        assert_eq!(
            by_id["gemini-5h"].pointer("/primary/usedPercent"),
            Some(&json!(60.0))
        );
        assert_eq!(
            by_id["gemini-weekly"].pointer("/secondary/usedPercent"),
            Some(&json!(75.0))
        );
    }

    #[test]
    fn skips_disabled_buckets_and_rejects_empty_payloads() {
        let payload = json!({
            "summary": {
                "groups": [
                    {
                        "displayName": "Gemini Models",
                        "buckets": [
                            {
                                "bucketId": "gemini-5h",
                                "window": "5h",
                                "disabled": true,
                                "remainingFraction": 0.1
                            }
                        ]
                    }
                ]
            }
        });
        assert!(normalize_usage(&payload).is_none());
        assert!(normalize_usage(&json!({})).is_none());
        assert!(normalize_usage(&json!({ "groups": [] })).is_none());
    }

    #[test]
    fn marks_a_fully_spent_window() {
        let payload = json!({
            "groups": [{
                "displayName": "Gemini Models",
                "buckets": [{
                    "bucketId": "gemini-weekly",
                    "window": "weekly",
                    "remainingFraction": 0.0
                }]
            }]
        });
        let normalized = normalize_usage(&payload).expect("windows");
        let weekly = &normalized.rate_limits["rateLimitsByLimitId"]["gemini-weekly"];
        assert_eq!(
            weekly.pointer("/secondary/usedPercent"),
            Some(&json!(100.0))
        );
        assert_eq!(
            weekly.get("rateLimitReachedType"),
            Some(&json!("limit_reached"))
        );
    }

    #[test]
    fn local_ports_prefer_the_language_server_and_skip_other_apps() {
        let output = "\
p100
cGoogle Chrome
n127.0.0.1:9222
p200
cAntigravity
n127.0.0.1:53111
n192.168.1.8:53112
p300
clanguage_server
n127.0.0.1:43123
n[::1]:43124
";
        assert_eq!(
            parse_lsof_bases(output),
            vec![
                "http://127.0.0.1:43123".to_owned(),
                "http://127.0.0.1:43124".to_owned(),
                "http://127.0.0.1:53111".to_owned(),
            ]
        );
    }

    #[test]
    fn reads_the_current_http_port_and_csrf_from_an_ide_launch_log() {
        let log = r#"
2026-10-20 [info] [LS Main] Args: --csrf_token secret-value --extension_server_port 53570
server.go:561] Language server listening on random port at 53572 for HTTPS (gRPC)
server.go:569] Language server listening on random port at 53573 for HTTP
"#;
        assert_eq!(
            parse_language_server_log(log),
            Some(LocalEndpoint {
                base: "http://127.0.0.1:53573".to_owned(),
                csrf: Some("secret-value".to_owned()),
            })
        );
    }

    #[test]
    fn reads_the_hub_port_and_csrf_from_a_vscode_extension_log() {
        let log = r#"
[INSTALL] Installed binary is valid (actual version 1.4.5 >= target 1.4.5). Skipping download.
[LAUNCH] Spawning C:\Users\Asus\.gemini\bin\agy.exe (v1.4.5) --hub --hub-port=54551 --app_data_dir=antigravity --csrf_token=9e14ea38-658b-4c1d-bd00-a0be303eadea --add-dir=d:\VS Code\AI_Usage_Meter
[LAUNCH] Server at http://127.0.0.1:54551 is READY after 3 attempt(s).
"#;
        assert_eq!(
            parse_language_server_log(log),
            Some(LocalEndpoint {
                base: "http://127.0.0.1:54551".to_owned(),
                csrf: Some("9e14ea38-658b-4c1d-bd00-a0be303eadea".to_owned()),
            })
        );
    }

    #[test]
    fn collects_launch_logs_from_vscode_and_antigravity_ide() {
        let temp =
            std::env::temp_dir().join(format!("usagebar-antigravity-logs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        let ide_dir = temp.join("Antigravity IDE/logs/20261022T100000");
        let vscode_dir =
            temp.join("Code/logs/20261022T104121/window1/exthost/output_logging_20261022T105348");
        std::fs::create_dir_all(&ide_dir).expect("ide dir");
        std::fs::create_dir_all(&vscode_dir).expect("vscode dir");
        let ide_log = ide_dir.join("ls-main.log");
        let vscode_log = vscode_dir.join("2-Antigravity.log");
        let ignored_log = vscode_dir.join("1-Antigravity LS.log");
        std::fs::write(&ide_log, "ide").expect("write ide");
        std::fs::write(&vscode_log, "vscode").expect("write vscode");
        std::fs::write(&ignored_log, "ignored").expect("write ignored");

        let found = collect_windows_launch_logs(&temp);
        assert!(found.contains(&ide_log));
        assert!(found.contains(&vscode_log));
        assert!(!found.contains(&ignored_log));
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn ignores_incomplete_ide_launch_logs() {
        assert_eq!(
            parse_language_server_log("Language server listening on random port at 53573 for HTTP"),
            None
        );
        assert_eq!(
            parse_language_server_log("Args: --csrf_token secret-value"),
            None
        );
    }

    #[test]
    fn plan_name_comes_from_the_local_status() {
        let status = json!({
            "userStatus": { "userTier": { "name": "Google AI Pro" } }
        });
        assert_eq!(plan_from_status(&status), "Google AI Pro");
        assert_eq!(plan_from_status(&json!({})), "Antigravity");
    }

    #[test]
    fn a_suppressed_keychain_prompt_counts_as_no_login() {
        for code in [-25300, -128, -25308] {
            assert!(
                matches!(classify_keychain_status(code), CredentialRead::Absent),
                "status {code} must hide the meter"
            );
        }
        assert!(matches!(
            classify_keychain_status(-25293),
            CredentialRead::Unavailable(_)
        ));
    }

    fn stored_token() -> OAuthToken {
        OAuthToken {
            access_token: "ya29".into(),
            refresh_token: None,
            expires_at: Some(9_999_999_999.0),
        }
    }

    #[test]
    fn an_unreadable_antigravity_store_keeps_a_live_meter() {
        assert!(matches!(
            login_action(CredentialRead::Absent, true, true),
            LoginAction::Hide
        ));
        assert!(matches!(
            login_action(CredentialRead::Unavailable("busy".into()), false, false),
            LoginAction::Hide
        ));
        assert!(matches!(
            login_action(CredentialRead::Unavailable("busy".into()), true, true),
            LoginAction::UseCached
        ));
        assert!(matches!(
            login_action(CredentialRead::Unavailable("busy".into()), false, true),
            LoginAction::KeepLast
        ));
        assert!(matches!(
            login_action(CredentialRead::Found(stored_token()), false, false),
            LoginAction::UseStored(_)
        ));
    }
}
