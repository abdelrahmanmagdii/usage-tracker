//! A local crash log: what UsageBar was doing when it quit unexpectedly.
//!
//! Release builds abort on panic (`panic = "abort"`), so without this a crash
//! leaves nothing behind but "it disappeared". A panic hook appends a short,
//! scrubbed report to `crash-report.log` in the app's log directory and drops a
//! marker; the next launch sees the marker, posts one local notification, and
//! the tray menu offers "Show Crash Report…" for as long as the file exists.
//!
//! Nothing here touches the network. Reports stay on this machine until the
//! user chooses to share one.
//!
//! **Where.** The directory is Tauri's `app_log_dir` for the bundle identifier:
//! `~/Library/Logs/com.abdelrahmanamer.usagebar` on macOS,
//! `%LOCALAPPDATA%\com.abdelrahmanamer.usagebar\logs` on Windows and
//! `$XDG_DATA_HOME/com.abdelrahmanamer.usagebar/logs` (normally
//! `~/.local/share/…`) on Linux. The hook is installed before the Tauri
//! builder exists, so there is no `AppHandle` to ask yet: [`default_log_dir`]
//! computes the same path with the same `dirs` lookups Tauri itself uses.
//! Once setup runs, [`on_launch`] asks Tauri for the real `app_log_dir` and
//! adopts it if it ever differs (an `appDirectoriesOverride` added to the
//! config later), so the two phases cannot drift apart silently.
//!
//! **Privacy.** Panic messages are formatted values and may carry tokens. Each
//! report runs through [`scrub`] (bearer tokens, `sk-…` keys, JWTs,
//! `token=`/`key=`-style pairs, long base64/hex runs) and has the user's home
//! directory replaced with `~`.

use std::backtrace::Backtrace;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

/// Must match `identifier` in `tauri.conf.json` (a test holds them together).
pub const IDENTIFIER: &str = "com.abdelrahmanamer.usagebar";
pub const REPORT_FILE: &str = "crash-report.log";
/// Present only between a crash and the next launch that noticed it.
const PENDING_MARKER: &str = "crash-report.pending";
const REPORT_HEADER: &str = "=== UsageBar crash report ===";

/// Reports kept in the file; older ones roll off the front.
const KEEP_REPORTS: usize = 5;
const MAX_MESSAGE_BYTES: usize = 2 * 1024;
const MAX_BACKTRACE_BYTES: usize = 12 * 1024;
/// Raw text gathered before scrubbing. Scrubbing happens before the final
/// truncation so a secret is never cut in half below the detectors' minimum
/// lengths; this outer cap only bounds the work.
const MAX_RAW_BYTES: usize = 64 * 1024;
/// An existing file larger than this was not written by us; start over.
const MAX_EXISTING_BYTES: u64 = 256 * 1024;

const REDACTED: &str = "[redacted]";

/// Where the hook writes, and the home directory to fold into `~`.
#[derive(Debug, Clone)]
struct Target {
    dir: PathBuf,
    home: Option<String>,
}

/// The production hook's target. The mutex also serializes two threads
/// panicking at once so their reports do not interleave.
static TARGET: Mutex<Option<Target>> = Mutex::new(None);

/// Whether a report file existed at launch. A report is only ever written by a
/// dying process, so this cannot change while the app runs (short of the user
/// deleting the file, which [`reveal_report`] notices).
static REPORT_AVAILABLE: AtomicBool = AtomicBool::new(false);

/// Tauri's `app_log_dir` for [`IDENTIFIER`], computed without an `AppHandle`.
/// Mirrors `tauri::path::PathResolver::app_log_dir` (tauri 2.x, desktop).
pub fn default_log_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        dirs::home_dir().map(|home| home.join("Library/Logs").join(IDENTIFIER))
    }
    #[cfg(not(target_os = "macos"))]
    {
        dirs::data_local_dir().map(|dir| dir.join(IDENTIFIER).join("logs"))
    }
}

/// The home directory as it would appear in paths, or `None` when it is too
/// short to replace safely (folding `/` into `~` would mangle every path).
fn home_for_scrubbing() -> Option<String> {
    let home = dirs::home_dir()?;
    let home = home.to_str()?.trim_end_matches(['/', '\\']).to_owned();
    (home.len() > 3).then_some(home)
}

