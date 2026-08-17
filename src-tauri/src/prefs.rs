use std::{fs, path::PathBuf, sync::Mutex};

use serde::{Deserialize, Serialize};

/// Which window a provider's menu-bar meter follows. `auto` tracks whichever
/// window is most used; otherwise this is a window id from `tray::TrayWindow`
/// (e.g. `session:primary`, `weekly-scoped-fable:secondary`).
pub const TRAY_WINDOW_AUTO: &str = "auto";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct AppPrefs {
    /// Tray shows only the percentage, no countdown.
    pub compact_tray: bool,
    /// Local notifications at 80%/95% used and when a window resets.
    pub usage_alerts: bool,
    /// Window the Codex meter follows.
    pub codex_tray_window: String,
    /// Window the Claude meter follows.
    pub claude_tray_window: String,
    /// One combined menu-bar icon for both providers (true, the default, which
    /// resists macOS hiding it on crowded bars) versus one icon per provider.
    pub combined_tray: bool,
    /// First-run walkthrough has been completed or skipped.
    pub onboarding_complete: bool,
}

impl Default for AppPrefs {
    fn default() -> Self {
        Self {
            compact_tray: false,
            usage_alerts: true,
            codex_tray_window: TRAY_WINDOW_AUTO.to_owned(),
            claude_tray_window: TRAY_WINDOW_AUTO.to_owned(),
            combined_tray: true,
            onboarding_complete: false,
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
            .and_then(|raw| serde_json::from_str::<AppPrefs>(&raw).ok())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_full_meter_with_alerts_on() {
        let prefs = AppPrefs::default();
        assert!(!prefs.compact_tray);
        assert!(prefs.usage_alerts);
        assert_eq!(prefs.codex_tray_window, TRAY_WINDOW_AUTO);
        assert_eq!(prefs.claude_tray_window, TRAY_WINDOW_AUTO);
        assert!(!prefs.onboarding_complete);
    }

    #[test]
    fn unknown_and_missing_keys_fall_back_to_defaults() {
        // Preference files written by older builds must still load.
        let prefs: AppPrefs =
            serde_json::from_str(r#"{"compactTray":true,"claudeIncludeScoped":false}"#).unwrap();
        assert!(prefs.compact_tray);
        assert!(prefs.usage_alerts);
        assert_eq!(prefs.claude_tray_window, TRAY_WINDOW_AUTO);
    }

    #[test]
    fn update_persists_and_reloads() {
        let path = std::env::temp_dir().join(format!("usagebar-prefs-{}.json", std::process::id()));
        let store = PrefsStore::load(path.clone());
        store.update(|prefs| prefs.compact_tray = true);
        let reloaded = PrefsStore::load(path.clone());
        assert!(reloaded.get().compact_tray);
        assert!(reloaded.get().usage_alerts);
        let _ = fs::remove_file(path);
    }
}
