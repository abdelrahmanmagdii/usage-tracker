//! The provider registry: the one list of usage providers, and the plumbing
//! every provider shares (startup refresh, the staleness watchdog, and the
//! lookups the tray and commands do).
//!
//! These used to be hand-copied seven times across `lib.rs`, `commands.rs`
//! and the tray, which is how a provider ended up with a slightly different
//! loop than its siblings. Now each manager describes itself once through
//! [`Meter`] and everything else iterates [`Meters`].
//!
//! Adding a provider here takes a `Provider` variant, its row in
//! [`Provider::info`], a `Meter` impl for its manager, and an arm in
//! [`create_meter`]. The matches are exhaustive, so the compiler points at
//! each one that is still missing.

use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

use serde::Serialize;
use serde_json::Value;
use tauri::{App, AppHandle, Manager};

use crate::antigravity::AntigravityManager;
use crate::claude::ClaudeManager;
use crate::codex::process::{CodexManager, ConnectionState};
use crate::cursor::CursorManager;
use crate::devin::DevinManager;
use crate::gemini::GeminiManager;
use crate::opencode::OpenCodeManager;
use crate::provider::now_unix_seconds;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Provider {
    Codex,
    Claude,
    Cursor,
    OpenCode,
    Devin,
    Antigravity,
    Gemini,
}

/// Everything static about a provider, kept in one row so a new provider is
/// one match arm rather than one per accessor.
pub struct ProviderInfo {
    /// Stable id used in preferences, commands, and events.
    pub key: &'static str,
    pub display_name: &'static str,
    /// The provider's own item in the one-icon-per-tool menu-bar layout.
    pub tray_id: &'static str,
    /// Menu-bar ink. Compact layout uses these as left-to-right bars next to
    /// the percentages; extended layout paints the same colors into each
    /// tool's logo. Distinct on purpose.
    pub color: [f64; 3],
}

impl Provider {
    /// Display order everywhere: the tray title, menus, and the picker.
    pub const ALL: [Provider; 7] = [
        Provider::Codex,
        Provider::Claude,
        Provider::Cursor,
        Provider::OpenCode,
        Provider::Devin,
        Provider::Antigravity,
        Provider::Gemini,
    ];

    pub const fn info(self) -> ProviderInfo {
        match self {
            Provider::Codex => ProviderInfo {
                key: crate::prefs::PROVIDER_CODEX,
                display_name: "Codex",
                tray_id: "provider-codex",
                color: [140.0, 92.0, 240.0], // purple
            },
            Provider::Claude => ProviderInfo {
                key: crate::prefs::PROVIDER_CLAUDE,
                display_name: "Claude Code",
                tray_id: "provider-claude",
                color: [217.0, 119.0, 87.0], // coral
            },
            Provider::Cursor => ProviderInfo {
                key: crate::prefs::PROVIDER_CURSOR,
                display_name: "Cursor",
                tray_id: "provider-cursor",
                color: [15.0, 157.0, 142.0], // teal
            },
            Provider::OpenCode => ProviderInfo {
                key: crate::prefs::PROVIDER_OPENCODE,
                display_name: "OpenCode Go",
                tray_id: "provider-opencode",
                color: [79.0, 70.0, 229.0], // indigo
            },
            Provider::Devin => ProviderInfo {
                key: crate::prefs::PROVIDER_DEVIN,
                display_name: "Devin",
                tray_id: "provider-devin",
                color: [212.0, 132.0, 38.0], // amber
            },
            Provider::Antigravity => ProviderInfo {
                key: crate::prefs::PROVIDER_ANTIGRAVITY,
                display_name: "Antigravity",
                tray_id: "provider-antigravity",
                color: [66.0, 133.0, 244.0], // blue
            },
            Provider::Gemini => ProviderInfo {
                key: crate::prefs::PROVIDER_GEMINI,
                display_name: "Gemini CLI",
                tray_id: "provider-gemini",
                color: [203.0, 88.0, 192.0], // magenta
            },
        }
    }

    pub const fn key(self) -> &'static str {
        self.info().key
    }

    pub const fn display_name(self) -> &'static str {
        self.info().display_name
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|provider| provider.key() == key)
    }

    /// Parses a provider id handed in by the renderer.
    pub fn parse(key: &str) -> Result<Self, String> {
        Self::from_key(key).ok_or_else(|| format!("Unknown provider: {key}"))
    }

    /// Codex is the primary meter: it keeps its place in the menu bar even
    /// before its CLI is found, so the popover's setup path stays one click
    /// away. Every other tool appears only once it is detected.
    pub fn always_listed(self) -> bool {
        self == Provider::Codex
    }

    /// The reset radar tracks announced Codex resets, so only Codex's title
    /// carries the ⚡ marker.
    pub fn follows_reset_radar(self) -> bool {
        self == Provider::Codex
    }

    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|provider| *provider == self)
            .expect("every provider is listed in Provider::ALL")
    }
}