/// Installs the crash hook. Call first thing in `run()`, before the Tauri
/// builder, so a panic during plugin init or setup is recorded too.
pub fn install() {
    let target = default_log_dir().map(|dir| Target {
        dir,
        home: home_for_scrubbing(),
    });
    *TARGET.lock().unwrap_or_else(PoisonError::into_inner) = target;
    install_with(&TARGET);
}

/// Chains a report-writing hook in front of whatever hook was set before (the
/// default one prints to stderr, which keeps `tauri dev` output unchanged).
fn install_with(target: &'static Mutex<Option<Target>>) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        record_panic(target, info);
        previous(info);
    }));
}

/// The hook body. Never panics: every fallible step is ignored rather than
/// unwrapped, formatting goes into `String`s (infallible), and the backtrace
/// is written through a capped sink so a deep stack cannot balloon memory.
fn record_panic(target: &Mutex<Option<Target>>, info: &std::panic::PanicHookInfo<'_>) {
    let message = info
        .payload_as_str()
        .unwrap_or("(non-string panic payload)");
    let location = info
        .location()
        .map(|loc| format!("{}:{}:{}", loc.file(), loc.line(), loc.column()))
        .unwrap_or_else(|| "(unknown)".to_owned());
    let thread = std::thread::current();
    let thread_name = thread.name().unwrap_or("(unnamed)").to_owned();

    let mut backtrace = CappedString::new(MAX_RAW_BYTES);
    let _ = write!(backtrace, "{}", Backtrace::force_capture());

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let guard = target.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(target) = guard.as_ref() else {
        return;
    };
    let report = format_report(&ReportFields {
        unix_secs: now,
        version: env!("CARGO_PKG_VERSION"),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        thread: &thread_name,
        location: &location,
        message,
        backtrace: &backtrace.0,
        home: target.home.as_deref(),
    });
    let _ = write_report(&target.dir, &report);
}

/// A `fmt::Write` sink that silently stops accepting text past `limit` bytes.
struct CappedString(String, usize);

impl CappedString {
    fn new(limit: usize) -> Self {
        Self(String::new(), limit)
    }
}

impl std::fmt::Write for CappedString {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let room = self.1.saturating_sub(self.0.len());
        if room > 0 {
            self.0.push_str(truncate_at_char(s, room));
        }
        Ok(())
    }
}

struct ReportFields<'a> {
    unix_secs: u64,
    version: &'a str,
    os: &'a str,
    arch: &'a str,
    thread: &'a str,
    location: &'a str,
    message: &'a str,
    backtrace: &'a str,
    home: Option<&'a str>,
}

/// Renders one report. Every free-text field is home-folded, scrubbed and only
/// then truncated.
fn format_report(fields: &ReportFields<'_>) -> String {
    let clean = |text: &str, limit: usize| {
        let text = truncate_at_char(text, MAX_RAW_BYTES);
        let text = match fields.home {
            Some(home) if !home.is_empty() => text.replace(home, "~"),
            _ => text.to_owned(),
        };
        truncate_with_marker(&scrub(&text), limit)
    };
    let message = clean(fields.message, MAX_MESSAGE_BYTES);
    let location = clean(fields.location, 512);
    let thread = clean(fields.thread, 128);
    let backtrace = clean(fields.backtrace.trim_end(), MAX_BACKTRACE_BYTES);

    let mut out = String::with_capacity(message.len() + backtrace.len() + 256);
    let _ = writeln!(out, "{REPORT_HEADER}");
    let _ = writeln!(out, "time: {}", format_utc(fields.unix_secs));
    let _ = writeln!(out, "version: {}", fields.version);
    let _ = writeln!(out, "os: {} ({})", fields.os, fields.arch);
    let _ = writeln!(out, "thread: {thread}");
    let _ = writeln!(out, "location: {location}");
    // Indent continuation lines so a multi-line message cannot fake a header.
    let _ = writeln!(out, "message: {}", message.replace('\n', "\n  "));
    if backtrace.is_empty() {
        let _ = writeln!(out, "backtrace: (unavailable)");
    } else {
        let _ = writeln!(out, "backtrace:");
        for line in backtrace.lines() {
            let _ = writeln!(out, "  {line}");
        }
    }
    out
}

