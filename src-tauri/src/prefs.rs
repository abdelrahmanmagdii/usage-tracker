use std::{collections::BTreeMap, fs, path::PathBuf, sync::Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Which window a provider's menu-bar meter follows. `auto` tracks whichever
/// window is most used; otherwise this is a window id from `tray::TrayWindow`
/// (e.g. `session:primary`, `weekly-scoped-fable:secondary`).
pub const TRAY_WINDOW_AUTO: &str = "auto";

pub const PROVIDER_CODEX: &str = "codex";
pub const PROVIDER_CLAUDE: &str = "claude";
pub const PROVIDER_CURSOR: &str = "cursor";
pub const PROVIDER_OPENCODE: &str = "opencode";

fn bool_true() -> bool {
    true
}

fn auto_window() -> String {
    TRAY_WINDOW_AUTO.to_owned()
}

fn is_true(value: &bool) -> bool {
    *value
}

fn is_auto(value: &str) -> bool {
    value == TRAY_WINDOW_AUTO
}

/// Per-tool preference: whether the meter is shown, and which quota window
/// its menu-bar title follows.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct ProviderPref {
    /// When false the tool is omitted from the popover and menu bar, and its
    /// backend is not polled.
    #[serde(default = "bool_true", skip_serializing_if = "is_true")]
    pub visible: bool,
    #[serde(default = "auto_window", skip_serializing_if = "is_auto")]
    pub tray_window: String,
}