/// The part of a provider snapshot the tray and watchdog read.
#[derive(Debug, Clone)]
pub struct Reading {
    pub connection: ConnectionState,
    pub rate_limits: Option<Value>,
    pub updated_at: Option<u64>,
}

/// Copies the shared fields out of any snapshot shape (`CodexState`,
/// `ProviderState`, `ClaudeState`). A macro rather than `From` impls so it
/// keeps compiling whether or not `ClaudeState` is its own type.
macro_rules! reading_of {
    ($state:expr) => {{
        let state = $state;
        Reading {
            connection: state.connection,
            rate_limits: state.rate_limits,
            updated_at: state.updated_at,
        }
    }};
}

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

fn to_json(state: impl Serialize) -> Result<Value, String> {
    serde_json::to_value(state).map_err(|error| error.to_string())
}

/// Meters refresh when their data is older than this.
pub const STALE_AFTER_SECS: u64 = 5 * 60;
/// Steady-state spacing between refresh attempts, so a healthy meter cannot
/// hammer its backend.
pub const MIN_RETRY_SECS: u64 = 4 * 60;
/// Spacing after the first failed attempt. A denied keychain prompt or a
/// dropped network call recovers on the next watchdog tick instead of leaving
/// the meter wrong (or hidden) for minutes.
pub const FIRST_RETRY_SECS: u64 = 30;
/// How often most watchdogs wake to compare `updated_at` with the wall clock.
pub const DEFAULT_TICK_SECS: u64 = 30;
/// After a fresh local launch whose refresh did not land, the next try comes
/// this soon: the new server is usually still starting, not broken.
pub const LOCAL_LAUNCH_RETRY_SECS: u64 = 5;

/// How long to wait before the next attempt: healthy meters keep the steady
/// cadence, while a run of failures backs off from `FIRST_RETRY_SECS` up to the
/// same ceiling, so a persistent outage (an expired session that only Claude
/// Code itself can renew) is not retried any harder than before.
pub fn retry_after_secs(consecutive_failures: u32) -> u64 {
    if consecutive_failures == 0 {
        return MIN_RETRY_SECS;
    }
    let doublings = consecutive_failures.saturating_sub(1).min(16);
    FIRST_RETRY_SECS
        .saturating_mul(1_u64 << doublings)
        .min(MIN_RETRY_SECS)
}

/// Whether a meter is due for a refresh. Data younger than `STALE_AFTER_SECS`
/// is left alone; anything older waits only for the retry spacing.
pub fn should_refresh(
    now: u64,
    updated_at: Option<u64>,
    last_attempt: u64,
    consecutive_failures: u32,
) -> bool {
    let fresh = updated_at.is_some_and(|at| now.saturating_sub(at) < STALE_AFTER_SECS);
    if fresh {
        return false;
    }
    now.saturating_sub(last_attempt) >= retry_after_secs(consecutive_failures)
}

/// A provider-specific reason to refresh on this tick, on top of staleness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// Nothing beyond the staleness schedule.
    None,
    /// The meter has likely moved (Claude Code was used since the last
    /// reading). Refresh now, with ordinary failure bookkeeping.
    Due,
    /// The stored login was rewritten (a token rotation or fresh sign-in), so
    /// earlier failures say nothing about the next attempt: refresh now from a
    /// clean slate.
    LoginChanged,
    /// A new local server launched (Antigravity on Windows). A refresh that
    /// does not land is retried after `LOCAL_LAUNCH_RETRY_SECS` instead of
    /// backing off, because the server is usually still starting up.
    LocalLaunch,
}

/// The watchdog's bookkeeping, kept free of I/O so the timing is testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Schedule {
    pub last_attempt: u64,
    pub consecutive_failures: u32,
}

impl Schedule {
    /// Counts the startup refresh as an attempt and starts in the failure
    /// lane: if that refresh did not land, the meter recovers on the next
    /// tick instead of waiting out the steady-state interval. A refresh that
    /// did land is fresh, so the watchdog skips anyway.
    pub fn starting_at(now: u64) -> Self {
        Self {
            last_attempt: now,
            consecutive_failures: 1,
        }
    }