/// Appends `report`, keeping only the newest [`KEEP_REPORTS`], then sets the
/// marker the next launch looks for. Written to a temp file and renamed so a
/// second crash mid-write cannot leave a half-file behind.
fn write_report(dir: &Path, report: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(REPORT_FILE);
    let existing = match std::fs::metadata(&path) {
        Ok(meta) if meta.len() <= MAX_EXISTING_BYTES => {
            std::fs::read_to_string(&path).unwrap_or_default()
        }
        _ => String::new(),
    };
    let combined = keep_recent(&existing, report, KEEP_REPORTS);
    let tmp = dir.join(format!("{REPORT_FILE}.tmp"));
    std::fs::write(&tmp, combined)?;
    std::fs::rename(&tmp, &path)?;
    std::fs::write(dir.join(PENDING_MARKER), b"")
}

/// The existing file's last `keep - 1` reports followed by `report`.
fn keep_recent(existing: &str, report: &str, keep: usize) -> String {
    let mut starts: Vec<usize> = existing
        .match_indices(REPORT_HEADER)
        .map(|(at, _)| at)
        .filter(|&at| at == 0 || existing[..at].ends_with('\n'))
        .collect();
    let drop = (starts.len() + 1).saturating_sub(keep);
    starts.drain(..drop.min(starts.len()));
    let mut out = match (keep, starts.first()) {
        (0 | 1, _) | (_, None) => String::new(),
        (_, Some(&from)) => existing[from..].to_owned(),
    };
    if !out.is_empty() && !out.ends_with("\n\n") {
        out.push_str(if out.ends_with('\n') { "\n" } else { "\n\n" });
    }
    out.push_str(report);
    out
}

/// Called from setup: adopts Tauri's own log directory, records whether a
/// report exists for the tray menu, and posts the one-time notice for a crash
/// since the last launch.
pub fn on_launch(app: &tauri::AppHandle) {
    use tauri::Manager;

    let tauri_dir = app.path().app_log_dir().ok();
    let dir = {
        let mut guard = TARGET.lock().unwrap_or_else(PoisonError::into_inner);
        match (guard.as_mut(), tauri_dir) {
            (Some(target), Some(tauri_dir)) => {
                // Identical unless the config gains an app directory
                // override; Tauri's answer wins from here on.
                target.dir = tauri_dir;
                Some(target.dir.clone())
            }
            (None, Some(tauri_dir)) => {
                *guard = Some(Target {
                    dir: tauri_dir.clone(),
                    home: home_for_scrubbing(),
                });
                Some(tauri_dir)
            }
            (Some(target), None) => Some(target.dir.clone()),
            (None, None) => None,
        }
    };
    let Some(dir) = dir else {
        return;
    };
    REPORT_AVAILABLE.store(dir.join(REPORT_FILE).is_file(), Ordering::Relaxed);

    // Demo launches are for screenshots and recordings; leave the marker so
    // the next real launch still says something.
    if crate::demo::enabled() || !take_pending(&dir) {
        return;
    }
    use tauri_plugin_notification::NotificationExt;
    let _ = app
        .notification()
        .builder()
        .title("UsageBar")
        .body(crash_notice_body())
        .show();
}

fn crash_notice_body() -> &'static str {
    if cfg!(target_os = "macos") {
        "UsageBar quit unexpectedly last time. A crash report was saved — show it from the menu bar menu."
    } else {
        "UsageBar quit unexpectedly last time. A crash report was saved — show it from the tray icon menu."
    }
}

/// True once per crash: removes the marker the hook left. If the marker cannot
/// be removed it is still reported (better a repeat notice than none).
fn take_pending(dir: &Path) -> bool {
    let marker = dir.join(PENDING_MARKER);
    if !marker.exists() {
        return false;
    }
    let _ = std::fs::remove_file(&marker);
    true
}

/// Whether the tray menu should offer "Show Crash Report…".
pub fn report_available() -> bool {
    REPORT_AVAILABLE.load(Ordering::Relaxed)
}

fn report_path() -> Option<PathBuf> {
    TARGET
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .map(|target| target.dir.join(REPORT_FILE))
}

/// Shows the report file in Finder / Explorer (the containing folder on
/// Linux). Spawns the platform's file browser directly; no shell is involved,
/// and the only argument is a path this module built.
pub fn reveal_report() {
    let Some(path) = report_path().filter(|path| path.is_file()) else {
        // Deleted while the app was running: the tray's next tick sees the
        // flag in its menu signature and drops the item.
        REPORT_AVAILABLE.store(false, Ordering::Relaxed);
        return;
    };
    if let Err(_error) = reveal_in_file_browser(&path) {
        #[cfg(debug_assertions)]
        eprintln!("usagebar: could not reveal crash report: {_error}");
    }
}

