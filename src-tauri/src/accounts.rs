//! Account sources: which independently signed-in login each provider reads.
//!
//! The choice is saved in `accounts.json` beside the preferences and frozen at
//! process start (`AccountSources`). Changing it saves the new choice and
//! restarts UsageBar, so no in-flight reading, cached token or managed Codex
//! child can outlive a switch. UsageBar only stores folder paths, labels and
//! ids here. It never copies, writes or changes a login.
//!
//! Every selection fails closed: a saved profile whose folder is gone stays
//! selected and reads as unavailable, and an unreadable catalog never lets a
//! provider fall back to another identity.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager};

use crate::registry::Provider;

pub const DEFAULT_ID: &str = "default";
const CUSTOM_PREFIX: &str = "custom-";
const MAX_LABEL_CHARS: usize = 40;
const MAX_PROFILES: usize = 12;
const MAX_PATH_CHARS: usize = 1024;
const MAX_ID_CHARS: usize = 64;
const MAX_CATALOG_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The provider's long-standing automatic lookup.
    Default,
    /// Devin Desktop user-data root (holds `User/globalStorage/state.vscdb`).
    DevinDesktop(PathBuf),
    ClaudeKeychain,
    /// The exact Claude credentials file.
    ClaudeFile(PathBuf),
    AntigravityApp,
    AntigravityKeychain,
    /// A folder holding the provider's own credential file.
    Directory(PathBuf),
    /// The saved choice cannot be used. Providers must not read anything.
    Unavailable,
}

#[derive(Debug, Clone)]
pub struct ActiveSource {
    pub id: String,
    pub label: String,
    pub source: Source,
    pub problem: Option<String>,
}

impl ActiveSource {
    fn default_for(provider: &str) -> Self {
        let (label, source) =
            builtin(provider, DEFAULT_ID).unwrap_or(("Default".into(), Source::Default));
        Self {
            id: DEFAULT_ID.into(),
            label,
            source,
            problem: None,
        }
    }

    pub fn is_custom(&self) -> bool {
        self.id.starts_with(CUSTOM_PREFIX)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Profile {
    id: String,
    label: String,
    path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
struct ProviderEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    selected: Option<String>,
    #[serde(default)]
    profiles: Vec<Profile>,
}

#[derive(Debug, Clone, Default)]
struct Catalog {
    providers: BTreeMap<String, ProviderEntry>,
    /// Entries that could not be read. Written back untouched.
    preserved: BTreeMap<String, Value>,
    broken: BTreeMap<String, String>,
    /// The whole file is unreadable. Nothing is trusted or rewritten.
    whole_file_problem: Option<String>,
}

impl Catalog {
    fn problem_for(&self, provider: &str) -> Option<String> {
        self.whole_file_problem
            .clone()
            .or_else(|| self.broken.get(provider).cloned())
    }

    fn to_json(&self) -> Value {
        let mut providers = serde_json::Map::new();
        for (key, raw) in &self.preserved {
            providers.insert(key.clone(), raw.clone());
        }
        for (key, entry) in &self.providers {
            if entry.selected.is_none() && entry.profiles.is_empty() {
                continue;
            }
            providers.insert(
                key.clone(),
                serde_json::to_value(entry).unwrap_or(Value::Null),
            );
        }
        json!({ "version": 1, "providers": Value::Object(providers) })
    }
}

fn parse_catalog(raw: &[u8]) -> Catalog {
    let mut catalog = Catalog::default();
    let Ok(value) = serde_json::from_slice::<Value>(raw) else {
        catalog.whole_file_problem = Some("accounts.json could not be parsed".into());
        return catalog;
    };
    if value.get("version").and_then(Value::as_u64) != Some(1) {
        catalog.whole_file_problem = Some("accounts.json has an unsupported version".into());
        return catalog;
    }
    let Some(providers) = value.get("providers").and_then(Value::as_object) else {
        catalog.whole_file_problem = Some("accounts.json has no provider list".into());
        return catalog;
    };
    for (key, raw_entry) in providers {
        let Some(provider) = Provider::from_key(key) else {
            catalog.preserved.insert(key.clone(), raw_entry.clone());
            continue;
        };
        match serde_json::from_value::<ProviderEntry>(raw_entry.clone())
            .map_err(|_| "its saved profiles could not be read".to_owned())
            .and_then(|entry| validate_entry(provider.key(), &entry).map(|()| entry))
        {
            Ok(entry) => {
                catalog.providers.insert(key.clone(), entry);
            }
            Err(reason) => {
                catalog.preserved.insert(key.clone(), raw_entry.clone());
                catalog.broken.insert(
                    key.clone(),
                    format!("{}: {reason}", provider.display_name()),
                );
            }
        }
    }
    catalog
}

fn validate_entry(provider: &str, entry: &ProviderEntry) -> Result<(), String> {
    if entry.profiles.len() > MAX_PROFILES {
        return Err("too many saved profiles".into());
    }
    let mut seen = std::collections::BTreeSet::new();
    for profile in &entry.profiles {
        if !valid_custom_id(&profile.id)
            || profile.label.chars().count() > MAX_LABEL_CHARS
            || profile.path.is_empty()
            || profile.path.chars().count() > MAX_PATH_CHARS
            || profile.path.contains('\0')
            || !Path::new(&profile.path).is_absolute()
            || !seen.insert(profile.id.clone())
            || !matches!(
                (provider, profile.kind.as_deref()),
                (_, None) | ("devin", Some("cli" | "desktop"))
            )
        {
            return Err("a saved profile is malformed".into());
        }
    }
    if let Some(selected) = &entry.selected {
        let known = (selected != DEFAULT_ID && builtin(provider, selected).is_some())
            || entry.profiles.iter().any(|profile| &profile.id == selected);
        if !known {
            return Err("the selected profile is not in the saved list".into());
        }
    }
    Ok(())
}

fn valid_custom_id(id: &str) -> bool {
    id.len() <= MAX_ID_CHARS
        && id
            .strip_prefix(CUSTOM_PREFIX)
            .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn load_catalog(path: &Path) -> Catalog {
    match std::fs::metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Catalog::default(),
        Err(_) => {
            return Catalog {
                whole_file_problem: Some("accounts.json could not be read".into()),
                ..Catalog::default()
            }
        }
        Ok(metadata) if !metadata.is_file() || metadata.len() > MAX_CATALOG_BYTES => {
            return Catalog {
                whole_file_problem: Some("accounts.json is not a usable file".into()),
                ..Catalog::default()
            }
        }
        Ok(_) => {}
    }
    match std::fs::read(path) {
        Ok(bytes) => parse_catalog(&bytes),
        Err(_) => Catalog {
            whole_file_problem: Some("accounts.json could not be read".into()),
            ..Catalog::default()
        },
    }
}

fn home() -> Option<PathBuf> {
    crate::platform::home_dir()
}

/// Built-in choices for a provider, default first.
fn builtin_ids(provider: &str) -> Vec<&'static str> {
    let mut ids = vec![DEFAULT_ID];
    match provider {
        "claude" => {
            if cfg!(target_os = "macos") {
                ids.push("claude-keychain");
            }
            ids.push("claude-file");
        }
        "antigravity" => {
            ids.push("antigravity-app");
            if cfg!(target_os = "macos") {
                ids.push("antigravity-keychain");
            }
            ids.push("antigravity-file");
        }
        "devin" => {
            if cfg!(any(target_os = "macos", target_os = "windows")) {
                ids.push("devin-desktop");
            }
        }
        _ => {}
    }
    ids
}

fn builtin(provider: &str, id: &str) -> Option<(String, Source)> {
    if !builtin_ids(provider).contains(&id) {
        return None;
    }
    let (label, source) = match (provider, id) {
        ("claude", DEFAULT_ID) => ("Automatic (Keychain or CLI file)", Source::Default),
        ("claude", "claude-keychain") => ("macOS Keychain only", Source::ClaudeKeychain),
        ("claude", "claude-file") => (
            "Claude CLI file only",
            crate::claude::default_credentials_file()
                .map(Source::ClaudeFile)
                .unwrap_or(Source::Unavailable),
        ),
        ("antigravity", DEFAULT_ID) => ("Automatic (running app or stored login)", Source::Default),
        ("antigravity", "antigravity-app") => {
            ("Running Antigravity app only", Source::AntigravityApp)
        }
        ("antigravity", "antigravity-keychain") => {
            ("macOS Keychain only", Source::AntigravityKeychain)
        }
        ("antigravity", "antigravity-file") => (
            "Antigravity CLI file only",
            home()
                .map(|home| Source::Directory(home.join(".gemini/antigravity-cli")))
                .unwrap_or(Source::Unavailable),
        ),
        ("devin", DEFAULT_ID) => ("Devin CLI (default)", Source::Default),
        ("devin", "devin-desktop") => (
            "Devin Desktop",
            dirs::data_dir()
                .map(|dir| Source::DevinDesktop(dir.join("Devin")))
                .unwrap_or(Source::Unavailable),
        ),
        ("codex", DEFAULT_ID) => ("Codex CLI (default)", Source::Default),
        (_, DEFAULT_ID) => ("Default login", Source::Default),
        _ => return None,
    };
    Some((label.to_owned(), source))
}

/// The file inside a profile folder that proves it holds this provider's login.
fn marker(provider: &str, kind: Option<&str>) -> Option<&'static str> {
    Some(match (provider, kind) {
        ("codex", _) => "auth.json",
        ("claude", _) => ".credentials.json",
        ("cursor", _) => "User/globalStorage/state.vscdb",
        ("opencode", _) => "auth.json",
        ("devin", Some("desktop")) => "User/globalStorage/state.vscdb",
        ("devin", _) => "credentials.toml",
        ("gemini", _) => "oauth_creds.json",
        ("antigravity", _) => "antigravity-oauth-token",
        _ => return None,
    })
}