    /// Whether staleness alone makes this tick refresh.
    pub fn due(&self, now: u64, updated_at: Option<u64>) -> bool {
        should_refresh(
            now,
            updated_at,
            self.last_attempt,
            self.consecutive_failures,
        )
    }

    /// Decides whether this tick refreshes, recording the attempt if so.
    pub fn begin(&mut self, now: u64, updated_at: Option<u64>, trigger: Trigger) -> bool {
        match trigger {
            Trigger::None if !self.due(now, updated_at) => return false,
            Trigger::LoginChanged => self.consecutive_failures = 0,
            Trigger::None | Trigger::Due | Trigger::LocalLaunch => {}
        }
        self.last_attempt = now;
        true
    }

    /// Records how the attempt begun at `now` went. Success is measured by
    /// `updated_at` moving rather than by the returned `Result`: providers
    /// report "signed out" as an `Ok` state, and that still leaves the meter
    /// frozen.
    pub fn finish(&mut self, now: u64, landed: bool, trigger: Trigger) {
        if landed {
            self.consecutive_failures = 0;
        } else if trigger == Trigger::LocalLaunch {
            self.consecutive_failures = 1;
            self.last_attempt =
                now.saturating_sub(retry_after_secs(1).saturating_sub(LOCAL_LAUNCH_RETRY_SECS));
        } else {
            self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        }
    }
}

/// What the shared plumbing needs from one provider's manager. Implemented
/// here rather than in the manager files, so each backend stays free of
/// registry concerns. Object-safe (boxed futures) so the managers can sit in
/// one list.
pub trait Meter: Send + Sync + 'static {
    fn provider(&self) -> Provider;

    /// The fields the tray and watchdog read from the current snapshot.
    fn reading(&self) -> BoxFuture<'_, Reading>;

    /// The full snapshot, as the renderer receives it.
    fn state_json(&self) -> BoxFuture<'_, Result<Value, String>>;

    /// A background refresh: startup, the watchdog, and the tray's Refresh
    /// item. Never shows a credential prompt.
    fn background_refresh(&self) -> BoxFuture<'_, ()>;

    /// The renderer's Retry button. Providers that read the keychain may show
    /// its sheet once here.
    fn user_refresh(&self) -> BoxFuture<'_, Result<Value, String>>;

    /// The first refresh after launch.
    fn startup(&self) -> BoxFuture<'_, ()> {
        self.background_refresh()
    }

    /// How often the watchdog wakes to check this meter.
    fn tick_secs(&self) -> u64 {
        DEFAULT_TICK_SECS
    }

    /// A reason to refresh beyond staleness, checked every tick. `due` says
    /// whether the schedule already wants a refresh, so expensive checks can
    /// be skipped.
    fn trigger(
        &self,
        _now: u64,
        _updated_at: Option<u64>,
        _schedule: Schedule,
        _due: bool,
    ) -> BoxFuture<'_, Trigger> {
        Box::pin(async { Trigger::None })
    }
}

impl Meter for CodexManager {
    fn provider(&self) -> Provider {
        Provider::Codex
    }

    fn reading(&self) -> BoxFuture<'_, Reading> {
        Box::pin(async move { reading_of!(self.snapshot().await) })
    }

    fn state_json(&self) -> BoxFuture<'_, Result<Value, String>> {
        Box::pin(async move { to_json(self.snapshot().await) })
    }

    fn background_refresh(&self) -> BoxFuture<'_, ()> {
        // Codex's App Server can exit; a refresh relaunches it when needed.
        Box::pin(async move {
            let _ = self.refresh_or_start().await;
        })
    }

    fn user_refresh(&self) -> BoxFuture<'_, Result<Value, String>> {
        Box::pin(async move { self.refresh_or_start().await.and_then(to_json) })
    }

    fn startup(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let _ = CodexManager::start(self).await;
        })
    }
}

impl Meter for ClaudeManager {
    fn provider(&self) -> Provider {
        Provider::Claude
    }

