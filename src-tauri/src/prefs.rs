use std::{fs, path::PathBuf, sync::Mutex};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct AppPrefs {
    /// Tray shows only the percentage, no countdown.
    pub compact_tray: bool,
    /// Local notifications at 80%/95% used and when a window resets.
    pub usage_alerts: bool,
    /// Claude tray considers model-scoped windows (e.g. Fable weekly) too.
    /// On by default: the scoped limit is usually the one that actually binds.
    pub claude_include_scoped: bool,
}

impl Default for AppPrefs {
    fn default() -> Self {
        Self {
            compact_tray: false,
            usage_alerts: true,
            claude_include_scoped: true,
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
        *self.state.lock().expect("preferences poisoned")
    }

    pub fn update(&self, mutate: impl FnOnce(&mut AppPrefs)) -> AppPrefs {
        let mut state = self.state.lock().expect("preferences poisoned");
        mutate(&mut state);
        let snapshot = *state;
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