pub fn marker_path(root: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(root.to_path_buf(), |path, part| path.join(part))
}

pub fn folder_hint(provider: &str) -> &'static str {
    match provider {
        "codex" => "An existing Codex home folder that holds auth.json for a separately signed-in account. Nothing is copied.",
        "claude" => "A Claude config folder that holds .credentials.json and projects/. Keychain logins are not read from a folder.",
        "cursor" => "A Cursor user-data folder that holds User/globalStorage/state.vscdb.",
        "opencode" => "An OpenCode data folder that holds auth.json with an OpenCode Go key.",
        "devin" => "A Devin CLI folder that holds credentials.toml, or a Devin Desktop user-data folder that holds User/globalStorage/state.vscdb.",
        "gemini" => "A Gemini CLI folder that holds oauth_creds.json (and optionally settings.json).",
        "antigravity" => "An Antigravity CLI folder that holds antigravity-oauth-token.",
        _ => "",
    }
}

fn custom_source(provider: &str, profile: &Profile) -> Source {
    let root = PathBuf::from(&profile.path);
    match (provider, profile.kind.as_deref()) {
        ("claude", _) => Source::ClaudeFile(root.join(".credentials.json")),
        ("devin", Some("desktop")) => Source::DevinDesktop(root),
        _ => Source::Directory(root),
    }
}

fn custom_marker_present(provider: &str, profile: &Profile) -> bool {
    marker(provider, profile.kind.as_deref())
        .map(|relative| marker_path(Path::new(&profile.path), relative))
        .and_then(|path| std::fs::metadata(path).ok())
        .is_some_and(|metadata| metadata.is_file())
}