impl Default for ProviderPref {
    fn default() -> Self {
        Self {
            visible: true,
            tray_window: auto_window(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct AppPrefs {
    /// Local notifications at 80%/95% used and when a window resets.
    pub usage_alerts: bool,
    /// Compact layout: every visible tool under one menu-bar icon (the default,
    /// which resists macOS hiding it on crowded bars). Extended (false) gives
    /// each visible tool its own icon.
    pub combined_tray: bool,
    /// First-run walkthrough has been completed or skipped.
    pub onboarding_complete: bool,
    /// Sparse map of per-tool overrides. Missing keys mean visible + auto.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub providers: BTreeMap<String, ProviderPref>,
}

impl Default for AppPrefs {
    fn default() -> Self {
        Self {
            usage_alerts: true,
            combined_tray: true,
            onboarding_complete: false,
            providers: BTreeMap::new(),
        }
    }
}

impl AppPrefs {
    pub fn provider(&self, id: &str) -> ProviderPref {
        self.providers.get(id).cloned().unwrap_or_default()
    }

    pub fn is_visible(&self, id: &str) -> bool {
        self.provider(id).visible
    }

    pub fn tray_window(&self, id: &str) -> String {
        self.provider(id).tray_window
    }

    pub fn set_visible(&mut self, id: &str, visible: bool) {
        self.providers.entry(id.to_owned()).or_default().visible = visible;
        self.prune_default(id);
    }

    pub fn set_tray_window(&mut self, id: &str, window: String) {
        self.providers.entry(id.to_owned()).or_default().tray_window = window;
        self.prune_default(id);
    }

    fn prune_default(&mut self, id: &str) {
        if self.providers.get(id).is_some_and(|pref| *pref == ProviderPref::default()) {
            self.providers.remove(id);
        }
    }
}

pub struct PrefsStore {
    path: PathBuf,
    state: Mutex<AppPrefs>,
}

impl PrefsStore {
    pub fn load(path: PathBuf) -> Self {
        let state = fs::read_to_string(&path)
            .ok()
            .and_then(|raw| parse_prefs(&raw))
            .unwrap_or_default();
        Self {
            path,
            state: Mutex::new(state),
        }
    }

    pub fn get(&self) -> AppPrefs {
        self.state.lock().expect("preferences poisoned").clone()
    }

    pub fn update(&self, mutate: impl FnOnce(&mut AppPrefs)) -> AppPrefs {
        let mut state = self.state.lock().expect("preferences poisoned");
        mutate(&mut state);
        let snapshot = state.clone();
        drop(state);
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(&snapshot) {
            let _ = fs::write(&self.path, bytes);
        }
        snapshot
    }
}

fn parse_prefs(raw: &str) -> Option<AppPrefs> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let mut prefs: AppPrefs = serde_json::from_value(value.clone()).ok()?;
    migrate_legacy_tray_windows(&mut prefs, &value);
    Some(prefs)
}

/// Builds written before the per-provider map stored `codexTrayWindow` /
/// `claudeTrayWindow` as top-level keys. Copy them in only when the map has
/// not already customized that tool.
fn migrate_legacy_tray_windows(prefs: &mut AppPrefs, raw: &Value) {
    for (id, key) in [
        (PROVIDER_CODEX, "codexTrayWindow"),
        (PROVIDER_CLAUDE, "claudeTrayWindow"),
    ] {
        if prefs.providers.contains_key(id) {
            continue;
        }
        let Some(window) = raw.get(key).and_then(Value::as_str) else {
            continue;
        };
        if window.is_empty() || window == TRAY_WINDOW_AUTO {
            continue;
        }
        prefs.set_tray_window(id, window.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_compact_layout_with_alerts_on() {
        let prefs = AppPrefs::default();
        assert!(prefs.combined_tray);
        assert!(prefs.usage_alerts);
        assert!(prefs.is_visible(PROVIDER_CODEX));
        assert!(prefs.is_visible(PROVIDER_CLAUDE));
        assert!(prefs.is_visible(PROVIDER_CURSOR));
        assert_eq!(prefs.tray_window(PROVIDER_CODEX), TRAY_WINDOW_AUTO);
        assert!(!prefs.onboarding_complete);
    }

    #[test]
    fn unknown_and_missing_keys_fall_back_to_defaults() {
        // Preference files written by older builds (including the retired
        // compactTray key) must still load.
        let prefs: AppPrefs =
            serde_json::from_str(r#"{"compactTray":true,"claudeIncludeScoped":false}"#).unwrap();
        assert!(prefs.combined_tray);
        assert!(prefs.usage_alerts);
        assert_eq!(prefs.tray_window(PROVIDER_CLAUDE), TRAY_WINDOW_AUTO);
    }

    #[test]
    fn migrates_legacy_tray_window_fields() {
        let prefs = parse_prefs(r#"{"codexTrayWindow":"codex:primary","claudeTrayWindow":"session:primary"}"#)
            .expect("prefs");
        assert_eq!(prefs.tray_window(PROVIDER_CODEX), "codex:primary");
        assert_eq!(prefs.tray_window(PROVIDER_CLAUDE), "session:primary");
        assert!(prefs.is_visible(PROVIDER_CODEX));
    }

    #[test]
    fn hiding_a_provider_round_trips() {
        let mut prefs = AppPrefs::default();
        prefs.set_visible(PROVIDER_CURSOR, false);
        assert!(!prefs.is_visible(PROVIDER_CURSOR));
        assert!(prefs.is_visible(PROVIDER_CODEX));
        let encoded = serde_json::to_string(&prefs).unwrap();
        assert!(encoded.contains("cursor"));
        assert!(!encoded.contains("codex"));
        let reloaded: AppPrefs = serde_json::from_str(&encoded).unwrap();
        assert!(!reloaded.is_visible(PROVIDER_CURSOR));
        assert!(reloaded.is_visible(PROVIDER_OPENCODE));
    }

    #[test]
    fn update_persists_and_reloads() {
        let path = std::env::temp_dir().join(format!("usagebar-prefs-{}.json", std::process::id()));
        let store = PrefsStore::load(path.clone());
        store.update(|prefs| prefs.combined_tray = false);
        let reloaded = PrefsStore::load(path.clone());
        assert!(!reloaded.get().combined_tray);
        assert!(reloaded.get().usage_alerts);
        let _ = fs::remove_file(path);
    }
}
