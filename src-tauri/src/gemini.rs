//! Gemini CLI usage provider.
//!
//! The `gemini` CLI keeps an OAuth session in `~/.gemini/oauth_creds.json`.
//! UsageBar reads that session and asks Google's Cloud Code Assist API — the
//! same service the CLI itself calls — for the per-model quota buckets. The
//! token is never refreshed or written, and it is sent only to
//! `cloudcode-pa.googleapis.com`. When it expires, running `gemini` once
//! rewrites the file; UsageBar stays read-only because rotating the token
//! could invalidate the CLI's session.
//!
//! `~/.gemini/settings.json` can pin an auth type. `api-key` and
//! `vertex-ai` logins have no OAuth session for UsageBar to read, so the
//! meter stays hidden for those; an absent file or unknown type still tries
//! the OAuth file, which is also how a fresh `gemini` login is found.
//!
//! Google stopped serving this OAuth path for consumer (individual, AI Pro,
//! Ultra) accounts in June 2026 — those get an unsupported-client reply, and
//! the Antigravity provider covers them. The meter here exists for
//! Workspace/Standard/Enterprise logins where the endpoint still answers.

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

const QUOTA_URL: &str =
    "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota";
const CODE_ASSIST_URL: &str =
    "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist";
/// Refresh a minute before expiry so a poll does not race the token out.
const EXPIRY_SKEW_SECS: f64 = 60.0;

#[derive(Clone)]
pub struct GeminiManager {
    app: AppHandle,
    state: Arc<RwLock<ProviderState>>,
    client: reqwest::Client,
    refresh_lock: Arc<Mutex<()>>,
    token_cache: Arc<Mutex<Option<CachedAccessToken>>>,
}

#[derive(Clone)]
struct CachedAccessToken {
    access_token: String,
    expires_at: f64,
}

/// The CLI's stored OAuth session, or why there is none usable.
enum CredentialRead {
    Found(OAuthToken),
    /// The auth type is one that keeps no OAuth session (api-key, vertex-ai),
    /// or the file is absent — either way the meter hides.
    Absent,
    Unavailable(String),
}

struct OAuthToken {
    access_token: String,
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

impl GeminiManager {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            state: Arc::new(RwLock::new(ProviderState::default())),
            client: http_client(),
            refresh_lock: Arc::new(Mutex::new(())),
            token_cache: Arc::new(Mutex::new(None)),
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
            .is_visible(crate::prefs::PROVIDER_GEMINI)
        {
            return Ok(self.snapshot().await);
        }

        let _guard = self.refresh_lock.lock().await;

        let has_cached = self.cached_usable_token().await.is_some();
        let has_shown = self.snapshot().await.updated_at.is_some();
        let read = load_credentials().await;
        // An unreadable store leaves a note for the state snapshot even though
        // the meter hides; the missing-tools UI keys off the stored login name.
        let hide_reason = match &read {
            CredentialRead::Unavailable(reason) => Some(reason.clone()),
            _ => None,
        };
        let access_token = match login_action(read, has_cached, has_shown) {
            LoginAction::Hide => {
                self.hide_missing(hide_reason).await;
                return Ok(self.snapshot().await);
            }
            LoginAction::KeepLast => return Ok(self.snapshot().await),
            LoginAction::UseCached => match self.cached_usable_token().await {
                Some(token) => token,
                None if has_shown => return Ok(self.snapshot().await),
                None => {
                    self.hide_missing(None).await;
                    return Ok(self.snapshot().await);
                }
            },
            LoginAction::UseStored(token) => {
                match self.resolve_access_token(&token).await {
                    Ok(access_token) => access_token,
                    // An expired file stays expired until the CLI runs and
                    // rewrites it — UsageBar never refreshes it itself.
                    Err(_) => {
                        // Keep the last reading — the stale marker admits the
                        // token aged out — but a never-shown meter still hides.
                        if !has_shown {
                            self.hide_missing(None).await;
                        }
                        return Ok(self.snapshot().await);
                    }
                }
            }
        };

        // Project and tier are nice-to-have; quota answers without them.
        let identity = self.load_code_assist(&access_token).await.unwrap_or_default();

