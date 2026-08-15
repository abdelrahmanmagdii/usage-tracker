//! Local threshold notifications: "80% of the weekly window is gone" and
//! "a fresh window just started". Everything is computed from the normalized
//! rate-limit payloads both providers already emit; nothing leaves the Mac.

use std::collections::HashMap;

use serde_json::Value;
use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::Mutex;

use crate::prefs::PrefsStore;

/// Crossing thresholds, checked highest-first so one refresh that jumps past
/// both 80 and 95 produces a single, most-urgent notification.
const THRESHOLDS: [f64; 2] = [95.0, 80.0];

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AlertKind {
    Threshold(f64),
    FreshWindow,
}

/// Pure crossing logic: what changed between two observations of one window?
pub fn crossing(previous: f64, current: f64) -> Option<AlertKind> {
    for threshold in THRESHOLDS {
        if previous < threshold && current >= threshold {
            return Some(AlertKind::Threshold(threshold));
        }
    }
    if previous >= 50.0 && current < 10.0 {
        return Some(AlertKind::FreshWindow);
    }
    None
}

#[derive(Debug, PartialEq)]
pub struct WindowSample {
    pub key: String,
    pub label: String,
    pub used: f64,
    pub resets_at: Option<u64>,
}

fn duration_label(minutes: Option<f64>) -> Option<String> {
    let minutes = minutes?;
    Some(match minutes {
        m if m == 10_080.0 => "weekly".to_owned(),
        m if m == 1_440.0 => "daily".to_owned(),
        m if m == 300.0 => "5-hour".to_owned(),
        m if m > 0.0 && m % 60.0 == 0.0 => format!("{}-hour", (m / 60.0) as u64),
        _ => return None,
    })
}

fn collect_snapshot(provider: &str, id: &str, snapshot: &Value, out: &mut Vec<WindowSample>) {
    for kind in ["primary", "secondary"] {
        let Some(window) = snapshot.get(kind) else {
            continue;
        };
        let Some(used) = window.get("usedPercent").and_then(Value::as_f64) else {
            continue;
        };
        let label = snapshot
            .get("windowLabel")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| duration_label(window.get("windowDurationMins").and_then(Value::as_f64)))
            .unwrap_or_else(|| "usage".to_owned());
        out.push(WindowSample {
            key: format!("{provider}:{id}:{kind}"),
            label,
            used,
            resets_at: window
                .get("resetsAt")
                .and_then(Value::as_f64)
                .map(|value| value.max(0.0) as u64),
        });
    }
}

pub fn extract_windows(provider: &str, payload: &Value) -> Vec<WindowSample> {
    let mut out = Vec::new();
    if let Some(by_id) = payload.get("rateLimitsByLimitId").and_then(Value::as_object) {
        for (id, snapshot) in by_id {
            collect_snapshot(provider, id, snapshot, &mut out);
        }
    }
    if out.is_empty() {
        if let Some(snapshot) = payload.get("rateLimits") {
            collect_snapshot(provider, "default", snapshot, &mut out);
        }
    }
    out
}

#[derive(Default)]
pub struct UsageAlerts {
    seen: Mutex<HashMap<String, f64>>,
}

impl UsageAlerts {
    /// Feed one refreshed payload through the alert state machine. The first
    /// observation of each window seeds silently, so an app restart never
    /// re-notifies about a window that was already past a threshold.
    pub async fn observe(&self, app: &AppHandle, provider: &str, payload: &Value) {
        let enabled = app
            .try_state::<PrefsStore>()
            .map(|prefs| prefs.get().usage_alerts)
            .unwrap_or(true);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut seen = self.seen.lock().await;
        for window in extract_windows(provider, payload) {
            let previous = seen.insert(window.key.clone(), window.used);
            let Some(previous) = previous else {
                continue;
            };
            if !enabled {
                continue;
            }
            let Some(alert) = crossing(previous, window.used) else {
                continue;
            };
            let (title, body) = match alert {
                AlertKind::Threshold(threshold) => (
                    format!("{provider}: {} window at {threshold:.0}%", window.label),
                    match window.resets_at {
                        Some(resets_at) if resets_at > now => format!(
                            "{:.0}% used · resets in {}",
                            window.used,
                            crate::tray::format_countdown(resets_at - now)
                        ),
                        _ => format!("{:.0}% used", window.used),
                    },
                ),
                AlertKind::FreshWindow => (
                    format!("{provider}: fresh {} window", window.label),
                    format!("Usage is back down to {:.0}%", window.used),
                ),
            };
            let _ = app.notification().builder().title(title).body(body).show();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn crossings_fire_once_and_prefer_the_higher_threshold() {
        assert_eq!(crossing(70.0, 85.0), Some(AlertKind::Threshold(80.0)));
        assert_eq!(crossing(70.0, 97.0), Some(AlertKind::Threshold(95.0)));
        assert_eq!(crossing(85.0, 90.0), None);
        assert_eq!(crossing(85.0, 96.0), Some(AlertKind::Threshold(95.0)));
        assert_eq!(crossing(96.0, 97.0), None);
    }

    #[test]
    fn a_big_drop_is_a_fresh_window() {
        assert_eq!(crossing(84.0, 2.0), Some(AlertKind::FreshWindow));
        assert_eq!(crossing(30.0, 2.0), None);
        assert_eq!(crossing(84.0, 20.0), None);
    }

    #[test]
    fn extracts_labeled_windows_from_both_payload_shapes() {
        let by_id = json!({
            "rateLimitsByLimitId": {
                "weekly-scoped-fable": {
                    "windowLabel": "Fable",
                    "secondary": { "usedPercent": 63, "windowDurationMins": 10_080, "resetsAt": 2_000 }
                },
                "session": {
                    "primary": { "usedPercent": 5, "windowDurationMins": 300 }
                }
            }
        });
        let windows = extract_windows("Claude Code", &by_id);
        assert_eq!(windows.len(), 2);
        let fable = windows.iter().find(|w| w.key.contains("fable")).unwrap();
        assert_eq!(fable.label, "Fable");
        assert_eq!(fable.resets_at, Some(2_000));
        let session = windows.iter().find(|w| w.key.contains("session")).unwrap();
        assert_eq!(session.label, "5-hour");

        let flat = json!({ "rateLimits": { "primary": { "usedPercent": 20, "windowDurationMins": 300 } } });
        let windows = extract_windows("Codex", &flat);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].key, "Codex:default:primary");
    }
}
