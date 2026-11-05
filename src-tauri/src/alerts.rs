//! Local threshold notifications: "80% of the weekly window is gone", "a
//! fresh window just started" and "at this pace the weekly window runs out
//! well before it resets". Everything is computed from the normalized
//! rate-limit payloads the providers already emit; nothing leaves the Mac.

use std::collections::{HashMap, VecDeque};

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
    /// The window's length, when the provider reports it.
    pub duration_mins: Option<f64>,
    /// The provider says this limit is already hit (the popover's `reached`).
    pub reached: bool,
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
        // Mirrors `reached` in `parseWindow` (src/lib/rateLimits.ts).
        let reached = snapshot
            .get("rateLimitReachedType")
            .is_some_and(|value| !value.is_null())
            || snapshot.get("spendControlReached") == Some(&Value::Bool(true))
            || used >= 100.0;
        out.push(WindowSample {
            key: format!("{provider}:{id}:{kind}"),
            label,
            used,
            resets_at: window
                .get("resetsAt")
                .and_then(Value::as_f64)
                .map(|value| value.max(0.0) as u64),
            duration_mins: window.get("windowDurationMins").and_then(Value::as_f64),
            reached,
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

// ---------------------------------------------------------------------------
// Pace: "at this rate the window runs out before it resets".
//
// The projection is a port of `projectedRunOut` in src/lib/history.ts, so the
// notification and the popover's "At this pace, runs out in …" line agree on
// the same samples (src/lib/pace-fixtures.json is checked by both test
// suites). When that projection is worth a notification is decided by
// stricter rules than the popover's, because a notification interrupts.
// ---------------------------------------------------------------------------

/// `PACE_LOOKBACK_MS` in history.ts: long enough to smooth one burst, short
/// enough to follow today's pace rather than last week's.
pub const PACE_LOOKBACK_MS: u64 = 3 * 3_600_000;
/// `PACE_MIN_SPAN_MS` in history.ts.
pub const PACE_MIN_SPAN_MS: u64 = 10 * 60_000;
/// `SAME_WINDOW_TOLERANCE_S` in history.ts: reset times wobble by seconds
/// between polls, while a renewal moves them by a whole window.
pub const SAME_WINDOW_TOLERANCE_SECS: u64 = 90;

/// Only warn about a run-out at most this far ahead. Every window gets the
/// same horizon because the projection never knows more than its lookback:
/// it extrapolates the last three hours at most, so it is a fair guess for
/// about the next three. For a 5-hour window that is most of the window and
/// the margin below decides; for weekly and monthly windows it keeps one busy
/// afternoon from becoming "runs out in 3 days", which a night's sleep would
/// make wrong anyway.
pub const PACE_ALERT_HORIZON_SECS: u64 = 3 * 3_600;
/// The run-out must come at least this long before the reset...
pub const PACE_ALERT_MIN_MARGIN_SECS: u64 = 30 * 60;
/// ...and at least this share of the window, so the lockout is a real part
/// of it: 30 minutes of a 5-hour window, 2.4 hours of a day, about 17 hours
/// of a week, 3 days of a month. Running dry half an hour before a weekly
/// reset is not worth an interruption.
pub const PACE_ALERT_MIN_MARGIN_FRACTION: f64 = 0.1;
/// The popover shows a projection after 10 minutes of samples; a
/// notification waits for half an hour. A burst shorter than that is one big
/// task, not a pace.
pub const PACE_ALERT_MIN_SPAN_MS: u64 = 30 * 60_000;
/// Refreshes in a row that must agree before the warning fires, so a single
/// odd reading (a spike the next poll walks back) cannot trigger it.
pub const PACE_ALERT_CONFIRMATIONS: u8 = 2;
/// Samples kept per window. With one sample per change in usage (like the
/// popover's history) and the lookback pruning older ones, a window holds a
/// few dozen at most; this only bounds a pathological provider.
const PACE_MAX_SAMPLES: usize = 256;

/// One recorded reading of a window, as the popover's history keeps it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaceSample {
    /// Unix milliseconds.
    pub at_ms: u64,
    pub used: f64,
    /// Unix seconds.
    pub resets_at: Option<u64>,
}

/// `inSameWindow` in history.ts.
pub fn in_same_window(sample_resets_at: Option<u64>, resets_at: Option<u64>) -> bool {
    match (sample_resets_at, resets_at) {
        (Some(sample), Some(current)) => sample.abs_diff(current) <= SAME_WINDOW_TOLERANCE_SECS,
        _ => false,
    }
}

/// The oldest sample the burn rate is measured from: same window, inside the
/// lookback, and only when there are at least two such samples.
fn pace_anchor(samples: &[PaceSample], resets_at: u64, now_ms: u64) -> Option<&PaceSample> {
    let usable = || {
        samples.iter().filter(move |sample| {
            in_same_window(sample.resets_at, Some(resets_at))
                && now_ms.saturating_sub(sample.at_ms) <= PACE_LOOKBACK_MS
        })
    };
    if usable().count() < 2 {
        return None;
    }
    usable().min_by_key(|sample| sample.at_ms)
}

/// `projectedRunOut` in history.ts: when the window runs dry at the recent
/// burn rate, if that is before it renews. Only samples from the current
/// window and the lookback count, they must span at least
/// `PACE_MIN_SPAN_MS`, and usage must have risen. `used` is the current
/// reading, clamped to 0–100 like the popover's bucket. Returns unix ms.
pub fn projected_run_out(
    samples: &[PaceSample],
    used: f64,
    resets_at: Option<u64>,
    reached: bool,
    now_ms: u64,
) -> Option<f64> {
    let resets_at = resets_at.filter(|at| *at > 0)?;
    if reached || used >= 100.0 {
        return None;
    }
    let first = pace_anchor(samples, resets_at, now_ms)?;
    let span_ms = now_ms as f64 - first.at_ms as f64;
    let used_delta = used - first.used;
    if span_ms < PACE_MIN_SPAN_MS as f64 || used_delta <= 0.0 {
        return None;
    }
    let run_out = now_ms as f64 + ((100.0 - used) / used_delta) * span_ms;
    (run_out < resets_at as f64 * 1000.0).then_some(run_out)
}

/// A projection that clears every bar for a notification.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaceWarning {
    /// Seconds from now until the window runs dry at this pace.
    pub runs_out_in: u64,
    /// Seconds between running dry and the reset.
    pub before_reset: u64,
}