fn reveal_in_file_browser(path: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("/usr/bin/open")
            .arg("-R")
            .arg(path)
            .spawn()
            .map(|_| ())
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        // Explorer parses `/select,` itself and wants the path quoted after
        // the comma, which std's own argument quoting cannot produce. A
        // Windows path cannot contain `"`, so this quoting is unambiguous.
        let explorer = std::env::var_os("SystemRoot")
            .map(|root| PathBuf::from(root).join("explorer.exe"))
            .unwrap_or_else(|| PathBuf::from("explorer.exe"));
        let mut arg = std::ffi::OsString::from("/select,\"");
        arg.push(path.as_os_str());
        arg.push("\"");
        std::process::Command::new(explorer)
            .raw_arg(arg)
            .spawn()
            .map(|_| ())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        // No portable "select this file"; open the folder that holds it.
        let dir = path.parent().unwrap_or(path);
        std::process::Command::new("xdg-open")
            .arg(dir)
            .spawn()
            .map(|_| ())
    }
}

/// Debug builds only: `USAGEBAR_DEBUG_PANIC=1` panics right after the hook is
/// installed, to exercise it end to end. Compiled out of release builds, which
/// have no way to crash on purpose.
#[cfg(debug_assertions)]
pub fn debug_panic_if_requested() {
    if std::env::var_os("USAGEBAR_DEBUG_PANIC").is_some_and(|value| value == "1") {
        // Built at runtime so no secret-looking literal sits in the source;
        // both values must come out of the report as [redacted].
        let token = "a1".repeat(20);
        let key = "b2c3".repeat(6);
        panic!(
            "deliberate debug panic (USAGEBAR_DEBUG_PANIC=1): \
             Authorization: Bearer {token} url=https://example.invalid/usage?api_key={key}&x=1"
        );
    }
}

// --- Formatting helpers -------------------------------------------------------