fn resolve_provider(provider: Provider, catalog: &Catalog) -> ActiveSource {
    let key = provider.key();
    if let Some(problem) = catalog.problem_for(key) {
        return ActiveSource {
            id: DEFAULT_ID.into(),
            label: "Unavailable".into(),
            source: Source::Unavailable,
            problem: Some(format!(
                "{problem}. Reset the account list in Settings to choose again."
            )),
        };
    }
    let Some(entry) = catalog.providers.get(key) else {
        return ActiveSource::default_for(key);
    };
    let Some(selected) = entry.selected.as_deref() else {
        return ActiveSource::default_for(key);
    };
    if let Some((label, source)) = builtin(key, selected) {
        let problem = matches!(source, Source::Unavailable)
            .then(|| "That login location is not available on this computer.".to_owned());
        return ActiveSource {
            id: selected.to_owned(),
            label,
            source,
            problem,
        };
    }
    match entry.profiles.iter().find(|profile| profile.id == selected) {
        Some(profile) if custom_marker_present(key, profile) => ActiveSource {
            id: profile.id.clone(),
            label: profile.label.clone(),
            source: custom_source(key, profile),
            problem: None,
        },
        Some(profile) => ActiveSource {
            id: profile.id.clone(),
            label: profile.label.clone(),
            source: custom_source(key, profile),
            problem: Some(format!(
                "The folder for “{}” is missing or no longer holds a login.",
                profile.label
            )),
        },
        None => ActiveSource {
            id: DEFAULT_ID.into(),
            label: "Unavailable".into(),
            source: Source::Unavailable,
            problem: Some("The selected profile is not in the saved list.".into()),
        },
    }
}

/// Resolved once at startup and never changed afterwards.
pub struct AccountSources {
    active: BTreeMap<&'static str, ActiveSource>,
}

impl AccountSources {
    fn from_catalog(catalog: &Catalog) -> Self {
        Self {
            active: Provider::ALL
                .into_iter()
                .map(|provider| (provider.key(), resolve_provider(provider, catalog)))
                .collect(),
        }
    }

    pub fn get(&self, provider: &str) -> ActiveSource {
        self.active
            .get(provider)
            .cloned()
            .unwrap_or_else(|| ActiveSource::default_for(provider))
    }
}

/// The frozen selection for a provider. Falls back to the default only when the
/// state was never installed (unit tests).
pub fn active(app: &AppHandle, provider: &str) -> ActiveSource {
    app.try_state::<AccountSources>()
        .map(|sources| sources.get(provider))
        .unwrap_or_else(|| ActiveSource::default_for(provider))
}

pub fn install(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let path = app.path().app_config_dir()?.join("accounts.json");
    // Demo mode never reads the saved list or stats a profile's login files.
    let catalog = if crate::demo::enabled() {
        Catalog::default()
    } else {
        load_catalog(&path)
    };
    app.manage(AccountSources::from_catalog(&catalog));
    app.manage(AccountStore::new(path, catalog));
    Ok(())
}

// ---- identity ------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountContext {
    pub scope_id: Option<String>,
    pub source_label: String,
    pub masked_identity: Option<String>,
}

impl AccountContext {
    /// Adds the context fields to a sanitized account object.
    pub fn apply(&self, account: Value) -> Value {
        let mut account = match account {
            Value::Object(map) => map,
            _ => serde_json::Map::new(),
        };
        if let Some(scope) = &self.scope_id {
            account.insert("scopeId".into(), Value::from(scope.clone()));
        }
        account.insert("sourceLabel".into(), Value::from(self.source_label.clone()));
        if let Some(masked) = &self.masked_identity {
            account.insert("maskedIdentity".into(), Value::from(masked.clone()));
        }
        Value::Object(account)
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// `hex(sha256(provider \0 source_id \0 stable))`, 256 bits.
pub fn scope_id(provider: &str, source_id: &str, stable: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(provider.as_bytes());
    hasher.update([0]);
    hasher.update(source_id.as_bytes());
    hasher.update([0]);
    hasher.update(stable.as_bytes());
    hex(&hasher.finalize())
}

/// A stable stand-in for an opaque credential. Never shown or stored.
pub fn credential_stable(secret: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"credential\0");
    hasher.update(secret.as_bytes());
    format!("cred:{}", hex(&hasher.finalize()))
}

/// `a***@example.com`: first character of the local part and the domain.
pub fn mask_email(email: &str) -> Option<String> {
    let email = email.trim();
    if email.len() > 254 || email.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return None;
    }
    let (local, domain) = email.split_once('@')?;
    let first = local.chars().next()?;
    let domain_ok = !domain.is_empty()
        && domain.contains('.')
        && !domain.contains('@')
        && domain
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    domain_ok.then(|| format!("{first}***@{domain}"))
}

/// A short, non-reversible tag for a stable identifier with no human name.
pub fn short_tag(stable: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"tag\0");
    hasher.update(stable.as_bytes());
    format!("id {}", &hex(&hasher.finalize())[..6])
}

/// `sub` and `iss` from a JWT payload. The signature is not checked: this is
/// identity metadata only and never authorizes anything.
pub fn jwt_subject(token: &str) -> (Option<String>, Option<String>) {
    let Some(payload) = token.split('.').nth(1).filter(|part| part.len() <= 8192) else {
        return (None, None);
    };
    let Ok(bytes) =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('='))
    else {
        return (None, None);
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return (None, None);
    };
    let claim = |name: &str| {
        value
            .get(name)
            .and_then(Value::as_str)
            .filter(|text| {
                !text.is_empty() && text.len() <= 256 && !text.chars().any(char::is_control)
            })
            .map(str::to_owned)
    };
    (claim("sub"), claim("iss"))
}

/// The account context for a reading made with the frozen selection.
/// `stable` is a stable user id, or an opaque credential digest.
pub fn context_from(
    provider: &str,
    source: &ActiveSource,
    stable: Option<&str>,
    masked: Option<String>,
) -> AccountContext {
    AccountContext {
        scope_id: stable.map(|stable| scope_id(provider, &source.id, stable)),
        source_label: source.label.clone(),
        masked_identity: masked.or_else(|| stable.map(short_tag)),
    }
}

// ---- store and commands --------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChoiceDto {
    pub id: String,
    pub label: String,
    pub custom: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSourcesDto {
    pub active_id: String,
    pub choices: Vec<ChoiceDto>,
    pub folder_hint: &'static str,
    /// Set when the saved list cannot be read; the user may reset it.
    pub diagnostic: Option<String>,
    /// Set when the active choice cannot currently be used.
    pub unavailable: Option<String>,
}

