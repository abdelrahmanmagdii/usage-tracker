//! Quota windows and menu-bar title text.

use super::*;

/// One quota window a menu-bar meter can follow.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrayWindow {
    /// Stable identifier used by the menu-bar picker preference.
    pub id: String,
    /// Human label for the menu and tooltip ("5-hour", "Weekly", "Fable").
    pub label: String,
    pub used_percent: f64,
    pub resets_at: Option<u64>,
    pub duration_mins: Option<f64>,
}

pub fn window_duration_label(minutes: Option<f64>) -> String {
    match minutes {
        Some(m) if m == 300.0 => "5-hour".to_owned(),
        Some(m) if m == 1_440.0 => "Daily".to_owned(),
        Some(m) if m == 10_080.0 => "Weekly".to_owned(),
        Some(m) if m > 0.0 && m % 10_080.0 == 0.0 => format!("{}-week", (m / 10_080.0) as u64),
        Some(m) if m > 0.0 && m % 1_440.0 == 0.0 => format!("{}-day", (m / 1_440.0) as u64),
        Some(m) if m > 0.0 && m % 60.0 == 0.0 => format!("{}-hour", (m / 60.0) as u64),
        _ => "Limit".to_owned(),
    }
}

pub(super) fn collect_snapshot(limit_id: &str, snapshot: &Value, out: &mut Vec<TrayWindow>) {
    for kind in ["primary", "secondary"] {
        let Some(window) = snapshot.get(kind) else {
            continue;
        };
        let Some(used) = window.get("usedPercent").and_then(Value::as_f64) else {
            continue;
        };
        let duration = window.get("windowDurationMins").and_then(Value::as_f64);
        let label = window
            .get("windowLabel")
            .and_then(Value::as_str)
            .or_else(|| snapshot.get("windowLabel").and_then(Value::as_str))
            .map(str::to_owned)
            .unwrap_or_else(|| window_duration_label(duration));
        out.push(TrayWindow {
            id: format!("{limit_id}:{kind}"),
            label,
            used_percent: used.clamp(0.0, 100.0),
            resets_at: window
                .get("resetsAt")
                .and_then(Value::as_f64)
                .map(|value| value.max(0.0) as u64),
            duration_mins: duration,
        });
    }
}

/// Lists every window a provider reports, shortest window first so the picker
/// reads 5-hour → weekly regardless of map ordering.
pub fn collect_windows(payload: Option<&Value>) -> Vec<TrayWindow> {
    let Some(payload) = payload else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(by_id) = payload
        .get("rateLimitsByLimitId")
        .and_then(Value::as_object)
    {
        for (limit_id, snapshot) in by_id {
            collect_snapshot(limit_id, snapshot, &mut out);
        }
    }
    if out.is_empty() {
        if let Some(snapshot) = payload.get("rateLimits") {
            collect_snapshot("codex", snapshot, &mut out);
        }
    }
    out.sort_by(|left, right| {
        let a = left.duration_mins.unwrap_or(f64::MAX);
        let b = right.duration_mins.unwrap_or(f64::MAX);
        a.partial_cmp(&b)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.id.cmp(&right.id))
    });
    out
}

/// Resolves the preference to a window: an explicit choice when it still
/// exists, otherwise the most-used window.
pub fn select_window<'a>(windows: &'a [TrayWindow], preference: &str) -> Option<&'a TrayWindow> {
    if preference != crate::prefs::TRAY_WINDOW_AUTO {
        if let Some(chosen) = windows.iter().find(|window| window.id == preference) {
            return Some(chosen);
        }
    }
    windows.iter().max_by(|left, right| {
        left.used_percent
            .partial_cmp(&right.used_percent)
            .unwrap_or(std::cmp::Ordering::Equal)
    })
}

/// Titles show percent REMAINING, mirroring what the Codex and Claude apps
/// display, so the menu bar never disagrees with the app it mirrors.
pub fn tray_title(remaining_percent: Option<f64>, resets_at: Option<u64>, now_unix: u64) -> String {
    let Some(remaining) = remaining_percent else {
        return String::new();
    };
    let percent = remaining.clamp(0.0, 100.0).round() as u32;
    match resets_at {
        Some(target) if target > now_unix => {
            format!("{percent}% · {}", format_countdown(target - now_unix))
        }
        _ => format!("{percent}%"),
    }
}

/// Percent of a window still available.
pub fn remaining_percent(window: &TrayWindow) -> f64 {
    (100.0 - window.used_percent).clamp(0.0, 100.0)
}

/// One provider's contribution to the single combined menu-bar title.
#[derive(Debug, Clone, Copy, Default)]
pub struct MeterSegment {
    /// Whether this provider has usable data to show at all.
    pub present: bool,
    pub remaining: f64,
    pub resets_at: Option<u64>,
    pub incoming: bool,
    pub stale: bool,
}

pub(super) fn segment_percent(seg: &MeterSegment) -> String {
    let percent = seg.remaining.clamp(0.0, 100.0).round() as u32;
    with_incoming_prefix(
        with_stale_marker(format!("{percent}%"), seg.stale),
        seg.incoming,
    )
}

/// The combined (compact-layout) menu-bar title. A lone provider keeps its
/// countdown since there is room; several providers show percentages without
/// countdowns so they fit in one narrow item.
pub fn combined_title(segments: &[MeterSegment], now: u64) -> String {
    let present: Vec<&MeterSegment> = segments.iter().filter(|seg| seg.present).collect();
    match present.as_slice() {
        [] => String::new(),
        [only] => {
            let base = tray_title(Some(only.remaining), only.resets_at, now);
            with_incoming_prefix(with_stale_marker(base, only.stale), only.incoming)
        }
        many => many
            .iter()
            .map(|seg| segment_percent(seg))
            .collect::<Vec<_>>()
            .join(" · "),
    }
}

pub fn format_countdown(total_seconds: u64) -> String {
    let days = total_seconds / 86_400;
    if days > 0 {
        return format!("{days}d {}h", (total_seconds % 86_400) / 3_600);
    }
    let hours = total_seconds / 3_600;
    let minutes = (total_seconds % 3_600) / 60;
    let seconds = total_seconds % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}