    fn reading(&self) -> BoxFuture<'_, Reading> {
        Box::pin(async move { reading_of!(self.snapshot().await) })
    }

    fn state_json(&self) -> BoxFuture<'_, Result<Value, String>> {
        Box::pin(async move { to_json(self.snapshot().await) })
    }

    fn background_refresh(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let _ = self.refresh().await;
        })
    }

    fn user_refresh(&self) -> BoxFuture<'_, Result<Value, String>> {
        Box::pin(async move { self.refresh_from_user().await.and_then(to_json) })
    }

    /// A shorter tick than the other providers: each one checks whether
    /// Claude Code rewrote its login and whether it has been used since the
    /// last reading. Claude has no push channel like Codex's App Server, so
    /// without the second check the meter trailed `/usage` by up to five
    /// minutes during a busy session.
    fn tick_secs(&self) -> u64 {
        10
    }

    fn trigger(
        &self,
        now: u64,
        updated_at: Option<u64>,
        schedule: Schedule,
        due: bool,
    ) -> BoxFuture<'_, Trigger> {
        Box::pin(async move {
            // Checked on every tick, due or not: each call also records the
            // stamp the next one compares against.
            if self.login_changed().await {
                return Trigger::LoginChanged;
            }
            if !due
                && self
                    .active_refresh_due(
                        now,
                        updated_at,
                        schedule.last_attempt,
                        schedule.consecutive_failures,
                    )
                    .await
            {
                return Trigger::Due;
            }
            Trigger::None
        })
    }
}

impl Meter for AntigravityManager {
    fn provider(&self) -> Provider {
        Provider::Antigravity
    }

    fn reading(&self) -> BoxFuture<'_, Reading> {
        Box::pin(async move { reading_of!(self.snapshot().await) })
    }

    fn state_json(&self) -> BoxFuture<'_, Result<Value, String>> {
        Box::pin(async move { to_json(self.snapshot().await) })
    }

    fn background_refresh(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let _ = self.refresh().await;
        })
    }

    fn user_refresh(&self) -> BoxFuture<'_, Result<Value, String>> {
        Box::pin(async move { self.refresh_from_user().await.and_then(to_json) })
    }

    /// Windows also watches for a fresh Antigravity launch log, which needs a
    /// short tick; elsewhere that check is always false.
    fn tick_secs(&self) -> u64 {
        if cfg!(target_os = "windows") {
            LOCAL_LAUNCH_RETRY_SECS
        } else {
            DEFAULT_TICK_SECS
        }
    }

    fn trigger(
        &self,
        _now: u64,
        _updated_at: Option<u64>,
        _schedule: Schedule,
        _due: bool,
    ) -> BoxFuture<'_, Trigger> {
        Box::pin(async move {
            if self.should_poll_local_launch().await {
                Trigger::LocalLaunch
            } else {
                Trigger::None
            }
        })
    }
}

/// Providers with nothing beyond `snapshot()` and `refresh()`.
macro_rules! plain_meter {
    ($manager:ty, $provider:expr) => {
        impl Meter for $manager {
            fn provider(&self) -> Provider {
                $provider
            }

            fn reading(&self) -> BoxFuture<'_, Reading> {
                Box::pin(async move { reading_of!(self.snapshot().await) })
            }

            fn state_json(&self) -> BoxFuture<'_, Result<Value, String>> {
                Box::pin(async move { to_json(self.snapshot().await) })
            }

            fn background_refresh(&self) -> BoxFuture<'_, ()> {
                Box::pin(async move {
                    let _ = self.refresh().await;
                })
            }

            fn user_refresh(&self) -> BoxFuture<'_, Result<Value, String>> {
                Box::pin(async move { self.refresh().await.and_then(to_json) })
            }
        }
    };
}

plain_meter!(CursorManager, Provider::Cursor);
plain_meter!(OpenCodeManager, Provider::OpenCode);
plain_meter!(DevinManager, Provider::Devin);
plain_meter!(GeminiManager, Provider::Gemini);

/// Every provider's manager, in `Provider::ALL` order.
pub struct Meters(Vec<Arc<dyn Meter>>);

impl Meters {
    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn Meter>> {
        self.0.iter()
    }

    pub fn get(&self, provider: Provider) -> &Arc<dyn Meter> {
        &self.0[provider.index()]
    }
}

/// Builds one provider's manager and also manages it under its concrete type,
/// which the per-provider commands and a few provider-specific paths (Codex
/// shutdown, Claude's refresh on open) still look up.
fn create_meter(app: &App, provider: Provider) -> Arc<dyn Meter> {
    fn managed<M: Meter + Clone>(app: &App, manager: M) -> Arc<dyn Meter> {
        app.manage(manager.clone());
        Arc::new(manager)
    }
    let handle = app.handle().clone();
    match provider {
        Provider::Codex => managed(app, CodexManager::new(handle)),
        Provider::Claude => managed(app, ClaudeManager::new(handle)),
        Provider::Cursor => managed(app, CursorManager::new(handle)),
        Provider::OpenCode => managed(app, OpenCodeManager::new(handle)),
        Provider::Devin => managed(app, DevinManager::new(handle)),
        Provider::Antigravity => managed(app, AntigravityManager::new(handle)),
        Provider::Gemini => managed(app, GeminiManager::new(handle)),
    }
}

