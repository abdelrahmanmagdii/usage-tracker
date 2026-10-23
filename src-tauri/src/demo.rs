//! Sample quotas for screen recordings.
//!
//! Off unless the process was started with `--demo` or `USAGEBAR_DEMO=1`
//! (also `true`, `yes`, or `on`). Nothing here runs for a normal launch.
//! While it is on, every provider snapshot below is fictional sample data:
//! refresh paths return it and do not touch the network, keychain, or
//! credential files. The numbers are deliberately uneven — one window sits
//! near its limit — so the real meters, tray title, and reset countdowns
//! have something to show.

use std::{
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    sync::OnceLock,
    thread,
    time::Duration,
};

use serde_json::{json, Map, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::codex::process::{CodexState, ConnectionState};
use crate::provider::ProviderState;

/// Stored on each sample snapshot. The connected UI does not render this;
/// the header's "Demo data" label does. It is here so a dumped state is
/// obviously not a live account.
pub const SAMPLE_NOTE: &str = "Sample data for UsageBar demo mode. Not a live account.";

const DEFAULT_PORT: u16 = 47621;

pub fn enabled() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| {
        let flag = std::env::args().any(|arg| arg == "--demo");
        let env = std::env::var("USAGEBAR_DEMO").ok();
        enabled_from(flag, env.as_deref())
    })
}

pub fn enabled_from(flag: bool, env: Option<&str>) -> bool {
    if flag {
        return true;
    }
    let Some(value) = env else {
        return false;
    };
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

pub fn log(message: &str) {
    eprintln!("usagebar: {message}");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path())
    {
        let _ = writeln!(file, "{message}");
    }
}

pub fn log_enablement() {
    if !enabled() {
        return;
    }
    let _ = std::fs::write(log_path(), "");
    if std::env::args().any(|arg| arg == "--demo") {
        log("demo mode enabled by --demo");
    }
    if enabled_from(false, std::env::var("USAGEBAR_DEMO").ok().as_deref()) {
        log("demo mode enabled by USAGEBAR_DEMO");
    }
    log("sample quotas only: no provider network calls and no credential reads");
}

/// Binds the localhost control port the recording scripts drive.
/// Commands are a single line: `SHOW`, `HIDE`, `SCROLL 190`, `SETTINGS`,
/// `CLOSE_SETTINGS`, `SCROLL_TOP`, `WIDGET EXPAND`, `WIDGET COLLAPSE`,
/// `GEOMETRY main`, `VISIBLE`, `PING`, `QUIT`.
pub fn spawn_control(app: AppHandle) {
    let port = std::env::var("USAGEBAR_DEMO_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_PORT);
    thread::Builder::new()
        .name("usagebar-demo".into())
        .spawn(move || {
            let listener = match TcpListener::bind(("127.0.0.1", port)) {
                Ok(listener) => listener,
                Err(error) => {
                    log(&format!("demo control port {port} failed: {error}"));
                    return;
                }
            };
            let bound = listener
                .local_addr()
                .map(|addr| addr.port())
                .unwrap_or(port);
            if let Err(error) = std::fs::write(port_path(), bound.to_string()) {
                log(&format!("demo port file failed: {error}"));
            }
            log(&format!("control listening on 127.0.0.1:{bound}"));
            for stream in listener.incoming() {
                let mut stream = match stream {
                    Ok(stream) => stream,
                    Err(error) => {
                        log(&format!("demo control accept failed: {error}"));
                        continue;
                    }
                };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let Some(line) = read_line(&mut stream) else {
                    continue;
                };
                let response = handle(&app, &line);
                let _ = write!(stream, "{response}\n");
                if line.trim().eq_ignore_ascii_case("QUIT") {
                    app.exit(0);
                    break;
                }
            }
        })
        .ok();
}

fn read_line(stream: &mut std::net::TcpStream) -> Option<String> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    for _ in 0..512 {
        match stream.read(&mut byte) {
            Ok(0) => break,
            Ok(_) if byte[0] == b'\n' => break,
            Ok(_) if byte[0] != b'\r' => buf.push(byte[0]),
            Ok(_) => {}
            Err(_) => {
                return if buf.is_empty() {
                    None
                } else {
                    Some(lossy(&buf))
                }
            }
        }
    }
    if buf.is_empty() {
        None
    } else {
        Some(lossy(&buf))
    }
}