/// Whether one projection is worth a notification, before the
/// once-per-window and confirmation rules (see `PaceTracker`).
///
/// `top_threshold` is the highest usage-alert level. Once usage is at or
/// past it, the "last call" alert has spoken for the window (or, right after
/// launch, the window is that full already): a pace warning on top would say
/// the same thing twice. Below it the two complement each other: thresholds
/// say how much is gone, the pace warning says when the rest will be.
pub fn pace_warning(
    samples: &[PaceSample],
    current: &WindowSample,
    now_ms: u64,
    top_threshold: f64,
) -> Option<PaceWarning> {
    if current.used >= top_threshold {
        return None;
    }
    let resets_at = current.resets_at?;
    let used = current.used.clamp(0.0, 100.0);
    let run_out_ms = projected_run_out(samples, used, Some(resets_at), current.reached, now_ms)?;
    // More evidence than the popover asks for.
    let first = pace_anchor(samples, resets_at, now_ms)?;
    if now_ms.saturating_sub(first.at_ms) < PACE_ALERT_MIN_SPAN_MS {
        return None;
    }
    let now = now_ms / 1000;
    let run_out = (run_out_ms / 1000.0).max(now as f64) as u64;
    let runs_out_in = run_out - now;
    let before_reset = resets_at.saturating_sub(run_out);
    let window_secs = current
        .duration_mins
        .filter(|mins| mins.is_finite() && *mins > 0.0)
        .map_or(0.0, |mins| mins * 60.0);
    let min_margin =
        (PACE_ALERT_MIN_MARGIN_SECS as f64).max(window_secs * PACE_ALERT_MIN_MARGIN_FRACTION);
    (runs_out_in <= PACE_ALERT_HORIZON_SECS && before_reset as f64 >= min_margin).then_some(
        PaceWarning {
            runs_out_in,
            before_reset,
        },
    )
}