        match self.fetch_quota(&access_token, identity.project.as_deref()).await {
            Ok(payload) => self.publish(identity.plan.as_deref(), &payload).await?,
            Err(QuotaFetch::Unauthorized) => {
                // Includes the consumer-tier shutdown signal: this Mac's
                // Gemini login is one the Antigravity provider covers instead.
                self.hide_missing(None).await;
                return Ok(self.snapshot().await);
            }
            Err(QuotaFetch::Failed(message)) => {
                if self.snapshot().await.updated_at.is_none() {
                    self.hide_missing(None).await;
                    return Ok(self.snapshot().await);
                }
                self.set_connection(ConnectionState::Error, Some(message.clone()))
                    .await;
                return Err(message);
            }
        }
        Ok(self.snapshot().await)
    }

    async fn publish(&self, plan: Option<&str>, payload: &Value) -> Result<(), String> {
        let Some(normalized) = normalize_quota(payload) else {
            let message = "Gemini CLI did not report any quota windows".to_owned();
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
                "planType": plan.unwrap_or("Gemini CLI"),
            }));
            state.rate_limits = Some(normalized.rate_limits.clone());
            state.updated_at = Some(now_unix_seconds());
        }
        if let Some(alerts) = self.app.try_state::<crate::alerts::UsageAlerts>() {
            alerts.observe(&self.app, "Gemini", &normalized.rate_limits).await;
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
            let expires_at = token
                .expires_at
                .unwrap_or(now + 3_600.0);
            *self.token_cache.lock().await = Some(CachedAccessToken {
                access_token: token.access_token.clone(),
                expires_at,
            });
            return Ok(token.access_token.clone());
        }
        Err("The Gemini CLI login has expired; run `gemini` once to renew it".to_owned())
    }

    async fn cached_usable_token(&self) -> Option<String> {
        let now = now_unix_seconds() as f64;
        let cache = self.token_cache.lock().await;
        cache.as_ref().and_then(|cached| {
            (cached.expires_at - EXPIRY_SKEW_SECS > now).then(|| cached.access_token.clone())
        })
    }

    /// Drop the meter. No OAuth session, or Google stopped serving it.
    async fn hide_missing(&self, diagnostic: Option<String>) {
        *self.token_cache.lock().await = None;
        {
            let mut state = self.state.write().await;
            crate::provider::conceal_provider(&mut state, diagnostic);
        }
        self.emit_state().await;
    }

    async fn load_code_assist(&self, access_token: &str) -> Option<CodeAssistIdentity> {
        let response = self
            .client
            .post(CODE_ASSIST_URL)
            .bearer_auth(access_token)
            .header("Content-Type", "application/json")
            .json(&json!({
                "metadata": { "ideType": "GEMINI_CLI", "pluginType": "GEMINI" }
            }))
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        let body: Value = response.json().await.ok()?;
        Some(CodeAssistIdentity {
            project: first_string(
                &body,
                &["cloudaicompanionProject", "cloudaicompanion_project", "project"],
            ),
            plan: plan_label(&body),
        })
    }

    async fn fetch_quota(
        &self,
        access_token: &str,
        project: Option<&str>,
    ) -> Result<Value, QuotaFetch> {
        let response = self
            .client
            .post(QUOTA_URL)
            .bearer_auth(access_token)
            .header("Content-Type", "application/json")
            .json(&json!({ "project": project.unwrap_or(" ") }))
            .send()
            .await
            .map_err(|error| {
                QuotaFetch::Failed(format!("Gemini quota request failed: {error}"))
            })?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(QuotaFetch::Unauthorized);
        }
        if !status.is_success() {
            return Err(QuotaFetch::Failed(format!(
                "Gemini quota endpoint returned HTTP {status}"
            )));
        }
        response.json().await.map_err(|error| {
            QuotaFetch::Failed(format!("Gemini quota response was not valid JSON: {error}"))
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
        let _ = self.app.emit("gemini://state", state);
        tray::refresh_unified_tray(&self.app).await;
    }
}

#[derive(Default)]
struct CodeAssistIdentity {
    project: Option<String>,
    plan: Option<String>,
}

enum QuotaFetch {
    Unauthorized,
    Failed(String),
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

fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}

fn credentials_path() -> Option<std::path::PathBuf> {
    Some(home_dir()?.join(".gemini/oauth_creds.json"))
}

fn settings_path() -> Option<std::path::PathBuf> {
    Some(home_dir()?.join(".gemini/settings.json"))
}