fn lossy(buf: &[u8]) -> String {
    String::from_utf8_lossy(buf).into_owned()
}

fn handle(app: &AppHandle, line: &str) -> String {
    let mut parts = line.split_whitespace();
    let command = parts.next().unwrap_or("").to_ascii_uppercase();
    match command.as_str() {
        "PING" => "PONG demo".to_owned(),
        "SHOW" => show_main(app),
        "HIDE" => hide_main(app),
        "SETTINGS" => {
            emit_action(app, "settings", None);
            "OK".to_owned()
        }
        "CLOSE_SETTINGS" => {
            emit_action(app, "close-settings", None);
            "OK".to_owned()
        }
        "SCROLL_TOP" => {
            emit_action(app, "scroll-top", None);
            "OK".to_owned()
        }
        "SCROLL" => {
            let amount = parts.next().and_then(|value| value.parse().ok());
            emit_action(app, "scroll", amount.or(Some(200.0)));
            "OK".to_owned()
        }
        "WIDGET" => {
            let action = match parts.next().unwrap_or("").to_ascii_uppercase().as_str() {
                "EXPAND" => "widget-expand",
                "COLLAPSE" => "widget-collapse",
                _ => return "ERR widget".to_owned(),
            };
            emit_action(app, action, None);
            "OK".to_owned()
        }
        "VISIBLE" => visible(app),
        "GEOMETRY" => {
            let label = parts.next().unwrap_or("main").to_owned();
            geometry(app, label)
        }
        "QUIT" => "OK".to_owned(),
        _ => format!("ERR unknown {command}"),
    }
}

fn emit_action(app: &AppHandle, action: &str, amount: Option<f64>) {
    let _ = app.emit(
        "usagebar://demo",
        json!({ "action": action, "amount": amount }),
    );
}

fn show_main(app: &AppHandle) -> String {
    on_main(app, |app| {
        match crate::commands::show_dashboard(app.clone()) {
            Ok(()) => "OK".to_owned(),
            Err(error) => format!("ERR {error}"),
        }
    })
}

fn hide_main(app: &AppHandle) -> String {
    on_main(app, |app| {
        crate::commands::hide_window(app.clone());
        "OK".to_owned()
    })
}

fn visible(app: &AppHandle) -> String {
    on_main(app, |app| {
        let shown = app
            .get_webview_window("main")
            .and_then(|window| window.is_visible().ok())
            .unwrap_or(false);
        format!("VISIBLE {}", if shown { 1 } else { 0 })
    })
}

fn geometry(app: &AppHandle, label: String) -> String {
    on_main(app, move |app| {
        let Some(window) = app.get_webview_window(&label) else {
            return format!("ERR no {label}");
        };
        let pos = match window.outer_position() {
            Ok(pos) => pos,
            Err(error) => return format!("ERR {error}"),
        };
        let size = match window.outer_size() {
            Ok(size) => size,
            Err(error) => return format!("ERR {error}"),
        };
        let (screen_w, screen_h) = window
            .current_monitor()
            .ok()
            .flatten()
            .map(|monitor| (monitor.size().width, monitor.size().height))
            .unwrap_or((0, 0));
        format!(
            "GEOMETRY {} {} {} {} {} {}",
            pos.x, pos.y, size.width, size.height, screen_w, screen_h
        )
    })
}

