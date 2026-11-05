//! Local threshold notifications: "80% of the weekly window is gone" and
//! "a fresh window just started". Everything is computed from the normalized
//! rate-limit payloads both providers already emit; nothing leaves the Mac.

use std::collections::HashMap;

use serde_json::Value;
use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::Mutex;

use crate::prefs::PrefsStore;

/// Fallback levels when preferences are unavailable. The stored preference
/// is the source of truth; this only covers the first moments before the
/// prefs store is managed.
pub const DEFAULT_THRESHOLDS: [f64; 2] = [95.0, 80.0];

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AlertKind {
    Threshold(f64),
    FreshWindow,
}

/// What one refresh said about one window: how much is used and when the
/// window ends. Kept per window between refreshes so the next one can tell a
/// renewal apart from a number that merely dipped.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Observation {
    pub used: f64,
    pub resets_at: Option<u64>,
}

/// A renewed window must have been at least this full before it is
/// announced. A window that barely moved renews unnoticed every few hours for
/// a light user, and telling them about it each time would be noise.
const RENEWAL_MIN_PREVIOUS_USED: f64 = 25.0;
/// How far `resetsAt` has to move forward to count as a new window. Real
/// windows are an hour or longer, while the same window's timestamp can wobble
/// by a second between requests (Claude reports fractional seconds), and a
/// rolling window creeps forward a little with every refresh.
const RENEWAL_MIN_RESET_ADVANCE_SECS: u64 = 15 * 60;
/// Without reset times, the only evidence is the number itself, so the old,
/// stricter rule applies: from at least this full...
const LEGACY_RENEWAL_FROM: f64 = 50.0;
/// ...to below this.
const LEGACY_RENEWAL_TO: f64 = 10.0;

/// Pure crossing logic: what changed between two observations of one window?
/// `thresholds` must be descending so one refresh that jumps past several
/// levels produces a single, most-urgent notification. `now` is unix seconds.
pub fn crossing(
    previous: &Observation,
    current: &Observation,
    now: u64,
    thresholds: &[f64],
) -> Option<AlertKind> {
    for threshold in thresholds {
        if previous.used < *threshold && current.used >= *threshold {
            return Some(AlertKind::Threshold(*threshold));
        }
    }
    renewed(previous, current, now).then_some(AlertKind::FreshWindow)
}

/// Whether `current` belongs to a fresh window that is worth announcing.
///
/// Usage has to have been meaningful and has to at least halve, so a rolling
/// window shedding a little old usage never qualifies. Beyond that the reset
/// time decides wherever it can:
/// - Both readings have one: a renewal moves `resetsAt` forward by about a
///   window's length. A big drop under an unchanged reset time is a glitch
///   in the data, not a new window, and stays quiet.
/// - The old reading had one that has now passed, and the new one has none:
///   the window ran out and the next has not started yet (Claude reports an
///   idle 5-hour window this way).
/// - Otherwise only the size of the drop can say anything, so it has to be
///   dramatic.
fn renewed(previous: &Observation, current: &Observation, now: u64) -> bool {
    if previous.used < RENEWAL_MIN_PREVIOUS_USED || current.used > previous.used / 2.0 {
        return false;
    }
    match (previous.resets_at, current.resets_at) {
        (Some(before), Some(after)) => {
            after >= before.saturating_add(RENEWAL_MIN_RESET_ADVANCE_SECS)
        }
        (Some(before), None) if now >= before => true,
        _ => previous.used >= LEGACY_RENEWAL_FROM && current.used < LEGACY_RENEWAL_TO,
    }
}

#[derive(Debug, PartialEq)]
pub struct WindowSample {
    pub key: String,
    pub label: String,
    pub used: f64,
    pub resets_at: Option<u64>,
}

impl WindowSample {
    fn observation(&self) -> Observation {
        Observation {
            used: self.used,
            resets_at: self.resets_at,
        }
    }
}