pub struct AccountStore {
    path: PathBuf,
    catalog: Mutex<Catalog>,
}

impl AccountStore {
    fn new(path: PathBuf, catalog: Catalog) -> Self {
        Self {
            path,
            catalog: Mutex::new(catalog),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Catalog> {
        self.catalog
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn save(&self, catalog: &Catalog) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(&catalog.to_json())
            .map_err(|error| format!("Could not save accounts: {error}"))?;
        if bytes.len() as u64 > MAX_CATALOG_BYTES {
            return Err("The saved account list would be too large.".into());
        }
        crate::prefs::write_atomically(&self.path, &bytes)
            .map_err(|error| format!("Could not save accounts: {error}"))
    }
}

fn choices(provider: &str, entry: Option<&ProviderEntry>) -> Vec<ChoiceDto> {
    let mut list: Vec<ChoiceDto> = builtin_ids(provider)
        .into_iter()
        .filter_map(|id| {
            builtin(provider, id).map(|(label, _)| ChoiceDto {
                id: id.to_owned(),
                label,
                custom: false,
            })
        })
        .collect();
    if let Some(entry) = entry {
        list.extend(entry.profiles.iter().map(|profile| ChoiceDto {
            id: profile.id.clone(),
            label: profile.label.clone(),
            custom: true,
        }));
    }
    list
}

fn dto(provider: Provider, catalog: &Catalog, active: &ActiveSource) -> AccountSourcesDto {
    AccountSourcesDto {
        active_id: active.id.clone(),
        choices: choices(provider.key(), catalog.providers.get(provider.key())),
        folder_hint: folder_hint(provider.key()),
        diagnostic: catalog.problem_for(provider.key()),
        unavailable: active.problem.clone(),
    }
}

fn clean_label(label: Option<&str>, fallback: &Path) -> Result<String, String> {
    let given = label.map(str::trim).filter(|text| !text.is_empty());
    let text = match given {
        Some(text) => text.to_owned(),
        None => fallback
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Profile")
            .to_owned(),
    };
    if text.chars().any(char::is_control) {
        return Err("The profile name cannot contain control characters.".into());
    }
    Ok(text.chars().take(MAX_LABEL_CHARS).collect())
}

fn custom_id(provider: &str, canonical: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(provider.as_bytes());
    hasher.update([0]);
    hasher.update(canonical.to_string_lossy().as_bytes());
    format!("{CUSTOM_PREFIX}{}", &hex(&hasher.finalize())[..16])
}

/// Which marker a chosen folder holds. Devin has two shapes of login.
fn detect_kind(provider: &str, root: &Path) -> Result<Option<String>, String> {
    let present = |kind: Option<&str>| {
        marker(provider, kind)
            .map(|relative| marker_path(root, relative))
            .and_then(|path| std::fs::metadata(path).ok())
            .is_some_and(|metadata| metadata.is_file())
    };
    if provider == "devin" {
        if present(Some("cli")) {
            return Ok(Some("cli".into()));
        }
        if present(Some("desktop")) {
            return Ok(Some("desktop".into()));
        }
    } else if present(None) {
        return Ok(None);
    }
    Err(format!(
        "That folder does not hold a {} login. {}",
        Provider::from_key(provider).map_or("provider", Provider::display_name),
        folder_hint(provider)
    ))
}

impl AccountStore {
    fn describe(&self, provider: Provider, active: &ActiveSource) -> AccountSourcesDto {
        let catalog = self.lock();
        let mut info = dto(provider, &catalog, active);
        // Marker files come and go while the app runs: the notice follows the
        // folder as it is now. It never changes which source is in use.
        if active.is_custom() {
            if let Some(profile) = catalog
                .providers
                .get(provider.key())
                .and_then(|entry| entry.profiles.iter().find(|p| p.id == active.id))
            {
                info.unavailable = (!custom_marker_present(provider.key(), profile)).then(|| {
                    format!(
                        "The folder for “{}” is missing or no longer holds a login.",
                        profile.label
                    )
                });
            }
        }
        info
    }

    fn add(
        &self,
        provider: Provider,
        folder: &Path,
        label: Option<&str>,
        active: &ActiveSource,
    ) -> Result<AccountSourcesDto, String> {
        let key = provider.key();
        let mut current = self.lock();
        let mut next = current.clone();
        if let Some(problem) = next.problem_for(key) {
            return Err(format!("{problem}. Reset the account list first."));
        }
        let canonical = std::fs::canonicalize(folder)
            .map_err(|_| "That folder could not be opened.".to_owned())?;
        if !canonical.is_dir() {
            return Err("Choose a folder, not a file.".into());
        }
        let path = canonical
            .to_str()
            .filter(|text| text.chars().count() <= MAX_PATH_CHARS && !text.contains('\0'))
            .ok_or_else(|| "That folder path cannot be saved.".to_owned())?
            .to_owned();
        let kind = detect_kind(key, &canonical)?;
        let id = custom_id(key, &canonical);
        let label = clean_label(label, &canonical)?;
        let entry = next.providers.entry(key.to_owned()).or_default();
        if !entry.profiles.iter().any(|profile| profile.id == id) {
            if entry.profiles.len() >= MAX_PROFILES {
                return Err(format!("At most {MAX_PROFILES} profiles can be saved."));
            }
            entry.profiles.push(Profile {
                id,
                label,
                path,
                kind,
            });
        }
        self.save(&next)?;
        *current = next;
        Ok(dto(provider, &current, active))
    }

    fn forget(
        &self,
        provider: Provider,
        id: &str,
        active: &ActiveSource,
    ) -> Result<AccountSourcesDto, String> {
        let key = provider.key();
        let mut current = self.lock();
        let mut next = current.clone();
        if let Some(problem) = next.problem_for(key) {
            return Err(format!("{problem}. Reset the account list first."));
        }
        let entry = next.providers.entry(key.to_owned()).or_default();
        if !entry.profiles.iter().any(|profile| profile.id == id) {
            return Err("That profile is not saved.".into());
        }
        if entry.selected.as_deref() == Some(id) || active.id == id {
            return Err("Switch to another profile before forgetting the active one.".into());
        }
        entry.profiles.retain(|profile| profile.id != id);
        self.save(&next)?;
        *current = next;
        Ok(dto(provider, &current, active))
    }

    /// Saves the new choice. Returns false when it is already the active one.
    fn select(&self, provider: Provider, id: &str, active: &ActiveSource) -> Result<bool, String> {
        let key = provider.key();
        let mut current = self.lock();
        let mut next = current.clone();
        if let Some(problem) = next.problem_for(key) {
            return Err(format!("{problem}. Reset the account list first."));
        }
        let entry = next.providers.entry(key.to_owned()).or_default();
        let known = builtin(key, id).is_some() || entry.profiles.iter().any(|p| p.id == id);
        if !known {
            return Err("That profile is not available for this provider.".into());
        }
        let selected = entry.selected.as_deref().unwrap_or(DEFAULT_ID);
        if selected == id && active.id == id {
            return Ok(false);
        }
        entry.selected = (id != DEFAULT_ID).then(|| id.to_owned());
        self.save(&next)?;
        *current = next;
        Ok(true)
    }

    /// Clears an unreadable mapping. A damaged file is first kept, never
    /// overwritten, as `accounts.json.invalid` (or `.invalid.N`).
    fn reset(&self, provider: Provider) -> Result<(), String> {
        let key = provider.key();
        let mut current = self.lock();
        if current.problem_for(key).is_none() {
            return Err("The account list for this provider is readable.".into());
        }
        if current.whole_file_problem.is_some() {
            let is_file = std::fs::symlink_metadata(&self.path)
                .map(|metadata| metadata.file_type().is_file())
                .unwrap_or(false);
            if !is_file {
                return Err(
                    "accounts.json is not a regular file. Move it aside yourself, then restart UsageBar."
                        .into(),
                );
            }
            let backup = copy_to_unused_backup(&self.path)?;
            let empty = Catalog::default();
            if let Err(error) = self.save(&empty) {
                let _ = std::fs::remove_file(&backup);
                return Err(error);
            }
            *current = empty;
            return Ok(());
        }
        let mut next = current.clone();
        next.broken.remove(key);
        next.preserved.remove(key);
        next.providers.remove(key);
        self.save(&next)?;
        *current = next;
        Ok(())
    }
}

/// Copies the damaged file to the first unused `<file>.invalid[.N]` name.
fn copy_to_unused_backup(path: &Path) -> Result<PathBuf, String> {
    for attempt in 0..100 {
        let mut name = path.as_os_str().to_owned();
        name.push(".invalid");
        if attempt > 0 {
            name.push(format!(".{attempt}"));
        }
        let backup = PathBuf::from(name);
        let created = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&backup);
        match created {
            Ok(mut file) => {
                let copied = std::fs::File::open(path)
                    .and_then(|mut original| std::io::copy(&mut original, &mut file))
                    .and_then(|_| file.sync_all());
                if let Err(error) = copied {
                    let _ = std::fs::remove_file(&backup);
                    return Err(format!(
                        "Could not keep a copy of the damaged file: {error}"
                    ));
                }
                return Ok(backup);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "Could not keep a copy of the damaged file: {error}"
                ))
            }
        }
    }
    Err("Too many earlier backups of accounts.json exist.".into())
}