/// Creates and manages every provider's manager plus the shared [`Meters`]
/// list. Call before the tray is set up; it reads the list on every repaint.
pub fn install(app: &App) {
    let meters = Provider::ALL
        .into_iter()
        .map(|provider| create_meter(app, provider))
        .collect();
    app.manage(Meters(meters));
}

/// Kicks off each provider's first refresh and its staleness watchdog.
pub fn spawn_refreshers(app: &AppHandle) {
    let Some(meters) = app.try_state::<Meters>() else {
        return;
    };
    for meter in meters.iter() {
        let starter = meter.clone();
        tauri::async_runtime::spawn(async move {
            starter.startup().await;
        });
        spawn_watchdog(meter.clone());
    }
}

/// Wall-clock staleness watchdog instead of a plain sleep loop: tokio timers
/// pause during system sleep, so a fixed 5-minute sleep silently stretched
/// across naps. Checking `updated_at` against the wall clock every tick catches
/// up within one tick of waking, while the retry spacing keeps failures from
/// hammering the backends.
fn spawn_watchdog(meter: Arc<dyn Meter>) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(meter.tick_secs()));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut schedule = Schedule::starting_at(now_unix_seconds());
        loop {
            interval.tick().await;
            let now = now_unix_seconds();
            let before = meter.reading().await.updated_at;
            let due = schedule.due(now, before);
            let trigger = meter.trigger(now, before, schedule, due).await;
            if !schedule.begin(now, before, trigger) {
                continue;
            }
            meter.background_refresh().await;
            let after = meter.reading().await.updated_at;
            schedule.finish(now, after != before, trigger);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_retry_quickly_then_back_off_to_the_steady_cadence() {
        assert_eq!(retry_after_secs(0), MIN_RETRY_SECS);
        assert_eq!(retry_after_secs(1), FIRST_RETRY_SECS);
        assert_eq!(retry_after_secs(2), 60);
        assert_eq!(retry_after_secs(3), 120);
        // The backoff never exceeds the healthy cadence, so a session only
        // Claude Code can renew is not polled harder than before.
        assert_eq!(retry_after_secs(4), MIN_RETRY_SECS);
        assert_eq!(retry_after_secs(u32::MAX), MIN_RETRY_SECS);
    }

    #[test]
    fn fresh_data_is_left_alone_and_stale_data_retries_on_schedule() {
        let now = 1_000_000;
        // Data younger than the staleness window is never re-fetched.
        assert!(!should_refresh(now, Some(now - 60), now - 3_600, 0));
        // Stale data waits out the steady cadence after a healthy run...
        assert!(!should_refresh(
            now,
            Some(now - STALE_AFTER_SECS),
            now - 60,
            0
        ));
        assert!(should_refresh(
            now,
            Some(now - STALE_AFTER_SECS),
            now - MIN_RETRY_SECS,
            0
        ));
        // ...but a failed attempt is retried on the next watchdog tick, which
        // is what pulls a hidden or frozen Claude meter back within seconds.
        assert!(should_refresh(
            now,
            Some(now - STALE_AFTER_SECS),
            now - FIRST_RETRY_SECS,
            1
        ));
        assert!(!should_refresh(
            now,
            Some(now - STALE_AFTER_SECS),
            now - 10,
            1
        ));
        // A meter that never got data at all is due as soon as spacing allows.
        assert!(should_refresh(now, None, now - FIRST_RETRY_SECS, 1));
        assert!(!should_refresh(now, None, now, 1));
    }

    #[test]
    fn a_startup_refresh_that_missed_is_retried_on_the_first_retry_spacing() {
        let start = 1_000_000;
        let mut schedule = Schedule::starting_at(start);
        // No data yet, but the startup attempt was just made.
        assert!(!schedule.begin(start + 10, None, Trigger::None));
        assert_eq!(schedule.last_attempt, start);
        assert!(schedule.begin(start + FIRST_RETRY_SECS, None, Trigger::None));
        assert_eq!(schedule.last_attempt, start + FIRST_RETRY_SECS);
    }

    #[test]
    fn landing_resets_the_backoff_and_misses_extend_it() {
        let now = 1_000_000;
        let mut schedule = Schedule::starting_at(now);
        schedule.finish(now, false, Trigger::None);
        assert_eq!(schedule.consecutive_failures, 2);
        schedule.finish(now, false, Trigger::Due);
        assert_eq!(schedule.consecutive_failures, 3);
        schedule.finish(now, true, Trigger::None);
        assert_eq!(schedule.consecutive_failures, 0);
        // Fresh data is left alone until it ages out.
        assert!(!schedule.begin(now + 60, Some(now), Trigger::None));
    }

    #[test]
    fn a_rewritten_login_refreshes_at_once_from_a_clean_slate() {
        let now = 1_000_000;
        let mut schedule = Schedule {
            last_attempt: now - 1,
            consecutive_failures: 5,
        };
        // Fresh data and a just-made attempt would normally skip this tick.
        assert!(schedule.begin(now, Some(now - 1), Trigger::LoginChanged));
        assert_eq!(schedule.last_attempt, now);
        assert_eq!(schedule.consecutive_failures, 0);
        // A miss after the rewrite counts as the first failure, so the next
        // try comes on the short spacing rather than the old backoff.
        schedule.finish(now, false, Trigger::LoginChanged);
        assert_eq!(schedule.consecutive_failures, 1);
    }

    #[test]
    fn activity_refreshes_fresh_data_with_ordinary_bookkeeping() {
        let now = 1_000_000;
        let mut schedule = Schedule {
            last_attempt: now - 20,
            consecutive_failures: 2,
        };
        assert!(schedule.begin(now, Some(now - 30), Trigger::Due));
        assert_eq!(schedule.last_attempt, now);
        assert_eq!(schedule.consecutive_failures, 2);
        schedule.finish(now, false, Trigger::Due);
        assert_eq!(schedule.consecutive_failures, 3);
    }

    #[test]
    fn a_local_launch_that_did_not_land_is_retried_after_a_few_seconds() {
        let now = 1_000_000;
        let mut schedule = Schedule {
            last_attempt: now - 1,
            consecutive_failures: 4,
        };
        assert!(schedule.begin(now, Some(now - 1), Trigger::LocalLaunch));
        schedule.finish(now, false, Trigger::LocalLaunch);
        // Back on the first retry spacing, with the clock wound back so the
        // next attempt is due `LOCAL_LAUNCH_RETRY_SECS` from now rather than
        // after the full backoff.
        assert_eq!(schedule.consecutive_failures, 1);
        assert!(!schedule.due(now + LOCAL_LAUNCH_RETRY_SECS - 1, None));
        assert!(schedule.due(now + LOCAL_LAUNCH_RETRY_SECS, None));
        // A launch whose refresh landed is simply healthy.
        let mut landed = Schedule::starting_at(now);
        landed.finish(now, true, Trigger::LocalLaunch);
        assert_eq!(landed.consecutive_failures, 0);
        assert_eq!(landed.last_attempt, now);
    }

    #[test]
    fn provider_ids_are_unique_and_match_the_preference_keys() {
        let keys: Vec<&str> = Provider::ALL.iter().map(|p| p.key()).collect();
        assert_eq!(
            keys,
            vec![
                crate::prefs::PROVIDER_CODEX,
                crate::prefs::PROVIDER_CLAUDE,
                crate::prefs::PROVIDER_CURSOR,
                crate::prefs::PROVIDER_OPENCODE,
                crate::prefs::PROVIDER_DEVIN,
                crate::prefs::PROVIDER_ANTIGRAVITY,
                crate::prefs::PROVIDER_GEMINI,
            ]
        );
        for (index, provider) in Provider::ALL.into_iter().enumerate() {
            assert_eq!(provider.index(), index);
            assert_eq!(Provider::from_key(provider.key()), Some(provider));
            assert_eq!(Provider::parse(provider.key()), Ok(provider));
            assert!(provider.info().tray_id.ends_with(provider.key()));
        }
        let mut tray_ids: Vec<&str> = Provider::ALL.iter().map(|p| p.info().tray_id).collect();
        tray_ids.sort_unstable();
        tray_ids.dedup();
        assert_eq!(tray_ids.len(), Provider::ALL.len());
        assert_eq!(
            Provider::parse("copilot"),
            Err("Unknown provider: copilot".to_owned())
        );
        assert!(Provider::from_key("").is_none());
    }
}