fn on_main(app: &AppHandle, f: impl FnOnce(&AppHandle) -> String + Send + 'static) -> String {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let handle = app.clone();
    if let Err(error) = app.run_on_main_thread(move || {
        let _ = tx.send(f(&handle));
    }) {
        return format!("ERR {error}");
    }
    match rx.recv_timeout(Duration::from_secs(4)) {
        Ok(value) => value,
        Err(error) => format!("ERR {error}"),
    }
}

/// Puts the window where a recording can see it. A normal launch keeps the
/// centered window from `tauri.conf.json`.
///
/// macOS: under the menu-bar clock, which is where the status item opens it.
/// Windows: a right-hand column above the floating meter, so the meter and
/// the tray are in frame and the meter does not cover the dashboard.
pub fn place_popover(window: &tauri::WebviewWindow) {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    let Ok(Some(monitor)) = window.current_monitor() else {
        return;
    };
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    let scale = monitor.scale_factor();
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    if scale <= f64::EPSILON {
        return;
    }
    #[cfg(target_os = "macos")]
    {
        let screen_w = monitor.size().width as f64 / scale;
        let window_w = window
            .outer_size()
            .map(|size| size.width as f64 / scale)
            .unwrap_or(388.0);
        let x = (screen_w - window_w - 8.0).max(0.0);
        let _ = window.set_position(tauri::LogicalPosition::new(x, 36.0));
    }
    #[cfg(target_os = "windows")]
    {
        let screen_w = monitor.size().width as f64 / scale;
        let screen_h = monitor.size().height as f64 / scale;
        let window_w = 388.0;
        let window_h = 600.0;
        let margin = 16.0;
        // Taskbar plus the expanded Codex meter, so the two do not overlap
        // when the screen is tall enough to stack them.
        let reserved_below = 48.0 + 180.0;
        let x = (screen_w - window_w - margin).max(0.0);
        let stacked_y = screen_h - reserved_below - 12.0 - window_h;
        let y = if stacked_y >= margin { stacked_y } else { margin };
        let _ = window.set_position(tauri::LogicalPosition::new(x, y));
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = window;
    }
}

fn log_path() -> PathBuf {
    runtime_path("usagebar-demo.log")
}

fn port_path() -> PathBuf {
    runtime_path("usagebar-demo.port")
}

fn runtime_path(name: &str) -> PathBuf {
    #[cfg(windows)]
    {
        let base = std::env::var("TEMP").unwrap_or_else(|_| r"C:\Windows\Temp".to_owned());
        return PathBuf::from(base).join(name);
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("/tmp").join(name)
    }
}

pub fn initial_codex() -> CodexState {
    if enabled() {
        codex_fixture()
    } else {
        CodexState::default()
    }
}

pub fn initial_claude() -> crate::claude::ClaudeState {
    if enabled() {
        claude_fixture()
    } else {
        crate::claude::ClaudeState::default()
    }
}

pub fn initial_cursor() -> ProviderState {
    if enabled() {
        connected("Pro", cursor_limits())
    } else {
        ProviderState::default()
    }
}

pub fn initial_opencode() -> ProviderState {
    if enabled() {
        connected("Go", opencode_limits())
    } else {
        ProviderState::default()
    }
}

pub fn initial_devin() -> ProviderState {
    if enabled() {
        connected("Pro", devin_limits())
    } else {
        ProviderState::default()
    }
}

pub fn initial_antigravity() -> ProviderState {
    if enabled() {
        connected("Antigravity", antigravity_limits())
    } else {
        ProviderState::default()
    }
}

pub fn initial_gemini() -> ProviderState {
    if enabled() {
        connected("Free", gemini_limits())
    } else {
        ProviderState::default()
    }
}

fn connected(plan: &str, limits: Value) -> ProviderState {
    ProviderState {
        connection: ConnectionState::Connected,
        diagnostic: Some(SAMPLE_NOTE.to_owned()),
        account: Some(json!({ "type": "oauth", "planType": plan })),
        rate_limits: Some(limits),
        updated_at: Some(now_secs()),
    }
}