/// The auth modes that never keep an OAuth session for UsageBar to read.
fn oauth_supported(settings: &Value) -> bool {
    let auth = settings
        .pointer("/security/auth/selectedType")
        .and_then(Value::as_str)
        .map(str::trim)
        .map(str::to_ascii_lowercase);
    !matches!(auth.as_deref(), Some("api-key") | Some("vertex-ai"))
}

async fn load_credentials() -> CredentialRead {
    let Some(path) = settings_path() else {
        return CredentialRead::Unavailable("HOME is not set".to_owned());
    };
    if let Ok(bytes) = tokio::fs::read(&path).await {
        if let Ok(settings) = serde_json::from_slice::<Value>(&bytes) {
            if !oauth_supported(&settings) {
                return CredentialRead::Absent;
            }
        }
    }

    let Some(path) = credentials_path() else {
        return CredentialRead::Unavailable("HOME is not set".to_owned());
    };
    match tokio::fs::read(&path).await {
        Ok(bytes) => match parse_oauth_value(&bytes) {
            Some(token) => CredentialRead::Found(token),
            None => CredentialRead::Absent,
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => CredentialRead::Absent,
        Err(error) => {
            CredentialRead::Unavailable(format!("Could not read {}: {error}", path.display()))
        }
    }
}

/// `oauth_creds.json` is plain JSON: `access_token`, `refresh_token`,
/// `id_token`, `expiry_date` (ms). UsageBar reads only the access token.
fn parse_oauth_value(bytes: &[u8]) -> Option<OAuthToken> {
    let value: Value = serde_json::from_slice(bytes).ok()?;
    let access_token = first_string(&value, &["access_token", "accessToken"])?;
    let expires_at = first_expiry(&value);
    Some(OAuthToken {
        access_token,
        expires_at,
    })
}

fn first_string(value: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(text) = value.get(*key).and_then(Value::as_str) {
            let text = text.trim();
            if !text.is_empty() {
                return Some(text.to_owned());
            }
        }
    }
    None
}

fn first_expiry(value: &Value) -> Option<f64> {
    for key in ["expiry_date", "expiryDate", "expires_at", "expiresAt", "expiry"] {
        if let Some(number) = finite_f64(value.get(key)) {
            // Millisecond epochs are bigger than any second timestamp this app
            // will live to see; normalize to seconds.
            if number > 10_000_000_000.0 {
                return Some(number / 1_000.0);
            }
            return Some(number);
        }
        if let Some(text) = value.get(key).and_then(Value::as_str) {
            if let Some(parsed) = parse_reset_timestamp(value.get(key)) {
                return Some(parsed);
            }
            if let Ok(number) = text.trim().parse::<f64>() {
                return Some(if number > 10_000_000_000.0 {
                    number / 1_000.0
                } else {
                    number
                });
            }
        }
    }
    None
}

/// Human plan name: Google's `paidTier.name` wins, then the tier id.
fn plan_label(body: &Value) -> Option<String> {
    if let Some(name) = body
        .pointer("/paidTier")
        .and_then(|tier| first_string(tier, &["name", "id"]))
    {
        return Some(name);
    }
    let tier = body
        .pointer("/currentTier/id")
        .or_else(|| body.pointer("/currentTier"))
        .and_then(Value::as_str)
        .or_else(|| {
            body.pointer("/allowedTiers")
                .and_then(Value::as_array)
                .and_then(|tiers| tiers.first())
                .and_then(|tier| {
                    tier.get("id")
                        .or_else(|| tier.get("name"))
                        .and_then(Value::as_str)
                })
        })?;
    Some(
        match tier.trim().to_ascii_lowercase().as_str() {
            "free-tier" | "free" => "Free",
            "standard-tier" | "standard" => "Paid",
            "enterprise-tier" | "enterprise" => "Enterprise",
            "legacy-tier" | "legacy" => "Legacy",
            other => other,
        }
        .to_owned(),
    )
}

struct NormalizedUsage {
    rate_limits: Value,
}