fn demo_dto(provider: Provider) -> AccountSourcesDto {
    AccountSourcesDto {
        active_id: DEFAULT_ID.into(),
        choices: choices(provider.key(), None),
        folder_hint: folder_hint(provider.key()),
        diagnostic: None,
        unavailable: None,
    }
}

fn refuse_in_demo() -> Result<(), String> {
    if crate::demo::enabled() {
        return Err("Accounts cannot be changed in demo mode.".into());
    }
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
async fn pick_folder(provider: Provider) -> Result<Option<PathBuf>, String> {
    let title = format!("Choose a {} profile folder", provider.display_name());
    Ok(rfd::AsyncFileDialog::new()
        .set_title(title)
        .pick_folder()
        .await
        .map(|handle| handle.path().to_path_buf()))
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
async fn pick_folder(_provider: Provider) -> Result<Option<PathBuf>, String> {
    Err("Choosing a folder is not supported on this platform.".into())
}

#[tauri::command]
pub fn get_account_sources(app: AppHandle, provider: String) -> Result<AccountSourcesDto, String> {
    let provider = Provider::parse(&provider)?;
    if crate::demo::enabled() {
        return Ok(demo_dto(provider));
    }
    let active = active(&app, provider.key());
    Ok(app.state::<AccountStore>().describe(provider, &active))
}

#[tauri::command]
pub async fn add_account_profile(
    app: AppHandle,
    provider: String,
    label: Option<String>,
) -> Result<Option<AccountSourcesDto>, String> {
    let provider = Provider::parse(&provider)?;
    refuse_in_demo()?;
    let Some(folder) = pick_folder(provider).await? else {
        return Ok(None);
    };
    let active = active(&app, provider.key());
    app.state::<AccountStore>()
        .add(provider, &folder, label.as_deref(), &active)
        .map(Some)
}

#[tauri::command]
pub fn forget_account_profile(
    app: AppHandle,
    provider: String,
    id: String,
) -> Result<AccountSourcesDto, String> {
    let provider = Provider::parse(&provider)?;
    refuse_in_demo()?;
    let active = active(&app, provider.key());
    app.state::<AccountStore>().forget(provider, &id, &active)
}

async fn restart_usagebar(app: &AppHandle) {
    if let Some(codex) = app.try_state::<crate::codex::process::CodexManager>() {
        let codex = codex.inner().clone();
        codex.shutdown().await;
    }
    app.request_restart();
}

#[tauri::command]
pub async fn switch_account_profile(
    app: AppHandle,
    provider: String,
    id: String,
) -> Result<(), String> {
    let provider = Provider::parse(&provider)?;
    refuse_in_demo()?;
    let active = active(&app, provider.key());
    let changed = app.state::<AccountStore>().select(provider, &id, &active)?;
    if changed {
        restart_usagebar(&app).await;
    }
    Ok(())
}

#[tauri::command]
pub async fn reset_account_mapping(app: AppHandle, provider: String) -> Result<(), String> {
    let provider = Provider::parse(&provider)?;
    refuse_in_demo()?;
    app.state::<AccountStore>().reset(provider)?;
    restart_usagebar(&app).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "usagebar-accounts-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn profile_dir(root: &Path, name: &str, provider: &str, kind: Option<&str>) -> PathBuf {
        let dir = root.join(name);
        let file = marker_path(&dir, marker(provider, kind).unwrap());
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, b"x").unwrap();
        dir
    }

    fn store(root: &Path) -> AccountStore {
        let path = root.join("accounts.json");
        let catalog = load_catalog(&path);
        AccountStore::new(path, catalog)
    }

    fn active_for(store: &AccountStore, provider: Provider) -> ActiveSource {
        resolve_provider(provider, &store.lock())
    }

    #[test]
    fn every_provider_has_a_marker_and_a_default() {
        for provider in Provider::ALL {
            assert!(marker(provider.key(), None).is_some(), "{}", provider.key());
            assert!(builtin(provider.key(), DEFAULT_ID).is_some());
            assert!(!folder_hint(provider.key()).is_empty());
        }
        assert_eq!(marker("devin", Some("cli")), Some("credentials.toml"));
        assert_eq!(
            marker("devin", Some("desktop")),
            Some("User/globalStorage/state.vscdb")
        );
        assert_eq!(
            marker("cursor", None),
            Some("User/globalStorage/state.vscdb")
        );
    }

    #[test]
    fn registering_needs_the_marker_file_and_saves_only_paths_and_labels() {
        let root = temp_dir("add");
        for provider in Provider::ALL {
            let kind = (provider.key() == "devin").then_some("cli");
            let dir = profile_dir(&root, provider.key(), provider.key(), kind);
            let store = store(&root);
            let active = active_for(&store, provider);
            let result = store.add(provider, &dir, Some("Work"), &active).unwrap();
            assert!(result.choices.iter().any(|c| c.custom && c.label == "Work"));
            // A folder without the marker is refused.
            let empty = root.join(format!("empty-{}", provider.key()));
            std::fs::create_dir_all(&empty).unwrap();
            assert!(store.add(provider, &empty, None, &active).is_err());
            let _ = std::fs::remove_file(root.join("accounts.json"));
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_marker_that_is_a_directory_is_not_a_login() {
        let root = temp_dir("dirmarker");
        let dir = root.join("codex");
        std::fs::create_dir_all(dir.join("auth.json")).unwrap();
        let store = store(&root);
        let active = active_for(&store, Provider::Codex);
        assert!(store.add(Provider::Codex, &dir, None, &active).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn selection_persists_across_a_reload_and_dedupes_one_folder() {
        let root = temp_dir("persist");
        let dir = profile_dir(&root, "work", "gemini", None);
        let store1 = store(&root);
        let active = active_for(&store1, Provider::Gemini);
        store1
            .add(Provider::Gemini, &dir, Some("Work"), &active)
            .unwrap();
        let again = store1
            .add(Provider::Gemini, &dir, Some("Other"), &active)
            .unwrap();
        assert_eq!(again.choices.iter().filter(|c| c.custom).count(), 1);
        let id = again.choices.iter().find(|c| c.custom).unwrap().id.clone();
        assert!(store1.select(Provider::Gemini, &id, &active).unwrap());

        let reloaded = store(&root);
        let resolved = active_for(&reloaded, Provider::Gemini);
        assert_eq!(resolved.id, id);
        assert_eq!(resolved.label, "Work");
        assert!(resolved.problem.is_none());
        assert!(matches!(resolved.source, Source::Directory(_)));
        // Selecting the active one again changes nothing.
        assert!(!reloaded.select(Provider::Gemini, &id, &resolved).unwrap());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn unknown_or_foreign_ids_are_rejected() {
        let root = temp_dir("ids");
        let dir = profile_dir(&root, "w", "gemini", None);
        let store = store(&root);
        let active = active_for(&store, Provider::Gemini);
        let id = store
            .add(Provider::Gemini, &dir, None, &active)
            .unwrap()
            .choices
            .into_iter()
            .find(|c| c.custom)
            .unwrap()
            .id;
        assert!(store
            .select(Provider::Gemini, "custom-ffff", &active)
            .is_err());
        assert!(store
            .select(Provider::Gemini, "claude-file", &active)
            .is_err());
        // An id saved for Gemini cannot be used for Codex.
        assert!(store.select(Provider::Codex, &id, &active).is_err());
        assert!(store.forget(Provider::Gemini, "default", &active).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_missing_folder_stays_selected_and_unavailable() {
        let root = temp_dir("missing");
        let dir = profile_dir(&root, "work", "codex", None);
        let store1 = store(&root);
        let active = active_for(&store1, Provider::Codex);
        let id = store1
            .add(Provider::Codex, &dir, Some("Work"), &active)
            .unwrap()
            .choices
            .into_iter()
            .find(|c| c.custom)
            .unwrap()
            .id;
        store1.select(Provider::Codex, &id, &active).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let resolved = active_for(&store(&root), Provider::Codex);
        assert_eq!(resolved.id, id);
        assert!(resolved.problem.is_some());
        assert!(matches!(resolved.source, Source::Directory(_)));
        // The active profile cannot be forgotten.
        assert!(store1.forget(Provider::Codex, &id, &resolved).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_broken_catalog_fails_closed_and_is_never_overwritten() {
        let root = temp_dir("broken");
        let path = root.join("accounts.json");
        std::fs::write(&path, b"{ not json").unwrap();
        let store = store(&root);
        for provider in Provider::ALL {
            let resolved = active_for(&store, provider);
            assert_eq!(resolved.source, Source::Unavailable);
            assert!(resolved.problem.is_some());
        }
        let dir = profile_dir(&root, "w", "codex", None);
        let active = active_for(&store, Provider::Codex);
        assert!(store.add(Provider::Codex, &dir, None, &active).is_err());
        assert!(store.select(Provider::Codex, "default", &active).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"{ not json");
        // Reset sets the file aside and clears the problem.
        store.reset(Provider::Codex).unwrap();
        assert!(root.join("accounts.json.invalid").exists());
        assert!(active_for(&store, Provider::Codex).problem.is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn one_broken_provider_entry_leaves_the_others_alone_and_is_preserved() {
        let root = temp_dir("partial");
        let dir = profile_dir(&root, "w", "gemini", None);
        std::fs::write(
            root.join("accounts.json"),
            br#"{"version":1,"providers":{"codex":{"selected":"custom-zz","profiles":[]},"mystery":{"x":1}}}"#,
        )
        .unwrap();
        let store = store(&root);
        assert_eq!(
            active_for(&store, Provider::Codex).source,
            Source::Unavailable
        );
        assert!(active_for(&store, Provider::Gemini).problem.is_none());
        let active = active_for(&store, Provider::Gemini);
        store.add(Provider::Gemini, &dir, None, &active).unwrap();
        let saved: Value =
            serde_json::from_slice(&std::fs::read(root.join("accounts.json")).unwrap()).unwrap();
        assert_eq!(saved["providers"]["codex"]["selected"], "custom-zz");
        assert_eq!(saved["providers"]["mystery"]["x"], 1);
        store.reset(Provider::Codex).unwrap();
        assert!(active_for(&store, Provider::Codex).problem.is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_saved_file_holds_no_secrets_and_the_dto_hides_paths() {
        let root = temp_dir("secrets");
        let dir = root.join("claude");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".credentials.json"), b"sk-ant-SECRET-TOKEN").unwrap();
        let store = store(&root);
        let active = active_for(&store, Provider::Claude);
        let dto = store
            .add(Provider::Claude, &dir, Some("Team"), &active)
            .unwrap();
        let saved = std::fs::read_to_string(root.join("accounts.json")).unwrap();
        assert!(!saved.contains("SECRET"));
        let json = serde_json::to_string(&dto).unwrap();
        assert!(!json.contains(dir.to_str().unwrap()));
        assert!(!json.contains("path"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn devin_remembers_the_kind_it_found_and_never_flips_it() {
        let root = temp_dir("devin");
        let desktop = profile_dir(&root, "desk", "devin", Some("desktop"));
        let store = store(&root);
        let active = active_for(&store, Provider::Devin);
        let id = store
            .add(Provider::Devin, &desktop, None, &active)
            .unwrap()
            .choices
            .into_iter()
            .find(|c| c.custom)
            .unwrap()
            .id;
        store.select(Provider::Devin, &id, &active).unwrap();
        let resolved = active_for(&store, Provider::Devin);
        assert!(matches!(resolved.source, Source::DevinDesktop(_)));
        // The database vanishing leaves it a Desktop profile.
        std::fs::remove_dir_all(&desktop).unwrap();
        let resolved = active_for(&self::store(&root), Provider::Devin);
        assert!(matches!(resolved.source, Source::DevinDesktop(_)));
        assert!(resolved.problem.is_some());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn claude_custom_profiles_pin_the_credentials_file() {
        let root = temp_dir("claude");
        let dir = profile_dir(&root, "c", "claude", None);
        let store = store(&root);
        let active = active_for(&store, Provider::Claude);
        let id = store
            .add(Provider::Claude, &dir, None, &active)
            .unwrap()
            .choices
            .into_iter()
            .find(|c| c.custom)
            .unwrap()
            .id;
        store.select(Provider::Claude, &id, &active).unwrap();
        let canonical = std::fs::canonicalize(&dir).unwrap();
        assert_eq!(
            active_for(&store, Provider::Claude).source,
            Source::ClaudeFile(canonical.join(".credentials.json"))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn labels_are_bounded_and_clean() {
        let path = Path::new("/tmp/some-folder");
        assert_eq!(clean_label(None, path).unwrap(), "some-folder");
        assert_eq!(clean_label(Some("  "), path).unwrap(), "some-folder");
        assert_eq!(
            clean_label(Some(&"x".repeat(200)), path)
                .unwrap()
                .chars()
                .count(),
            MAX_LABEL_CHARS
        );
        assert!(clean_label(Some("a\nb"), path).is_err());
    }

    #[test]
    fn scope_ids_are_long_and_depend_on_every_part() {
        let a = scope_id("codex", "default", "user-1");
        assert_eq!(a.len(), 64);
        assert_ne!(a, scope_id("codex", "default", "user-2"));
        assert_ne!(a, scope_id("codex", "custom-1", "user-1"));
        assert_ne!(a, scope_id("claude", "default", "user-1"));
        // The separator stops "a|bc" colliding with "ab|c".
        assert_ne!(scope_id("a", "bc", "x"), scope_id("ab", "c", "x"));
    }

    #[test]
    fn emails_are_masked_and_bad_input_is_refused() {
        assert_eq!(
            mask_email("alice@example.com").as_deref(),
            Some("a***@example.com")
        );
        assert_eq!(mask_email("not an email"), None);
        assert_eq!(mask_email("a@b@c.com"), None);
        assert_eq!(mask_email("a@nodot"), None);
        assert_eq!(mask_email("@example.com"), None);
        assert!(!mask_email("alice@example.com").unwrap().contains("alice"));
    }

    #[test]
    fn jwt_subject_reads_only_sub_and_iss() {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"sub":"u-1","iss":"https://accounts.example","email":"a@b.com"}"#);
        let token = format!("h.{payload}.s");
        assert_eq!(
            jwt_subject(&token),
            (Some("u-1".into()), Some("https://accounts.example".into()))
        );
        assert_eq!(jwt_subject("opaque"), (None, None));
        assert_eq!(jwt_subject("a.!!.c"), (None, None));
    }

    #[test]
    fn context_carries_label_scope_and_a_masked_identity() {
        let source = ActiveSource {
            id: "custom-1".into(),
            label: "Work".into(),
            source: Source::Default,
            problem: None,
        };
        let context = context_from("codex", &source, Some("user-1"), Some("a***@b.com".into()));
        let account = context.apply(json!({ "planType": "pro" }));
        assert_eq!(account["sourceLabel"], "Work");
        assert_eq!(account["maskedIdentity"], "a***@b.com");
        assert_eq!(account["scopeId"].as_str().unwrap().len(), 64);
        let anonymous = context_from("codex", &source, None, None).apply(json!({}));
        assert!(anonymous.get("scopeId").is_none());
    }

    fn custom_ids(info: &AccountSourcesDto) -> Vec<String> {
        info.choices
            .iter()
            .filter(|c| c.custom)
            .map(|c| c.id.clone())
            .collect()
    }

    #[test]
    fn a_failed_save_leaves_no_partial_state_and_cannot_resurface_later() {
        let root = temp_dir("txn");
        let dir_a = profile_dir(&root, "a", "codex", None);
        let dir_b = profile_dir(&root, "b", "codex", None);
        let mut store = store(&root);
        let active = active_for(&store, Provider::Codex);
        let id_a = custom_ids(
            &store
                .add(Provider::Codex, &dir_a, Some("A"), &active)
                .unwrap(),
        )
        .remove(0);

        let blocker = root.join("blocker");
        std::fs::write(&blocker, b"file").unwrap();
        let good_path = std::mem::replace(&mut store.path, blocker.join("accounts.json"));
        assert!(store
            .add(Provider::Codex, &dir_b, Some("B"), &active)
            .is_err());
        assert!(store.select(Provider::Codex, &id_a, &active).is_err());
        assert!(store.forget(Provider::Codex, &id_a, &active).is_err());
        // Validation failures change nothing either.
        assert!(store.select(Provider::Codex, "nope", &active).is_err());
        let info = store.describe(Provider::Codex, &active);
        assert_eq!(custom_ids(&info), vec![id_a.clone()]);
        assert_eq!(active_for(&store, Provider::Codex).id, DEFAULT_ID);
        assert_eq!(store.lock().providers["codex"].selected, None);

        // A later, unrelated successful save cannot carry the failed switch.
        store.path = good_path;
        store
            .add(Provider::Codex, &dir_b, Some("B"), &active)
            .unwrap();
        let saved: Value =
            serde_json::from_slice(&std::fs::read(root.join("accounts.json")).unwrap()).unwrap();
        assert!(saved["providers"]["codex"].get("selected").is_none());
        assert_eq!(
            saved["providers"]["codex"]["profiles"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_catalog_too_large_to_load_is_never_saved() {
        let root = temp_dir("size");
        let store = store(&root);
        let mut huge = Catalog::default();
        huge.preserved.insert(
            "mystery".into(),
            json!("x".repeat(MAX_CATALOG_BYTES as usize)),
        );
        assert!(store.save(&huge).is_err());
        assert!(!root.join("accounts.json").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_unsupported_or_missing_version_is_refused_not_rewritten() {
        for raw in [
            &br#"{"version":2,"providers":{}}"#[..],
            br#"{"providers":{}}"#,
            br#"{"version":"1","providers":{}}"#,
        ] {
            let catalog = parse_catalog(raw);
            assert!(catalog.whole_file_problem.is_some());
            assert_eq!(
                resolve_provider(Provider::Codex, &catalog).source,
                Source::Unavailable
            );
        }
        assert!(parse_catalog(br#"{"version":1,"providers":{}}"#)
            .whole_file_problem
            .is_none());
    }

    #[test]
    fn resetting_keeps_every_earlier_backup() {
        let root = temp_dir("backup");
        let path = root.join("accounts.json");
        std::fs::write(&path, b"{ broken").unwrap();
        std::fs::write(root.join("accounts.json.invalid"), b"older backup").unwrap();
        let store = store(&root);
        store.reset(Provider::Codex).unwrap();
        assert_eq!(
            std::fs::read(root.join("accounts.json.invalid")).unwrap(),
            b"older backup"
        );
        assert_eq!(
            std::fs::read(root.join("accounts.json.invalid.1")).unwrap(),
            b"{ broken"
        );
        assert!(!load_catalog(&path).whole_file_problem.is_some());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_failed_reset_leaves_the_damaged_file_readable() {
        let root = temp_dir("resetfail");
        let path = root.join("sub").join("accounts.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{ broken").unwrap();
        let store = store(&root.join("sub"));
        // Make the atomic write impossible: its temporary name is a directory.
        std::fs::create_dir(root.join("sub").join("accounts.json.tmp")).unwrap();
        assert!(store.reset(Provider::Codex).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"{ broken");
        assert!(store.lock().whole_file_problem.is_some());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_directory_at_accounts_json_is_never_renamed_away() {
        let root = temp_dir("dirreset");
        let path = root.join("accounts.json");
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("keep.txt"), b"mine").unwrap();
        let store = store(&root);
        assert!(store.lock().whole_file_problem.is_some());
        assert!(store.reset(Provider::Codex).is_err());
        assert!(path.join("keep.txt").exists());
        assert!(!root.join("accounts.json.invalid").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_unavailable_notice_follows_the_folder_without_changing_the_source() {
        let root = temp_dir("notice");
        let dir = profile_dir(&root, "w", "gemini", None);
        let store = store(&root);
        let active = active_for(&store, Provider::Gemini);
        let id = custom_ids(
            &store
                .add(Provider::Gemini, &dir, Some("Work"), &active)
                .unwrap(),
        )
        .remove(0);
        store.select(Provider::Gemini, &id, &active).unwrap();
        let frozen = active_for(&store, Provider::Gemini);
        assert!(store
            .describe(Provider::Gemini, &frozen)
            .unavailable
            .is_none());
        std::fs::remove_file(marker_path(&dir, "oauth_creds.json")).unwrap();
        let info = store.describe(Provider::Gemini, &frozen);
        assert!(info.unavailable.is_some());
        assert_eq!(info.active_id, id);
        std::fs::write(marker_path(&dir, "oauth_creds.json"), b"x").unwrap();
        assert!(store
            .describe(Provider::Gemini, &frozen)
            .unavailable
            .is_none());
        let _ = std::fs::remove_dir_all(root);
    }
}