fn codex_fixture() -> CodexState {
    CodexState {
        connection: ConnectionState::Connected,
        diagnostic: Some(SAMPLE_NOTE.to_owned()),
        account: Some(json!({ "type": "oauth", "planType": "Plus" })),
        rate_limits: Some(codex_limits()),
        usage: Some(codex_usage()),
        updated_at: Some(now_secs()),
    }
}

fn claude_fixture() -> crate::claude::ClaudeState {
    crate::claude::ClaudeState {
        connection: ConnectionState::Connected,
        diagnostic: Some(SAMPLE_NOTE.to_owned()),
        account: Some(json!({ "type": "oauth", "planType": "Max" })),
        rate_limits: Some(claude_limits()),
        updated_at: Some(now_secs()),
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn window(used: f64, mins: f64, resets_in: f64) -> Value {
    json!({
        "usedPercent": used,
        "windowDurationMins": mins,
        "resetsAt": now_secs() as f64 + resets_in,
    })
}

fn snapshot(
    limit_id: &str,
    label: Option<&str>,
    name: Option<&str>,
    primary: Option<Value>,
    secondary: Option<Value>,
) -> Value {
    let mut map = Map::new();
    map.insert("limitId".into(), json!(limit_id));
    if let Some(label) = label {
        map.insert("windowLabel".into(), json!(label));
    }
    if let Some(name) = name {
        map.insert("limitName".into(), json!(name));
    }
    if let Some(primary) = primary {
        map.insert("primary".into(), primary);
    }
    if let Some(secondary) = secondary {
        map.insert("secondary".into(), secondary);
    }
    Value::Object(map)
}

fn by_id(entries: Vec<(&str, Value)>) -> Value {
    let mut map = Map::new();
    for (id, value) in entries {
        map.insert(id.to_owned(), value);
    }
    json!({ "rateLimitsByLimitId": Value::Object(map) })
}

/// Codex: a comfortable 5-hour window and a half-spent week, plus one reset credit.
fn codex_limits() -> Value {
    let mut limits = by_id(vec![(
        "codex",
        snapshot(
            "codex",
            None,
            Some("Codex subscription"),
            Some(window(18.0, 300.0, 2.2 * 3_600.0)),
            Some(window(47.0, 10_080.0, 3.25 * 86_400.0)),
        ),
    )]);
    limits.as_object_mut().expect("limits object").insert(
        "rateLimitResetCredits".into(),
        json!({ "availableCount": 1 }),
    );
    limits
}

/// Claude's Fable week is the near-limit window (91% used, about 47 minutes left).
fn claude_limits() -> Value {
    by_id(vec![
        (
            "session",
            snapshot(
                "session",
                None,
                None,
                Some(window(6.0, 300.0, 4.6 * 3_600.0)),
                None,
            ),
        ),
        (
            "weekly-all",
            snapshot(
                "weekly-all",
                None,
                Some("All models"),
                None,
                Some(window(38.0, 10_080.0, 1.2 * 86_400.0)),
            ),
        ),
        (
            "weekly-scoped-fable",
            snapshot(
                "weekly-scoped-fable",
                Some("Fable"),
                Some("Weekly limit"),
                None,
                Some(window(91.0, 10_080.0, 47.0 * 60.0)),
            ),
        ),
    ])
}

fn cursor_limits() -> Value {
    by_id(vec![
        (
            "plan",
            snapshot(
                "plan",
                Some("Monthly"),
                Some("Cursor plan"),
                Some(window(74.0, 43_200.0, 12.0 * 86_400.0)),
                None,
            ),
        ),
        (
            "auto",
            snapshot(
                "auto",
                Some("Auto"),
                Some("Auto + Composer"),
                None,
                Some(window(15.0, 43_200.0, 12.0 * 86_400.0)),
            ),
        ),
    ])
}

fn opencode_limits() -> Value {
    by_id(vec![
        (
            "rolling",
            snapshot(
                "rolling",
                None,
                None,
                Some(window(22.0, 300.0, 3.4 * 3_600.0)),
                None,
            ),
        ),
        (
            "weekly",
            snapshot(
                "weekly",
                None,
                None,
                None,
                Some(window(31.0, 10_080.0, 4.0 * 86_400.0)),
            ),
        ),
        (
            "monthly",
            snapshot(
                "monthly",
                Some("Monthly"),
                None,
                None,
                Some(window(8.0, 43_200.0, 20.0 * 86_400.0)),
            ),
        ),
    ])
}

fn devin_limits() -> Value {
    by_id(vec![
        (
            "daily",
            snapshot(
                "daily",
                Some("Daily"),
                Some("Devin plan"),
                Some(window(58.0, 1_440.0, 11.0 * 3_600.0)),
                None,
            ),
        ),
        (
            "weekly",
            snapshot(
                "weekly",
                Some("Weekly"),
                Some("Devin plan"),
                None,
                Some(window(44.0, 10_080.0, 4.0 * 86_400.0)),
            ),
        ),
    ])
}

fn antigravity_limits() -> Value {
    by_id(vec![
        (
            "gemini-5h",
            snapshot(
                "gemini-5h",
                Some("Gemini 5-hour"),
                Some("Gemini"),
                Some(window(12.0, 300.0, 3.1 * 3_600.0)),
                None,
            ),
        ),
        (
            "gemini-weekly",
            snapshot(
                "gemini-weekly",
                Some("Gemini weekly"),
                Some("Gemini"),
                None,
                Some(window(46.0, 10_080.0, 3.0 * 86_400.0)),
            ),
        ),
        (
            "3p-5h",
            snapshot(
                "3p-5h",
                Some("Claude and GPT 5-hour"),
                Some("Claude and GPT"),
                Some(window(9.0, 300.0, 4.2 * 3_600.0)),
                None,
            ),
        ),
        (
            "3p-weekly",
            snapshot(
                "3p-weekly",
                Some("Claude and GPT weekly"),
                Some("Claude and GPT"),
                None,
                Some(window(33.0, 10_080.0, 5.0 * 86_400.0)),
            ),
        ),
        (
            "argon-weekly",
            snapshot(
                "argon-weekly",
                Some("Gemini 4 Argon weekly"),
                Some("Gemini 4 Argon"),
                None,
                Some(window(3.0, 10_080.0, 6.0 * 86_400.0)),
            ),
        ),
    ])
}

fn gemini_limits() -> Value {
    by_id(vec![
        (
            "gemini-pro",
            snapshot(
                "gemini-pro",
                Some("Pro models"),
                Some("Gemini Pro"),
                Some(window(41.0, 1_440.0, 11.0 * 3_600.0)),
                None,
            ),
        ),
        (
            "gemini-flash",
            snapshot(
                "gemini-flash",
                Some("Flash models"),
                Some("Gemini Flash"),
                None,
                Some(window(17.0, 1_440.0, 11.0 * 3_600.0)),
            ),
        ),
    ])
}

fn codex_usage() -> Value {
    let buckets: Vec<Value> = [41_000, 72_000, 56_000, 94_000, 132_000, 81_000, 118_000]
        .into_iter()
        .enumerate()
        .map(|(index, tokens)| {
            json!({
                "startDate": iso_days_ago(6 - index as i64),
                "tokens": tokens,
            })
        })
        .collect();
    json!({
        "summary": {
            "lifetimeTokens": 2_470_000,
            "peakDailyTokens": 184_000,
            "currentStreakDays": 12,
            "longestStreakDays": 21,
        },
        "dailyUsageBuckets": buckets,
    })
}

fn iso_days_ago(days: i64) -> String {
    let secs = now_secs() as i64 - days * 86_400;
    iso_from_unix(secs.max(0))
}

fn iso_from_unix(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let (year, month, day) = civil_from_unix_days(days);
    format!("{year:04}-{month:02}-{day:02}T00:00:00Z")
}

/// Howard Hinnant's `civil_from_days`, shifted so day 0 is 1970-01-01.
fn civil_from_unix_days(unix_days: i64) -> (i64, u32, u32) {
    let z = unix_days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    (year, month as u32, day as u32)
}

#[cfg(test)]
fn used_percents(limits: &Value) -> Vec<f64> {
    let Some(by_id) = limits.get("rateLimitsByLimitId").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for snapshot in by_id.values() {
        for kind in ["primary", "secondary"] {
            if let Some(used) = snapshot
                .pointer(&format!("/{kind}/usedPercent"))
                .and_then(Value::as_f64)
            {
                out.push(used);
            }
        }
    }
    out
}

#[cfg(test)]
fn resets_at(limits: &Value) -> Vec<f64> {
    let Some(by_id) = limits.get("rateLimitsByLimitId").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for snapshot in by_id.values() {
        for kind in ["primary", "secondary"] {
            if let Some(at) = snapshot
                .pointer(&format!("/{kind}/resetsAt"))
                .and_then(Value::as_f64)
            {
                out.push(at);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_mode_is_opt_in() {
        assert!(!enabled_from(false, None));
        assert!(!enabled_from(false, Some("")));
        assert!(!enabled_from(false, Some("0")));
        assert!(!enabled_from(false, Some("false")));
        assert!(enabled_from(false, Some("1")));
        assert!(enabled_from(false, Some(" true ")));
        assert!(enabled_from(false, Some("YES")));
        assert!(enabled_from(false, Some("on")));
        assert!(enabled_from(true, None));
        assert!(enabled_from(true, Some("0")));
    }

    #[test]
    fn sample_quotas_cover_every_provider_and_one_near_limit() {
        let sets = [
            codex_limits(),
            claude_limits(),
            cursor_limits(),
            opencode_limits(),
            devin_limits(),
            antigravity_limits(),
            gemini_limits(),
        ];
        assert_eq!(sets.len(), 7);
        let mut all = Vec::new();
        let now = now_secs() as f64;
        for limits in &sets {
            let used = used_percents(limits);
            assert!(!used.is_empty(), "provider missing windows");
            for value in &used {
                assert!((0.0..=100.0).contains(value));
            }
            for reset in resets_at(limits) {
                assert!(reset > now, "reset should still be in the future");
            }
            all.extend(used);
        }
        let max = all.iter().copied().fold(0.0_f64, f64::max);
        let min = all.iter().copied().fold(100.0_f64, f64::min);
        assert!(max >= 90.0, "expected a near-limit window, max was {max}");
        assert!(max - min > 50.0, "sample windows should not all look alike");
        assert_eq!(
            claude_limits()
                .pointer("/rateLimitsByLimitId/weekly-scoped-fable/secondary/usedPercent"),
            Some(&json!(91.0))
        );
    }

    #[test]
    fn fixtures_are_labeled_sample_data_when_built_directly() {
        let codex = codex_fixture();
        assert_eq!(codex.connection, ConnectionState::Connected);
        assert_eq!(codex.diagnostic.as_deref(), Some(SAMPLE_NOTE));
        assert!(codex.usage.is_some());
        assert_eq!(
            codex
                .rate_limits
                .as_ref()
                .and_then(|limits| limits.pointer("/rateLimitResetCredits/availableCount"))
                .and_then(Value::as_i64),
            Some(1)
        );
        assert_eq!(claude_fixture().diagnostic.as_deref(), Some(SAMPLE_NOTE));
    }

    #[test]
    fn unix_days_convert_to_calendar_dates() {
        assert_eq!(iso_from_unix(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_from_unix(86_400), "1970-01-02T00:00:00Z");
        // 2024-01-01 00:00:00 UTC.
        assert_eq!(iso_from_unix(1_704_067_200), "2024-01-01T00:00:00Z");
    }
}