/// What `PaceTracker::observe` needs to know beyond the reading itself.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaceContext {
    /// Usage alerts and pace alerts are both on.
    pub enabled: bool,
    /// A threshold or fresh-window notification for this window went out on
    /// this refresh. The pace warning then waits for the next refresh, so
    /// one refresh never stacks two notifications about one window.
    pub other_alert: bool,
    /// See `pace_warning`.
    pub top_threshold: f64,
}

/// Pace history and firing state for one window (provider:limit:kind).
#[derive(Debug, Default)]
pub struct PaceTracker {
    samples: VecDeque<PaceSample>,
    /// Consecutive refreshes whose projection qualified.
    streak: u8,
    /// `resetsAt` of the window instance already warned about. A renewal
    /// moves it by a whole window and re-arms the warning.
    warned_for: Option<u64>,
}

impl PaceTracker {
    /// Record one successful refresh of the window and say whether to warn
    /// now. Fires at most once per window instance, never on the first
    /// reading (one sample is not a pace), and only once the warning held
    /// for `PACE_ALERT_CONFIRMATIONS` refreshes in a row.
    pub fn observe(
        &mut self,
        current: &WindowSample,
        now_ms: u64,
        context: PaceContext,
    ) -> Option<PaceWarning> {
        self.record(current, now_ms);
        let warning = pace_warning(
            self.samples.make_contiguous(),
            current,
            now_ms,
            context.top_threshold,
        );
        if !context.enabled || warning.is_none() || self.already_warned(current.resets_at) {
            self.streak = 0;
            return None;
        }
        self.streak = self.streak.saturating_add(1);
        if self.streak < PACE_ALERT_CONFIRMATIONS || context.other_alert {
            return None;
        }
        self.warned_for = current.resets_at;
        self.streak = 0;
        warning
    }

    fn already_warned(&self, resets_at: Option<u64>) -> bool {
        match (self.warned_for, resets_at) {
            // The renewal rule: the reset time has to move forward by far
            // more than a timestamp wobbles or a rolling window creeps.
            (Some(warned), Some(current)) => {
                current < warned.saturating_add(RENEWAL_MIN_RESET_ADVANCE_SECS)
            }
            _ => false,
        }
    }

    /// The popover's history rules: a new sample only when usage or the
    /// reset time changed, so a flat stretch weighs on the burn rate exactly
    /// as it does on screen. A new window starts a fresh history. Samples
    /// past the lookback are dropped, except the newest, which is still what
    /// the next reading is compared with.
    fn record(&mut self, current: &WindowSample, now_ms: u64) {
        let sample = PaceSample {
            at_ms: now_ms,
            used: current.used.clamp(0.0, 100.0),
            resets_at: current.resets_at,
        };
        match self.samples.back() {
            Some(last) if last.used == sample.used && last.resets_at == sample.resets_at => {}
            Some(last) if !in_same_window(last.resets_at, sample.resets_at) => {
                self.samples.clear();
                self.streak = 0;
                self.samples.push_back(sample);
            }
            _ => self.samples.push_back(sample),
        }
        while self.samples.len() > 1
            && self
                .samples
                .front()
                .is_some_and(|first| now_ms.saturating_sub(first.at_ms) > PACE_LOOKBACK_MS)
        {
            self.samples.pop_front();
        }
        while self.samples.len() > PACE_MAX_SAMPLES {
            self.samples.pop_front();
        }
    }
}

/// "Claude Code: weekly window runs out in ~2:00:00" /
/// "72% used · at this pace, empty 1d 4h before it resets".
fn pace_notification(
    provider: &str,
    window: &WindowSample,
    warning: PaceWarning,
) -> (String, String) {
    // An estimate: whole minutes, marked with "~".
    let runs_out_in = warning.runs_out_in.div_ceil(60).max(1) * 60;
    (
        format!(
            "{provider}: {} window runs out in ~{}",
            window.label,
            crate::tray::format_countdown(runs_out_in)
        ),
        format!(
            "{:.0}% used · at this pace, empty {} before it resets",
            window.used,
            crate::tray::format_countdown(warning.before_reset)
        ),
    )
}

#[derive(Default)]
pub struct UsageAlerts {
    seen: Mutex<HashMap<String, Observation>>,
    pace: Mutex<HashMap<String, PaceTracker>>,
}