/// The longest prefix of `text` that fits in `max` bytes on a char boundary.
fn truncate_at_char(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn truncate_with_marker(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let kept = truncate_at_char(text, max);
    format!("{kept}… [truncated {} bytes]", text.len() - kept.len())
}

/// `YYYY-MM-DDTHH:MM:SSZ` without a date library (and so without anything in
/// the hook that could fail on an odd clock).
fn format_utc(unix_secs: u64) -> String {
    let days = unix_secs / 86_400;
    let rem = unix_secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Days since 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's
/// `civil_from_days`, restricted to non-negative days).
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

// --- Scrubbing ----------------------------------------------------------------

/// Removes anything that looks like a credential. Deliberately greedy: a crash
/// report that over-redacts is still useful; one that leaks a token is not.
pub(crate) fn scrub(text: &str) -> String {
    let text = scrub_jwts(text);
    let text = scrub_prefixed_keys(&text);
    let text = scrub_auth_schemes(&text);
    let text = scrub_named_values(&text);
    scrub_long_runs(&text)
}

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_')
}

/// Splits `text` into maximal runs where `in_run` holds and passes each run
/// through `map` (`None` keeps it as is).
fn map_runs(
    text: &str,
    in_run: impl Fn(char) -> bool,
    map: impl Fn(&str) -> Option<String>,
) -> String {
    let mut out = String::with_capacity(text.len());
    let mut run_start: Option<usize> = None;
    let flush = |out: &mut String, run: &str| match map(run) {
        Some(replacement) => out.push_str(&replacement),
        None => out.push_str(run),
    };
    for (at, c) in text.char_indices() {
        if in_run(c) {
            run_start.get_or_insert(at);
        } else {
            if let Some(start) = run_start.take() {
                flush(&mut out, &text[start..at]);
            }
            out.push(c);
        }
    }
    if let Some(start) = run_start {
        flush(&mut out, &text[start..]);
    }
    out
}

/// `eyJ…`-headed dotted tokens (JWTs and JWS/JWE relatives).
fn scrub_jwts(text: &str) -> String {
    map_runs(
        text,
        |c| is_token_char(c) || c == '.' || c == '=',
        |run| {
            // A run may carry a leading word joined by dots ("x.eyJ…"); find
            // the token itself.
            let at = run.find("eyJ")?;
            let token = &run[at..];
            let segments = token.split('.').filter(|s| !s.is_empty()).count();
            (segments >= 2 && token.len() >= 20).then(|| format!("{}{REDACTED}", &run[..at]))
        },
    )
}

/// Vendor key prefixes: OpenAI/Anthropic `sk-`, GitHub, Slack, Google.
fn scrub_prefixed_keys(text: &str) -> String {
    const PREFIXES: [&str; 9] = [
        "sk-",
        "sk_",
        "ghp_",
        "gho_",
        "ghu_",
        "ghs_",
        "github_pat_",
        "xoxb-",
        "xoxp-",
    ];
    map_runs(text, is_token_char, |run| {
        let lower = run.to_ascii_lowercase();
        let prefixed = PREFIXES
            .iter()
            .any(|prefix| lower.starts_with(prefix) && run.len() >= prefix.len() + 8);
        let google = run.starts_with("AIza") && run.len() >= 30;
        (prefixed || google).then(|| REDACTED.to_owned())
    })
}

/// `Bearer <token>`, `Basic <token>`, `Token <token>` (any case).
fn scrub_auth_schemes(text: &str) -> String {
    const SCHEMES: [&str; 3] = ["bearer", "basic", "token"];
    let lower = text.to_ascii_lowercase();
    let lower = lower.as_bytes();
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut i = 0;
    while i < bytes.len() {
        let at_word_start = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
        let scheme = SCHEMES
            .iter()
            .find(|scheme| at_word_start && lower[i..].starts_with(scheme.as_bytes()));
        let Some(scheme) = scheme else {
            i += 1;
            continue;
        };
        let mut start = i + scheme.len();
        while start < bytes.len() && matches!(bytes[start], b' ' | b'\t') {
            start += 1;
        }
        let mut end = start;
        while end < bytes.len() && !is_value_delimiter(bytes[end]) {
            end += 1;
        }
        let value = &text[start..end];
        // "token" is also an ordinary word ("the token expired"); only a
        // value that looks generated counts after it.
        let generated = value.len() >= 20 || value.bytes().any(|b| !b.is_ascii_alphabetic());
        let secret = start > i + scheme.len()
            && value.len() >= 8
            && (*scheme != "token" || generated)
            && !text[start..].starts_with(REDACTED);
        if secret {
            out.push_str(&text[copied..start]);
            out.push_str(REDACTED);
            copied = end;
        }
        i = end.max(i + 1);
    }
    out.push_str(&text[copied..]);
    out
}

/// Where an unquoted credential value ends.
fn is_value_delimiter(b: u8) -> bool {
    b.is_ascii_whitespace()
        || matches!(
            b,
            b'&' | b',' | b';' | b')' | b'}' | b']' | b'"' | b'\'' | b'<' | b'>'
        )
}

/// Field names whose values are credentials, matched case-insensitively as a
/// substring of the name (so `api_key`, `access_token`, `X-Api-Key`,
/// `client_secret`, `Authorization` all count).
const SECRET_NAME_PARTS: [&str; 12] = [
    "token",
    "secret",
    "password",
    "passwd",
    "key",
    "auth",
    "session",
    "cookie",
    "credential",
    "signature",
    "sig",
    "code",
];

fn is_secret_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    SECRET_NAME_PARTS.iter().any(|part| {
        if *part == "sig" || *part == "code" {
            // Short parts only as the whole name (`?sig=`, `?code=`), so
            // `design` or `encode` are left alone.
            lower == *part
        } else {
            lower.contains(part)
        }
    })
}

/// `name=value`, `name: value`, `"name": "value"` where the name looks like a
/// credential: the value becomes `[redacted]`.
fn scrub_named_values(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c != b'=' && c != b':' {
            i += 1;
            continue;
        }
        // `::` paths and `==` padding are not separators.
        if bytes.get(i + 1) == Some(&c) || (i > 0 && bytes[i - 1] == c) {
            i += 1;
            continue;
        }
        // The name ends right before the separator, optionally behind spaces
        // and a closing quote (`password = x`, `"token": "x"`).
        let mut name_end = i;
        while name_end > 0 && matches!(bytes[name_end - 1], b' ' | b'\t') {
            name_end -= 1;
        }
        if name_end > 0 && matches!(bytes[name_end - 1], b'"' | b'\'') {
            name_end -= 1;
        }
        let mut name_start = name_end;
        while name_start > 0
            && (bytes[name_start - 1].is_ascii_alphanumeric()
                || matches!(bytes[name_start - 1], b'_' | b'-'))
        {
            name_start -= 1;
        }
        if name_start == name_end || !is_secret_name(&text[name_start..name_end]) {
            i += 1;
            continue;
        }
        // The value: skip spaces and an opening quote, then read to the
        // closing quote (quoted) or the next delimiter (bare).
        let mut start = i + 1;
        while start < bytes.len() && matches!(bytes[start], b' ' | b'\t') {
            start += 1;
        }
        let quote = bytes
            .get(start)
            .copied()
            .filter(|q| matches!(q, b'"' | b'\''));
        if quote.is_some() {
            start += 1;
        }
        let mut end = start;
        while end < bytes.len() {
            let b = bytes[end];
            let stop = match quote {
                Some(q) => b == q || b == b'\n',
                None => is_value_delimiter(b),
            };
            if stop {
                break;
            }
            end += 1;
        }
        if end > start && !text[start..].starts_with(REDACTED) {
            out.push_str(&text[copied..start]);
            out.push_str(REDACTED);
            copied = end;
        }
        i = end.max(i + 1);
    }
    out.push_str(&text[copied..]);
    out
}