fn duration_label(minutes: Option<f64>) -> Option<String> {
    let minutes = minutes?;
    Some(match minutes {
        10_080.0 => "weekly".to_owned(),
        1_440.0 => "daily".to_owned(),
        300.0 => "5-hour".to_owned(),
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
    if let Some(by_id) = payload
        .get("rateLimitsByLimitId")
        .and_then(Value::as_object)
    {
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
    seen: Mutex<HashMap<String, Observation>>,
}

impl UsageAlerts {
    /// Feed one refreshed payload through the alert state machine. The first
    /// observation of each window seeds silently, so an app restart never
    /// re-notifies about a window that was already past a threshold.
    pub async fn observe(&self, app: &AppHandle, provider: &str, payload: &Value) {
        let prefs = app.try_state::<PrefsStore>().map(|store| store.get());
        let enabled = prefs.as_ref().map(|p| p.usage_alerts).unwrap_or(true);
        let thresholds = prefs
            .as_ref()
            .map(|p| p.alert_thresholds())
            .unwrap_or_else(|| DEFAULT_THRESHOLDS.to_vec());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut seen = self.seen.lock().await;
        for window in extract_windows(provider, payload) {
            let current = window.observation();
            let previous = seen.insert(window.key.clone(), current);
            let Some(previous) = previous else {
                continue;
            };
            if !enabled {
                continue;
            }
            let Some(alert) = crossing(&previous, &current, now, &thresholds) else {
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

    const NOW: u64 = 1_000_000;
    const HOUR: u64 = 3_600;

    fn obs(used: f64, resets_at: Option<u64>) -> Observation {
        Observation { used, resets_at }
    }

    /// Two readings of a window that reported no reset time.
    fn bare(previous: f64, current: f64, thresholds: &[f64]) -> Option<AlertKind> {
        crossing(&obs(previous, None), &obs(current, None), NOW, thresholds)
    }

    #[test]
    fn crossings_fire_once_and_prefer_the_higher_threshold() {
        assert_eq!(
            bare(70.0, 85.0, &DEFAULT_THRESHOLDS),
            Some(AlertKind::Threshold(80.0))
        );
        assert_eq!(
            bare(70.0, 97.0, &DEFAULT_THRESHOLDS),
            Some(AlertKind::Threshold(95.0))
        );
        assert_eq!(bare(85.0, 90.0, &DEFAULT_THRESHOLDS), None);
        assert_eq!(
            bare(85.0, 96.0, &DEFAULT_THRESHOLDS),
            Some(AlertKind::Threshold(95.0))
        );
        assert_eq!(bare(96.0, 97.0, &DEFAULT_THRESHOLDS), None);
    }

    #[test]
    fn custom_thresholds_are_honored() {
        assert_eq!(
            bare(55.0, 62.0, &[90.0, 60.0]),
            Some(AlertKind::Threshold(60.0))
        );
        assert_eq!(bare(91.0, 92.0, &[90.0, 60.0]), None);
    }

    #[test]
    fn thresholds_ignore_reset_times() {
        let reset = Some(NOW + HOUR);
        assert_eq!(
            crossing(
                &obs(70.0, reset),
                &obs(85.0, reset),
                NOW,
                &DEFAULT_THRESHOLDS
            ),
            Some(AlertKind::Threshold(80.0))
        );
        assert_eq!(
            crossing(
                &obs(85.0, reset),
                &obs(90.0, reset),
                NOW,
                &DEFAULT_THRESHOLDS
            ),
            None
        );
    }

    #[test]
    fn without_reset_times_only_a_big_drop_is_a_fresh_window() {
        assert_eq!(
            bare(84.0, 2.0, &DEFAULT_THRESHOLDS),
            Some(AlertKind::FreshWindow)
        );
        assert_eq!(bare(30.0, 2.0, &DEFAULT_THRESHOLDS), None);
        assert_eq!(bare(84.0, 20.0, &DEFAULT_THRESHOLDS), None);
    }

    #[test]
    fn a_reset_time_moving_forward_announces_a_modest_window_too() {
        // 30% used, then the 5-hour window rolls over: this used to pass
        // silently because only a drop from 50% or more counted.
        let old = obs(30.0, Some(NOW - 60));
        let new = obs(0.0, Some(NOW + 5 * HOUR));
        assert_eq!(
            crossing(&old, &new, NOW, &DEFAULT_THRESHOLDS),
            Some(AlertKind::FreshWindow)
        );
        // Activity right after the renewal still counts, as long as usage at
        // least halved.
        let busy = obs(12.0, Some(NOW + 5 * HOUR));
        assert_eq!(
            crossing(&old, &busy, NOW, &DEFAULT_THRESHOLDS),
            Some(AlertKind::FreshWindow)
        );
        // An early reset by the provider (the old reset time is still ahead)
        // is still a fresh window.
        let early = obs(70.0, Some(NOW + 2 * HOUR));
        let reset = obs(0.0, Some(NOW + 7 * 24 * HOUR));
        assert_eq!(
            crossing(&early, &reset, NOW, &DEFAULT_THRESHOLDS),
            Some(AlertKind::FreshWindow)
        );
    }

    #[test]
    fn a_renewal_after_light_use_stays_quiet() {
        let old = obs(10.0, Some(NOW - 60));
        let new = obs(0.0, Some(NOW + 5 * HOUR));
        assert_eq!(crossing(&old, &new, NOW, &DEFAULT_THRESHOLDS), None);
        let old = obs(RENEWAL_MIN_PREVIOUS_USED - 0.5, Some(NOW - 60));
        assert_eq!(crossing(&old, &new, NOW, &DEFAULT_THRESHOLDS), None);
    }

    #[test]
    fn a_drop_under_the_same_reset_time_is_a_glitch_not_a_renewal() {
        // A momentary bad reading used to be announced as a fresh window.
        let reset = Some(NOW + 3 * HOUR);
        assert_eq!(
            crossing(
                &obs(84.0, reset),
                &obs(2.0, reset),
                NOW,
                &DEFAULT_THRESHOLDS
            ),
            None
        );
        // A second of wobble in the reported reset time is the same window.
        assert_eq!(
            crossing(
                &obs(84.0, reset),
                &obs(2.0, reset.map(|at| at + 1)),
                NOW,
                &DEFAULT_THRESHOLDS
            ),
            None
        );
    }

    #[test]
    fn a_rolling_window_creeping_forward_is_not_a_renewal() {
        // Old usage ages out of a rolling window: the reset time edges
        // forward and usage dips a little.
        let old = obs(40.0, Some(NOW + HOUR));
        let new = obs(36.0, Some(NOW + HOUR + 10 * 60));
        assert_eq!(crossing(&old, &new, NOW, &DEFAULT_THRESHOLDS), None);
        // Even a long jump in the reset time needs usage to actually fall.
        let jumped = obs(36.0, Some(NOW + 6 * HOUR));
        assert_eq!(crossing(&old, &jumped, NOW, &DEFAULT_THRESHOLDS), None);
    }

    #[test]
    fn a_lapsed_window_with_no_successor_yet_is_fresh() {
        // Claude reports an idle 5-hour window with no reset time at all.
        let old = obs(30.0, Some(NOW - 60));
        assert_eq!(
            crossing(&old, &obs(0.0, None), NOW, &DEFAULT_THRESHOLDS),
            Some(AlertKind::FreshWindow)
        );
        // The reset time going missing before the window ends proves
        // nothing, so the strict size-of-drop rule applies.
        let running = obs(30.0, Some(NOW + HOUR));
        assert_eq!(
            crossing(&running, &obs(0.0, None), NOW, &DEFAULT_THRESHOLDS),
            None
        );
        let running = obs(84.0, Some(NOW + HOUR));
        assert_eq!(
            crossing(&running, &obs(2.0, None), NOW, &DEFAULT_THRESHOLDS),
            Some(AlertKind::FreshWindow)
        );
        // Once the next window starts, the idle reading is the baseline, so
        // the renewal is not announced twice.
        assert_eq!(
            crossing(
                &obs(0.0, None),
                &obs(3.0, Some(NOW + 5 * HOUR)),
                NOW,
                &DEFAULT_THRESHOLDS
            ),
            None
        );
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