impl UsageAlerts {
    /// Feed one refreshed payload through the alert state machine. The first
    /// observation of each window seeds silently, so an app restart never
    /// re-notifies about a window that was already past a threshold.
    ///
    /// Providers call this only after a successful refresh, right after they
    /// mark themselves connected with a fresh `updated_at`, so every reading
    /// here is live: a failing or signed-out provider feeds nothing, and the
    /// pace warning can never extrapolate from a stale number.
    pub async fn observe(&self, app: &AppHandle, provider: &str, payload: &Value) {
        let prefs = app.try_state::<PrefsStore>().map(|store| store.get());
        let enabled = prefs.as_ref().map(|p| p.usage_alerts).unwrap_or(true);
        let pace_enabled = enabled && prefs.as_ref().map(|p| p.pace_alerts).unwrap_or(true);
        let thresholds = prefs
            .as_ref()
            .map(|p| p.alert_thresholds())
            .unwrap_or_else(|| DEFAULT_THRESHOLDS.to_vec());
        let top_threshold = thresholds.iter().copied().reduce(f64::max).unwrap_or(100.0);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let now = now_ms / 1000;
        let mut seen = self.seen.lock().await;
        let mut pace = self.pace.lock().await;
        for window in extract_windows(provider, payload) {
            let current = window.observation();
            let previous = seen.insert(window.key.clone(), current);
            let alert = match previous {
                Some(previous) if enabled => crossing(&previous, &current, now, &thresholds),
                _ => None,
            };
            // Always recorded, so turning pace alerts on mid-window has
            // history to work with; a first sighting only seeds it.
            let warning = pace.entry(window.key.clone()).or_default().observe(
                &window,
                now_ms,
                PaceContext {
                    enabled: pace_enabled && previous.is_some(),
                    other_alert: alert.is_some(),
                    top_threshold,
                },
            );
            if let Some(warning) = warning {
                let (title, body) = pace_notification(provider, &window, warning);
                let _ = app.notification().builder().title(title).body(body).show();
            }
            let Some(alert) = alert else {
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
        assert_eq!(windows[0].duration_mins, Some(300.0));
        assert!(!windows[0].reached);
    }

    #[test]
    fn extracts_whether_a_limit_is_reached_like_the_popover() {
        let payload = json!({
            "rateLimitsByLimitId": {
                "hit": { "rateLimitReachedType": "limit_reached", "primary": { "usedPercent": 40 } },
                "unset": { "rateLimitReachedType": null, "primary": { "usedPercent": 40 } },
                "spend": { "spendControlReached": true, "primary": { "usedPercent": 40 } },
                "full": { "primary": { "usedPercent": 100 } }
            }
        });
        let windows = extract_windows("Cursor", &payload);
        let reached = |id: &str| windows.iter().find(|w| w.key.contains(id)).unwrap().reached;
        assert!(reached("hit"));
        assert!(!reached("unset"));
        assert!(reached("spend"));
        assert!(reached("full"));
    }

    // ---- Pace ------------------------------------------------------------

    const MIN_MS: u64 = 60_000;
    const T0_MS: u64 = 1_791_028_800_000;
    const WEEK_MINS: f64 = 10_080.0;

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Fixtures {
        now_ms: u64,
        cases: Vec<FixtureCase>,
    }

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct FixtureCase {
        name: String,
        samples: Vec<FixtureSample>,
        bucket: FixtureBucket,
        expected_run_out_ms: Option<f64>,
    }

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct FixtureSample {
        mins_ago: u64,
        used: f64,
        resets_at: Option<u64>,
    }

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct FixtureBucket {
        used_percent: f64,
        resets_at: Option<u64>,
        reached: bool,
    }

    #[test]
    fn projection_matches_the_popover_on_the_shared_fixtures() {
        // The same file drives src/lib/paceParity.test.ts against
        // `projectedRunOut`.
        let fixtures: Fixtures =
            serde_json::from_str(include_str!("../../src/lib/pace-fixtures.json")).unwrap();
        assert!(fixtures.cases.len() >= 10);
        for case in fixtures.cases {
            let samples: Vec<PaceSample> = case
                .samples
                .iter()
                .map(|s| PaceSample {
                    at_ms: fixtures.now_ms - s.mins_ago * MIN_MS,
                    used: s.used,
                    resets_at: s.resets_at,
                })
                .collect();
            let got = projected_run_out(
                &samples,
                case.bucket.used_percent,
                case.bucket.resets_at,
                case.bucket.reached,
                fixtures.now_ms,
            );
            match (got, case.expected_run_out_ms) {
                (None, None) => {}
                (Some(got), Some(expected)) => {
                    assert!(
                        (got - expected).abs() < 1.0,
                        "{}: {got} vs {expected}",
                        case.name
                    )
                }
                (got, expected) => panic!("{}: got {got:?}, expected {expected:?}", case.name),
            }
        }
    }

    fn reading(used: f64, resets_at: u64, duration_mins: f64) -> WindowSample {
        WindowSample {
            key: "Claude Code:weekly-all:secondary".to_owned(),
            label: "weekly".to_owned(),
            used,
            resets_at: Some(resets_at),
            duration_mins: Some(duration_mins),
            reached: false,
        }
    }

    fn sample(mins_ago: u64, used: f64, resets_at: u64) -> PaceSample {
        PaceSample {
            at_ms: T0_MS - mins_ago * MIN_MS,
            used,
            resets_at: Some(resets_at),
        }
    }

    const ON: PaceContext = PaceContext {
        enabled: true,
        other_alert: false,
        top_threshold: 95.0,
    };

    /// Feeds a weekly window rising 2% every 5 minutes from 50% (24%/h, so
    /// it runs dry in about two hours, two days before its reset) and
    /// returns the minutes at which a warning fired.
    fn burn(
        tracker: &mut PaceTracker,
        start_ms: u64,
        resets_at: u64,
        from: f64,
        steps: u64,
        context: impl Fn(u64) -> PaceContext,
    ) -> Vec<u64> {
        let mut fired = Vec::new();
        for step in 0..=steps {
            let minute = step * 5;
            let used = from + 2.0 * step as f64;
            let warning = tracker.observe(
                &reading(used, resets_at, WEEK_MINS),
                start_ms + minute * MIN_MS,
                context(minute),
            );
            if warning.is_some() {
                fired.push(minute);
            }
        }
        fired
    }

    #[test]
    fn warns_once_after_half_an_hour_of_pace_confirmed_twice() {
        let resets_at = T0_MS / 1000 + 2 * 24 * HOUR;
        let mut tracker = PaceTracker::default();
        // The popover projects from 10 minutes on; the notification waits
        // for 30 minutes of evidence, then one more refresh to confirm.
        assert_eq!(
            burn(&mut tracker, T0_MS, resets_at, 50.0, 8, |_| ON),
            vec![35]
        );
        // Still the same window: never again, however long the burn goes on.
        let later = T0_MS + 45 * MIN_MS;
        assert_eq!(
            burn(&mut tracker, later, resets_at, 68.0, 5, |_| ON),
            Vec::<u64>::new()
        );
    }

    #[test]
    fn the_warning_carries_run_out_and_lead_over_the_reset() {
        let resets_at = T0_MS / 1000 + 2 * 24 * HOUR;
        let samples = [sample(60, 40.0, resets_at), sample(30, 50.0, resets_at)];
        let warning = pace_warning(&samples, &reading(60.0, resets_at, WEEK_MINS), T0_MS, 95.0)
            .expect("warning");
        assert_eq!(warning.runs_out_in, 2 * HOUR);
        assert_eq!(warning.before_reset, 46 * HOUR);
        let (title, body) =
            pace_notification("Claude Code", &reading(60.0, resets_at, WEEK_MINS), warning);
        assert_eq!(title, "Claude Code: weekly window runs out in ~2:00:00");
        assert_eq!(
            body,
            "60% used · at this pace, empty 1d 22h before it resets"
        );
    }

    #[test]
    fn never_warns_from_a_single_reading() {
        // The first sighting after launch is one sample: no pace at all,
        // however full the window.
        let resets_at = T0_MS / 1000 + 2 * 24 * HOUR;
        let mut tracker = PaceTracker::default();
        assert_eq!(
            tracker.observe(&reading(90.0, resets_at, WEEK_MINS), T0_MS, ON),
            None
        );
        assert_eq!(
            tracker.observe(&reading(90.0, resets_at, WEEK_MINS), T0_MS + MIN_MS, ON),
            None
        );
    }

    #[test]
    fn a_renewed_window_re_arms_the_warning() {
        let resets_at = T0_MS / 1000 + 2 * 24 * HOUR;
        let mut tracker = PaceTracker::default();
        assert_eq!(
            burn(&mut tracker, T0_MS, resets_at, 50.0, 8, |_| ON),
            vec![35]
        );
        // A week later the next window burns just as hard: warned again,
        // with no samples carried over from the old window.
        let next_start = T0_MS + 2 * 24 * HOUR * 1000;
        let next_reset = resets_at + 7 * 24 * HOUR;
        let fired = burn(&mut tracker, next_start, next_reset, 50.0, 8, |_| ON);
        assert_eq!(fired, vec![35]);
    }

    #[test]
    fn a_reset_time_wobbling_or_creeping_does_not_re_arm() {
        let resets_at = T0_MS / 1000 + 2 * 24 * HOUR;
        let mut tracker = PaceTracker::default();
        assert_eq!(
            burn(&mut tracker, T0_MS, resets_at, 50.0, 8, |_| ON),
            vec![35]
        );
        assert!(tracker.already_warned(Some(resets_at + 60)));
        assert!(tracker.already_warned(Some(resets_at + 10 * 60)));
        assert!(!tracker.already_warned(Some(resets_at + RENEWAL_MIN_RESET_ADVANCE_SECS)));
    }

    #[test]
    fn one_qualifying_refresh_between_misses_is_not_enough() {
        let resets_at = T0_MS / 1000 + 2 * 24 * HOUR;
        let mut tracker = PaceTracker::default();
        // Every other refresh is suppressed (as a dip in the projection
        // would): the streak never reaches two.
        let fired = burn(&mut tracker, T0_MS, resets_at, 50.0, 12, |minute| {
            PaceContext {
                enabled: minute % 10 != 0,
                ..ON
            }
        });
        assert_eq!(fired, Vec::<u64>::new());
    }

    #[test]
    fn waits_a_refresh_when_another_alert_just_fired_for_the_window() {
        let resets_at = T0_MS / 1000 + 2 * 24 * HOUR;
        let mut tracker = PaceTracker::default();
        let fired = burn(&mut tracker, T0_MS, resets_at, 50.0, 8, |minute| {
            PaceContext {
                other_alert: minute == 35,
                ..ON
            }
        });
        assert_eq!(fired, vec![40]);
    }

    #[test]
    fn disabled_pace_alerts_stay_quiet_but_keep_history() {
        let resets_at = T0_MS / 1000 + 2 * 24 * HOUR;
        let mut tracker = PaceTracker::default();
        let off = PaceContext {
            enabled: false,
            ..ON
        };
        assert_eq!(
            burn(&mut tracker, T0_MS, resets_at, 50.0, 8, |_| off),
            Vec::<u64>::new()
        );
        // Turned on mid-window: the recorded history counts, and two fresh
        // confirmations are still required.
        let fired = burn(
            &mut tracker,
            T0_MS + 45 * MIN_MS,
            resets_at,
            68.0,
            3,
            |_| ON,
        );
        assert_eq!(fired, vec![5]);
    }

    #[test]
    fn no_warning_at_or_past_the_last_call_threshold() {
        let resets_at = T0_MS / 1000 + 2 * 24 * HOUR;
        let samples = [sample(60, 80.0, resets_at), sample(30, 90.0, resets_at)];
        let current = reading(95.0, resets_at, WEEK_MINS);
        assert_eq!(pace_warning(&samples, &current, T0_MS, 95.0), None);
        // A higher last call leaves room for the pace warning.
        assert!(pace_warning(&samples, &current, T0_MS, 100.0).is_some());
    }

    #[test]
    fn a_run_out_far_ahead_is_left_to_the_popover() {
        // 10% an hour with 40% left: dry in 4h, past the 3h horizon, even
        // though the weekly reset is days away.
        let resets_at = T0_MS / 1000 + 3 * 24 * HOUR;
        let samples = [sample(60, 50.0, resets_at), sample(30, 55.0, resets_at)];
        assert_eq!(
            pace_warning(&samples, &reading(60.0, resets_at, WEEK_MINS), T0_MS, 95.0),
            None
        );
        // 20% an hour: dry in 2h, inside it.
        let samples = [sample(60, 40.0, resets_at), sample(30, 50.0, resets_at)];
        assert!(
            pace_warning(&samples, &reading(60.0, resets_at, WEEK_MINS), T0_MS, 95.0).is_some()
        );
    }

    #[test]
    fn the_run_out_must_come_meaningfully_before_the_reset() {
        // 5-hour window: dry in 2h. A reset 20 minutes after that is not
        // worth a notification; 40 minutes (past the 30-minute floor) is.
        let samples = |resets_at| [sample(60, 40.0, resets_at), sample(30, 50.0, resets_at)];
        let five_hour = |resets_at| reading(60.0, resets_at, 300.0);
        let close = T0_MS / 1000 + 2 * HOUR + 20 * 60;
        assert_eq!(
            pace_warning(&samples(close), &five_hour(close), T0_MS, 95.0),
            None
        );
        let clear = T0_MS / 1000 + 2 * HOUR + 40 * 60;
        assert!(pace_warning(&samples(clear), &five_hour(clear), T0_MS, 95.0).is_some());
        // A weekly window needs a tenth of a week (about 17 hours) of lead.
        let weekly = |resets_at| reading(60.0, resets_at, WEEK_MINS);
        let short = T0_MS / 1000 + 2 * HOUR + 12 * HOUR;
        assert_eq!(
            pace_warning(&samples(short), &weekly(short), T0_MS, 95.0),
            None
        );
        let long = T0_MS / 1000 + 2 * HOUR + 17 * HOUR;
        assert!(pace_warning(&samples(long), &weekly(long), T0_MS, 95.0).is_some());
    }

    #[test]
    fn a_short_burst_is_not_a_pace() {
        // The popover projects from 10 minutes of samples; the notification
        // wants 30.
        let resets_at = T0_MS / 1000 + 2 * 24 * HOUR;
        let samples = [sample(20, 40.0, resets_at), sample(10, 50.0, resets_at)];
        let current = reading(60.0, resets_at, WEEK_MINS);
        assert!(projected_run_out(&samples, 60.0, Some(resets_at), false, T0_MS).is_some());
        assert_eq!(pace_warning(&samples, &current, T0_MS, 95.0), None);
    }

    #[test]
    fn history_keeps_changes_only_and_forgets_past_the_lookback() {
        let resets_at = T0_MS / 1000 + 2 * 24 * HOUR;
        let mut tracker = PaceTracker::default();
        for minute in 0..10 {
            tracker.observe(
                &reading(10.0, resets_at, WEEK_MINS),
                T0_MS + minute * MIN_MS,
                ON,
            );
        }
        assert_eq!(tracker.samples.len(), 1, "a flat stretch is one sample");
        // Four hours later the old sample is past the lookback but still the
        // newest one, so it stays as the comparison point.
        let later = T0_MS + 4 * HOUR * 1000;
        tracker.observe(&reading(10.0, resets_at, WEEK_MINS), later, ON);
        assert_eq!(tracker.samples.len(), 1);
        tracker.observe(&reading(12.0, resets_at, WEEK_MINS), later + MIN_MS, ON);
        assert_eq!(tracker.samples.len(), 1, "older samples are pruned");
        assert_eq!(tracker.samples[0].used, 12.0);
        // A new window starts a fresh history.
        tracker.observe(
            &reading(1.0, resets_at + 7 * 24 * HOUR, WEEK_MINS),
            later + 2 * MIN_MS,
            ON,
        );
        assert_eq!(tracker.samples.len(), 1);
        assert_eq!(tracker.samples[0].used, 1.0);
        // And the history is bounded whatever a provider sends.
        let reset = resets_at + 7 * 24 * HOUR;
        for step in 0..1_000u64 {
            let used = (step % 100) as f64 / 10.0 + 1.0;
            tracker.observe(
                &reading(used, reset, WEEK_MINS),
                later + 3 * MIN_MS + step * 1_000,
                ON,
            );
        }
        assert!(tracker.samples.len() <= PACE_MAX_SAMPLES);
    }
}