/// Long unbroken base64/hex-looking runs: 32+ characters mixing letters and
/// digits. Slash-separated runs are judged per component so file paths in a
/// backtrace survive; real base64 secrets with slashes are usually caught by
/// the context rules above.
fn scrub_long_runs(text: &str) -> String {
    let looks_secret = |part: &str| {
        part.len() >= 32
            && part.bytes().any(|b| b.is_ascii_digit())
            && part.bytes().any(|b| b.is_ascii_alphabetic())
            && part != REDACTED
    };
    map_runs(
        text,
        |c| is_token_char(c) || matches!(c, '+' | '/' | '='),
        |run| {
            if !run.split('/').any(looks_secret) {
                return None;
            }
            let parts: Vec<String> = run
                .split('/')
                .map(|part| {
                    if looks_secret(part) {
                        REDACTED.to_owned()
                    } else {
                        part.to_owned()
                    }
                })
                .collect();
            Some(parts.join("/"))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "usagebar-crashlog-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn fields<'a>(message: &'a str, backtrace: &'a str) -> ReportFields<'a> {
        ReportFields {
            unix_secs: 1_762_345_678,
            version: "0.2.4",
            os: "macos",
            arch: "aarch64",
            thread: "main",
            location: "src/tray/mod.rs:42:9",
            message,
            backtrace,
            home: Some("/Users/alice"),
        }
    }

    #[test]
    fn identifier_matches_the_tauri_config() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json");
        assert_eq!(config["identifier"], IDENTIFIER);
    }

    #[test]
    fn default_log_dir_ends_in_the_identifier_layout() {
        let dir = default_log_dir().expect("a home directory in tests");
        if cfg!(target_os = "macos") {
            assert!(dir.ends_with(Path::new("Library/Logs").join(IDENTIFIER)));
        } else {
            assert!(dir.ends_with(Path::new(IDENTIFIER).join("logs")));
        }
    }

    #[test]
    fn utc_timestamps_are_iso_8601() {
        assert_eq!(format_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(format_utc(1_762_345_678), "2025-11-05T12:27:58Z");
        assert_eq!(format_utc(4_107_542_399), "2100-02-28T23:59:59Z");
        assert_eq!(format_utc(4_107_542_400), "2100-03-01T00:00:00Z");
    }

    #[test]
    fn report_has_every_field_and_folds_home() {
        let report = format_report(&fields(
            "failed to read /Users/alice/.codex/auth.json",
            "   0: usagebar_lib::run\n             at /Users/alice/src/lib.rs:12:5\n",
        ));
        let expected = "\
=== UsageBar crash report ===
time: 2025-11-05T12:27:58Z
version: 0.2.4
os: macos (aarch64)
thread: main
location: src/tray/mod.rs:42:9
message: failed to read ~/.codex/auth.json
backtrace:
     0: usagebar_lib::run
               at ~/src/lib.rs:12:5
";
        assert_eq!(report, expected);
    }

    #[test]
    fn multi_line_messages_cannot_forge_a_report_header() {
        let report = format_report(&fields("line one\n=== UsageBar crash report ===", ""));
        let headers = |text: &str| {
            text.lines()
                .filter(|line| line.starts_with(REPORT_HEADER))
                .count()
        };
        assert_eq!(headers(&report), 1);
        assert!(report.contains("message: line one\n  === UsageBar"));
        assert!(report.contains("backtrace: (unavailable)"));
        // Rotation counts it as one report too.
        let file = keep_recent(&report, &report, 2);
        assert_eq!(headers(&file), 2);
        assert_eq!(keep_recent(&file, &report, 2), file);
    }

    #[test]
    fn long_fields_are_truncated_on_char_boundaries() {
        let message = "é".repeat(MAX_MESSAGE_BYTES);
        let backtrace = "frame\n".repeat(MAX_BACKTRACE_BYTES);
        let report = format_report(&fields(&message, &backtrace));
        assert!(report.contains("… [truncated "));
        assert!(report.len() < MAX_MESSAGE_BYTES + MAX_BACKTRACE_BYTES * 2);
        let message_line = report.lines().find(|l| l.starts_with("message: ")).unwrap();
        assert!(message_line.len() <= "message: ".len() + MAX_MESSAGE_BYTES + 40);

        assert_eq!(truncate_at_char("héllo", 2), "h");
        assert_eq!(truncate_at_char("héllo", 3), "hé");
        assert_eq!(truncate_at_char("abc", 10), "abc");
    }

    #[test]
    fn capped_sink_stops_at_its_limit() {
        let mut sink = CappedString::new(5);
        let _ = write!(sink, "abcdefgh");
        let _ = write!(sink, "more");
        assert_eq!(sink.0, "abcde");
    }

    #[test]
    fn scrubs_bearer_and_basic_credentials() {
        let token = "a1".repeat(20);
        let out = scrub(&format!("header Authorization: Bearer {token} sent"));
        assert!(!out.contains(&token), "{out}");
        assert!(out.ends_with(" sent"), "{out}");
        assert_eq!(scrub("bearer abcdefghij"), "bearer [redacted]");
        assert_eq!(scrub("Basic dXNlcjpwYXNzd29yZA=="), "Basic [redacted]");
        // Ordinary prose that merely contains the word survives.
        assert_eq!(scrub("a basic error"), "a basic error");
    }

    #[test]
    fn scrubs_vendor_prefixed_keys() {
        assert_eq!(
            scrub("key sk-ant-api03-abcDEF123 bad"),
            "key [redacted] bad"
        );
        assert_eq!(
            scrub("got sk-proj-AbCdEf012345, retry"),
            "got [redacted], retry"
        );
        assert_eq!(scrub("ghp_abcdefghijklmnop"), "[redacted]");
        assert_eq!(scrub("AIzaSyA1234567890abcdefghijklmnopq"), "[redacted]");
        // Too short to be a key.
        assert_eq!(scrub("sk-1"), "sk-1");
    }

    #[test]
    fn scrubs_jwts() {
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjMifQ.c2lnbmF0dXJl";
        assert_eq!(
            scrub(&format!("jwt {jwt} expired")),
            "jwt [redacted] expired"
        );
        assert_eq!(scrub(&format!("({jwt})")), "([redacted])");
    }

    #[test]
    fn scrubs_named_values_in_queries_and_debug_output() {
        assert_eq!(
            scrub("GET https://x.invalid/v1?api_key=abc123&page=2 failed"),
            "GET https://x.invalid/v1?api_key=[redacted]&page=2 failed"
        );
        assert_eq!(
            scrub("https://x.invalid/?access_token=zz&key=yy"),
            "https://x.invalid/?access_token=[redacted]&key=[redacted]"
        );
        assert_eq!(
            scrub(r#"Creds { refresh_token: "r-1", expires: 3 }"#),
            r#"Creds { refresh_token: "[redacted]", expires: 3 }"#
        );
        assert_eq!(
            scrub(r#"{"client_secret":"shh","id":"7"}"#),
            r#"{"client_secret":"[redacted]","id":"7"}"#
        );
        assert_eq!(scrub("password = hunter2"), "password = [redacted]");
        assert_eq!(scrub("X-Api-Key: k1"), "X-Api-Key: [redacted]");
        // Not credentials: paths, locations, ordinary pairs.
        let benign = "core::panicking::panic at src/main.rs:12:5 id=7 len: 3 design=ok";
        assert_eq!(scrub(benign), benign);
    }

    #[test]
    fn scrubs_long_base64_and_hex_runs() {
        let hex = "0123456789abcdef0123456789abcdef";
        assert_eq!(scrub(&format!("hash {hex}!")), "hash [redacted]!");
        let b64 = "QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVo0NTY3ODk=";
        assert_eq!(scrub(b64), "[redacted]");
        // Long identifiers without digits and ordinary paths survive.
        let path = "/rustc/library/std/src/panicking.rs tauri_runtime_wry_event_loop_handler";
        assert_eq!(scrub(path), path);
        // A secret path component is redacted, the rest of the path kept.
        assert_eq!(scrub(&format!("/tmp/{hex}/file")), "/tmp/[redacted]/file");
    }

    #[test]
    fn scrubbing_is_idempotent() {
        let once = scrub("token=abc Bearer abcdefghijkl sk-abcdefghijk");
        assert_eq!(scrub(&once), once);
        assert!(!once.contains("abcdefghijkl"));
    }

    #[test]
    fn keeps_only_the_most_recent_reports() {
        let report = |n: usize| format!("{REPORT_HEADER}\nmessage: {n}\n");
        let mut file = String::new();
        for n in 0..8 {
            file = keep_recent(&file, &report(n), 3);
        }
        assert_eq!(file.matches(REPORT_HEADER).count(), 3);
        assert_eq!(file, format!("{}\n{}\n{}", report(5), report(6), report(7)));
        // Junk before the first header (a hand-edited file) is dropped.
        let kept = keep_recent("garbage\n", &report(1), 3);
        assert_eq!(kept, report(1));
        assert_eq!(keep_recent(&report(0), &report(1), 1), report(1));
    }

    #[test]
    fn write_report_rotates_and_sets_the_marker_once() {
        let dir = temp_dir("rotate");
        assert!(!take_pending(&dir));
        for n in 0..(KEEP_REPORTS + 3) {
            let report = format_report(&fields(&format!("crash number {n}"), ""));
            write_report(&dir, &report).expect("write");
        }
        let file = std::fs::read_to_string(dir.join(REPORT_FILE)).expect("read");
        assert_eq!(file.matches(REPORT_HEADER).count(), KEEP_REPORTS);
        assert!(!file.contains("crash number 2\n"));
        assert!(file.contains("crash number 3\n"));
        assert!(file.contains(&format!("crash number {}\n", KEEP_REPORTS + 2)));
        assert!(!dir.join(format!("{REPORT_FILE}.tmp")).exists());

        // The crash since the last launch is noticed exactly once.
        assert!(take_pending(&dir));
        assert!(!take_pending(&dir));
        assert!(dir.join(REPORT_FILE).is_file(), "the report itself stays");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_oversized_foreign_file_is_replaced() {
        let dir = temp_dir("oversized");
        let path = dir.join(REPORT_FILE);
        std::fs::write(&path, "x".repeat(MAX_EXISTING_BYTES as usize + 1)).expect("seed");
        write_report(&dir, &format_report(&fields("fresh", ""))).expect("write");
        let file = std::fs::read_to_string(&path).expect("read");
        assert!(file.starts_with(REPORT_HEADER));
        assert!(file.len() < 4096);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The real hook, against a temp dir, with a real (unwinding) panic.
    #[test]
    fn hook_writes_a_scrubbed_report_for_a_real_panic() {
        static TEST_TARGET: Mutex<Option<Target>> = Mutex::new(None);
        let dir = temp_dir("hook");
        *TEST_TARGET.lock().unwrap() = Some(Target {
            dir: dir.join("nested").join("logs"),
            home: Some("/home/tester".to_owned()),
        });

        let saved = std::panic::take_hook();
        install_with(&TEST_TARGET);
        let token = "f0e1".repeat(10);
        let result = std::thread::Builder::new()
            .name("crash-test".to_owned())
            .spawn(move || {
                std::panic::catch_unwind(|| {
                    panic!("hook-test boom at /home/tester/x with token={token}");
                })
            })
            .expect("spawn")
            .join();
        drop(std::panic::take_hook());
        std::panic::set_hook(saved);
        assert!(matches!(result, Ok(Err(_))), "the panic was caught");

        let logs = dir.join("nested").join("logs");
        let file = std::fs::read_to_string(logs.join(REPORT_FILE)).expect("report written");
        let report = file
            .split(REPORT_HEADER)
            .find(|r| r.contains("hook-test boom"))
            .expect("this panic's report");
        assert!(report.contains("message: hook-test boom at ~/x with token=[redacted]"));
        assert!(!report.contains("f0e1f0e1"));
        assert!(report.contains("thread: crash-test"));
        assert!(report.contains("location: src/crashlog.rs:"));
        assert!(report.contains(&format!("version: {}", env!("CARGO_PKG_VERSION"))));
        assert!(report.contains(&format!("os: {} (", std::env::consts::OS)));
        assert!(report.contains("backtrace:"));
        assert!(take_pending(&logs));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