/// `retrieveUserQuota` answers with one `buckets` entry per model, each with
/// `modelId`, `remainingFraction`, and `resetTime`. Models group into Pro,
/// Flash, and Other families; the family meter mirrors the worst bucket, and
/// its reset is that bucket's own reset time.
fn normalize_quota(raw: &Value) -> Option<NormalizedUsage> {
    let buckets = raw
        .get("buckets")
        .and_then(Value::as_array)
        .or_else(|| {
            raw.get("response")
                .and_then(|value| value.get("buckets"))
                .and_then(Value::as_array)
        })?;

    let mut families: std::collections::BTreeMap<&'static str, Family> =
        std::collections::BTreeMap::new();
    for bucket in buckets {
        if bucket_disabled(bucket) {
            continue;
        }
        let Some(remaining) = remaining_fraction(bucket) else {
            continue;
        };
        let model = bucket
            .get("modelId")
            .or_else(|| bucket.get("model_id"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        let key = if model.contains("pro") {
            "pro"
        } else if model.contains("flash") || model.contains("lite") {
            "flash"
        } else {
            "models"
        };
        let resets_at = parse_reset_timestamp(
            bucket
                .get("resetTime")
                .or_else(|| bucket.get("reset_time"))
                .or_else(|| bucket.get("resetAt")),
        );
        let family = families.entry(key).or_insert(Family {
            used: 0.0,
            resets_at: None,
        });
        // Round to two decimals: (1 - 0.9) * 100 lands on 9.999… otherwise.
        let used = ((1.0 - remaining) * 10_000.0).round() / 100.0;
        if used > family.used {
            family.used = used;
            family.resets_at = resets_at;
        }
    }
    if families.is_empty() {
        return None;
    }

    let entries = families
        .into_iter()
        .map(|(id, family)| {
            let (label, name, kind) = match id {
                "pro" => ("Pro models", "Gemini Pro", "primary"),
                "flash" => ("Flash models", "Gemini Flash", "secondary"),
                _ => ("Models", "Gemini models", "secondary"),
            };
            let mut snapshot = Map::new();
            snapshot.insert("limitId".into(), Value::from(id));
            snapshot.insert("windowLabel".into(), Value::from(label));
            snapshot.insert("limitName".into(), Value::from(name));
            if family.used >= 100.0 {
                snapshot.insert("rateLimitReachedType".into(), Value::from("limit_reached"));
            }
            snapshot.insert(
                kind.into(),
                window_snapshot(family.used, None, family.resets_at),
            );
            (id.to_owned(), Value::Object(snapshot))
        })
        .collect();
    Some(NormalizedUsage {
        rate_limits: rate_limits_map(entries),
    })
}

struct Family {
    used: f64,
    resets_at: Option<f64>,
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
    for key in ["remainingFraction", "remaining_fraction", "remaining"] {
        if let Some(value) = finite_f64(bucket.get(key)).filter(|v| v.is_finite() && (0.0..=1.0).contains(v)) {
            return Some(value);
        }
    }
    // Counts form: remainingAmount / quotaAmount.
    let remaining = finite_f64(
        bucket
            .get("remainingAmount")
            .or_else(|| bucket.get("remaining_amount")),
    )?;
    let total = finite_f64(
        bucket
            .get("quotaAmount")
            .or_else(|| bucket.get("quota_amount")),
    )?;
    (total > 0.0).then(|| remaining / total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_oauth_creds_with_ms_expiry() {
        let token = parse_oauth_value(
            br#"{"access_token":"ya29.abc","refresh_token":"1//x","expiry_date":1773331200000}"#,
        )
        .expect("token");
        assert_eq!(token.access_token, "ya29.abc");
        assert_eq!(token.expires_at, Some(1_773_331_200.0));
        assert!(!token.is_expired(1_700_000_000.0));
    }

    #[test]
    fn parses_expired_and_rfc3339_expiries() {
        let expired = parse_oauth_value(
            br#"{"access_token":"t","expiry_date":1700000000000}"#,
        )
        .expect("token");
        assert!(expired.is_expired(1_800_000_000.0));
        let iso = parse_oauth_value(
            br#"{"access_token":"t","expiry":"2099-01-01T00:00:00Z"}"#,
        )
        .expect("token");
        assert!(!iso.is_expired(1_700_000_000.0));
        assert!(parse_oauth_value(b"{}").is_none());
        assert!(parse_oauth_value(b"").is_none());
    }

    #[test]
    fn api_key_and_vertex_logins_have_no_oauth_meter() {
        assert!(!oauth_supported(
            &json!({ "security": { "auth": { "selectedType": "api-key" } } })
        ));
        assert!(!oauth_supported(
            &json!({ "security": { "auth": { "selectedType": "vertex-ai" } } })
        ));
        assert!(oauth_supported(
            &json!({ "security": { "auth": { "selectedType": "oauth-personal" } } })
        ));
        // Missing or future types still try the OAuth file.
        assert!(oauth_supported(&json!({})));
        assert!(oauth_supported(
            &json!({ "security": { "auth": { "selectedType": "compute-adc" } } })
        ));
    }

    #[test]
    fn normalizes_model_buckets_into_families() {
        let payload = json!({
            "buckets": [
                { "modelId": "gemini-2.5-pro", "remainingFraction": 0.42, "resetTime": "2026-10-08T00:00:00Z" },
                { "modelId": "gemini-3-pro-preview", "remainingFraction": 0.1, "resetTime": "2026-10-07T18:00:00Z" },
                { "modelId": "gemini-2.5-flash", "remainingFraction": 0.9, "resetTime": "2026-10-08T02:00:00Z" },
                { "modelId": "gemini-2.5-flash-lite", "remainingFraction": 0.95 },
                { "modelId": "deep-research", "remainingFraction": 0.5 }
            ]
        });
        let normalized = normalize_quota(&payload).expect("families");
        let by_id = normalized
            .rate_limits
            .get("rateLimitsByLimitId")
            .and_then(Value::as_object)
            .expect("map");
        assert_eq!(by_id.len(), 3);
        // Pro mirrors the worst bucket, including its own reset time.
        assert_eq!(
            by_id["pro"].pointer("/primary/usedPercent"),
            Some(&json!(90.0))
        );
        assert_eq!(
            by_id["pro"].get("windowLabel"),
            Some(&json!("Pro models"))
        );
        assert_eq!(
            by_id["flash"].pointer("/secondary/usedPercent"),
            Some(&json!(10.0))
        );
        assert_eq!(
            by_id["models"].pointer("/secondary/usedPercent"),
            Some(&json!(50.0))
        );
        assert!(normalize_quota(&json!({})).is_none());
        assert!(normalize_quota(&json!({ "buckets": [] })).is_none());
    }

    #[test]
    fn a_full_bucket_marks_the_limit_reached() {
        let payload = json!({
            "buckets": [
                { "modelId": "gemini-2.5-pro", "remainingFraction": 0.0 }
            ]
        });
        let normalized = normalize_quota(&payload).expect("families");
        let pro = &normalized.rate_limits["rateLimitsByLimitId"]["pro"];
        assert_eq!(
            pro.pointer("/primary/usedPercent"),
            Some(&json!(100.0))
        );
        assert_eq!(
            pro.get("rateLimitReachedType"),
            Some(&json!("limit_reached"))
        );
    }

    #[test]
    fn disabled_and_unknown_shapes_are_skipped() {
        let payload = json!({
            "buckets": [
                { "modelId": "gemini-2.5-pro", "remainingFraction": 0.2, "disabled": true },
                { "modelId": "gemini-2.5-flash", "remainingAmount": 8, "quotaAmount": 10 }
            ]
        });
        let normalized = normalize_quota(&payload).expect("families");
        let by_id = normalized
            .rate_limits
            .get("rateLimitsByLimitId")
            .and_then(Value::as_object)
            .expect("map");
        assert_eq!(by_id.len(), 1);
        assert_eq!(
            by_id["flash"].pointer("/secondary/usedPercent"),
            Some(&json!(20.0))
        );
    }

    #[test]
    fn plan_names_come_from_paid_tier_then_tier_id() {
        assert_eq!(
            plan_label(&json!({ "paidTier": { "name": "Google AI Pro" } })),
            Some("Google AI Pro".to_owned())
        );
        assert_eq!(
            plan_label(&json!({ "currentTier": { "id": "standard-tier" } })),
            Some("Paid".to_owned())
        );
        assert_eq!(
            plan_label(&json!({ "currentTier": { "id": "free-tier" } })),
            Some("Free".to_owned())
        );
        assert_eq!(plan_label(&json!({})), None);
    }

    #[test]
    fn no_login_hides_and_unreadable_store_keeps_a_live_meter() {
        assert!(matches!(
            login_action(CredentialRead::Absent, true, true),
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
            login_action(CredentialRead::Unavailable("busy".into()), false, false),
            LoginAction::Hide
        ));
    }
}
