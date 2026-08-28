import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ChevronUp, Clock3, MousePointer2, Power, RefreshCw, Settings2, ShieldCheck, Sparkles, Terminal, Ticket } from "lucide-react";
import { MeterMark } from "./components/MeterMark";
import { meterTone } from "./components/EdgeMeter";
import { QuotaSection } from "./components/QuotaSection";
import { ConnectionStateView } from "./components/ConnectionState";
import { UsageDetails } from "./components/UsageDetails";
import { TiboWatch } from "./components/TiboWatch";
import { ResetAlert } from "./components/ResetAlert";
import { SettingsModal } from "./components/SettingsModal";
import { ProviderSection } from "./components/ProviderSection";
import { Onboarding } from "./components/Onboarding";
import { useCodexMeter } from "./hooks/useCodexMeter";
import { useClaudeMeter, useCursorMeter, useOpenCodeMeter } from "./hooks/useClaudeMeter";
import {
  DEFAULT_PREFS,
  isVisible,
  normalizePrefs,
  type AppPrefs,
} from "./lib/providers";
import { useResetEvents } from "./features/tibo-watch/useResetEvents";
import { upcomingReset } from "./features/tibo-watch/provider";
import { notifyFreshResets } from "./features/tibo-watch/notifications";
import { formatCountdown, windowDurationLabel } from "./lib/rateLimits";
import type { RateLimitBucket } from "./types/codex";

function headerResetText(bucket: RateLimitBucket, now: number): string {
  const countdown = bucket.resetsAt
    ? formatCountdown(bucket.resetsAt, now)
        .replace(/^Resets in /, "Renews in ")
        .replace(/^Reset due$/, "Renewing now")
        .replace(/^Reset time not reported$/, "Renewal time not reported")
    : "";
  return countdown || "Renewal unavailable";
}

export default function App() {
  const { state, buckets, resetCredits, usage, refreshing, refresh } = useCodexMeter();
  const claude = useClaudeMeter();
  const cursor = useCursorMeter();
  const opencode = useOpenCodeMeter();
  const [prefs, setPrefs] = useState<AppPrefs>(DEFAULT_PREFS);
  const [now, setNow] = useState(Date.now());
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [onboarding, setOnboarding] = useState(false);
  const resetEvents = useResetEvents();

  // First run shows the walkthrough; the tray's Setup Guide item reopens it.
  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) {
      setOnboarding(new URLSearchParams(window.location.search).has("onboarding"));
      return;
    }
    let active = true;
    void invoke<AppPrefs>("get_app_prefs")
      .then((next) => {
        if (!active) return;
        setPrefs(normalizePrefs(next));
        setOnboarding(!next.onboardingComplete);
      })
      .catch(() => undefined);
    const unlistenOnboarding = listen("usagebar://show-onboarding", () => {
      if (active) setOnboarding(true);
    });
    const unlistenPrefs = listen<AppPrefs>("usagebar://prefs", (event) => {
      if (active) setPrefs(normalizePrefs(event.payload));
    });
    return () => {
      active = false;
      void unlistenOnboarding.then((off) => off());
      void unlistenPrefs.then((off) => off());
    };
  }, []);

  const closeOnboarding = () => {
    setOnboarding(false);
    if ("__TAURI_INTERNALS__" in window) void invoke("complete_onboarding");
  };
  useEffect(() => {
    const interval = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(interval);
  }, []);
  useEffect(() => {
    void notifyFreshResets(resetEvents);
  }, [resetEvents]);
  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    const upcoming = upcomingReset(resetEvents);
    const until = upcoming?.occursAt
      ? Math.floor(Date.parse(upcoming.occursAt) / 1000)
      : null;
    void invoke("set_reset_incoming", { until });
  }, [resetEvents]);

  // Esc unwinds the topmost layer first — settings, onboarding — and only
  // then dismisses the popover itself, like any macOS popover.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      if (settingsOpen) {
        setSettingsOpen(false);
        return;
      }
      if (onboarding) {
        setOnboarding(false);
        if ("__TAURI_INTERNALS__" in window) void invoke("complete_onboarding");
        return;
      }
      if ("__TAURI_INTERNALS__" in window) void invoke("hide_window");
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [settingsOpen, onboarding]);
  const showCodex = isVisible(prefs, "codex");
  const showClaude = isVisible(prefs, "claude");
  const showCursor = isVisible(prefs, "cursor");
  const showOpenCode = isVisible(prefs, "opencode");
  const connected = state.connection === "connected";
  const visibleBuckets = [
    ...(showCodex ? buckets : []),
    ...(showClaude ? claude.buckets : []),
    ...(showCursor ? cursor.buckets : []),
    ...(showOpenCode ? opencode.buckets : []),
  ];
  const mostCooked = visibleBuckets.reduce<(typeof visibleBuckets)[number] | undefined>(
    (lowest, bucket) => !lowest || bucket.remainingPercent < lowest.remainingPercent ? bucket : lowest,
    undefined,
  );
  const anyRefreshing = refreshing || claude.refreshing || cursor.refreshing || opencode.refreshing;

  return (
    <main className="app-shell" data-tauri-drag-region>
      {/* The traffic-light corner: where macOS puts window chrome, so it is
          where the eye goes for "make this window go away". Hiding the popover
          sends it back to the menu bar — the app keeps running, which is what
          separates this from the power button in the footer. The chevron
          points at that home; the label spells it out on hover. */}
      <button
        type="button"
        className="dismiss-button"
        aria-label="Hide UsageBar to the menu bar"
        onClick={() => {
          if ("__TAURI_INTERNALS__" in window) void invoke("hide_window");
        }}
      >
        <ChevronUp size={15} strokeWidth={2.5} aria-hidden="true" />
        <span className="dismiss-label">Hide to menu bar</span>
      </button>
      <header
        className="app-header"
        data-tauri-drag-region
        onMouseDown={(event) => {
          // Only drag from a surface that opts in. Without this check the
          // window-button corner (and any control in the header) starts a drag
          // on mousedown and the click never reaches what it was aimed at.
          if (event.button !== 0) return;
          if (!(event.target instanceof Element)) return;
          if (!event.target.hasAttribute("data-tauri-drag-region")) return;
          if ("__TAURI_INTERNALS__" in window) {
            void getCurrentWindow().startDragging();
          }
        }}
      >
        <div className="header-toolbar" data-tauri-drag-region>
          <div className="brand-row" data-tauri-drag-region>
            <MeterMark />
              <div data-tauri-drag-region>
                <h1 data-tauri-drag-region>UsageBar</h1>
                <p data-tauri-drag-region>Usage meter</p>
            </div>
          </div>
          <span className="toolbar-divider" aria-hidden="true" />
          {mostCooked ? (
            <div
              className={`status-summary tone-${meterTone(mostCooked)}`}
              title={`${windowDurationLabel(mostCooked.windowDurationMins)} quota window`}
              data-tauri-drag-region
            >
              <div className="available-value" data-tauri-drag-region>
                <strong data-tauri-drag-region>{Math.round(mostCooked.remainingPercent)}</strong><span data-tauri-drag-region>%</span>
              </div>
              <div className="status-copy" data-tauri-drag-region>
                <span className="status-eyebrow" data-tauri-drag-region><i aria-hidden="true" />Left · {windowDurationLabel(mostCooked.windowDurationMins)}</span>
                <strong data-tauri-drag-region><Clock3 size={12} strokeWidth={2} aria-hidden="true" />{headerResetText(mostCooked, now)}</strong>
              </div>
            </div>
          ) : (
            <div className="status-summary status-summary--connection" title="UsageBar status" data-tauri-drag-region>
              <span className="connection-orb" aria-hidden="true" />
              <div className="status-copy" data-tauri-drag-region>
                <span className="status-eyebrow" data-tauri-drag-region>Status</span>
                <strong data-tauri-drag-region>{showCodex && state.connection === "starting" ? "Connecting…" : "Offline"}</strong>
              </div>
            </div>
          )}
        </div>
      </header>

      <div className="content-scroll">
        {showCodex && !connected ? (
          <ConnectionStateView state={state} onRetry={() => void refresh()} />
        ) : showCodex && buckets.length ? (
          <>
            <ResetAlert now={now} events={resetEvents} />
            <div className="quota-list">
              {buckets.map((bucket) => <QuotaSection key={bucket.id} bucket={bucket} now={now} />)}
            </div>
            {resetCredits.availableCount > 0 ? (
              <div className="reset-credit"><Ticket size={15} aria-hidden="true" /><strong>{resetCredits.availableCount}</strong> reset {resetCredits.availableCount === 1 ? "is" : "are"} available</div>
            ) : null}
            {usage ? <UsageDetails usage={usage} /> : null}
            <TiboWatch now={now} events={resetEvents} />
          </>
        ) : showCodex ? (
          <div className="state-panel glass-tile" role="status">
            <ShieldCheck size={22} aria-hidden="true" />
            <h2>No rate-limit buckets returned</h2>
            <p>Codex is connected, but this account did not report a quota window.</p>
            <button className="secondary-button" onClick={() => void refresh()}>Refresh</button>
          </div>
        ) : null}
        {showClaude ? (
          <ProviderSection
            id="claude"
            label="Claude Code"
            icon={<Sparkles size={14} aria-hidden="true" />}
            meter={claude}
            now={now}
            signedOutHint="the stored Claude Code login has expired. UsageBar reads the login kept by the `claude` command-line tool, so it refreshes the next time that runs — the desktop app keeps a separate login."
          />
        ) : null}
        {showCursor ? (
          <ProviderSection
            id="cursor"
            label="Cursor"
            icon={<MousePointer2 size={14} aria-hidden="true" />}
            meter={cursor}
            now={now}
            signedOutHint="the stored Cursor login was rejected. Sign in through the Cursor app, then retry."
          />
        ) : null}
        {showOpenCode ? (
          <ProviderSection
            id="opencode"
            label="OpenCode Go"
            icon={<Terminal size={14} aria-hidden="true" />}
            meter={opencode}
            now={now}
            signedOutHint="the stored OpenCode Go key was rejected. Sign in again with `/connect` and choose OpenCode Go."
          />
        ) : null}
      </div>

      <footer className="app-footer">
        <button className="footer-action" onClick={() => setSettingsOpen(true)}>
          <Settings2 size={16} aria-hidden="true" /> Settings
        </button>
        <button
          className="icon-button"
          onClick={() => {
            if (showCodex) void refresh();
            if (showClaude) void claude.refresh();
            if (showCursor) void cursor.refresh();
            if (showOpenCode) void opencode.refresh();
          }}
          disabled={anyRefreshing}
          aria-label="Refresh usage data"
          title="Refresh"
        >
          <RefreshCw size={17} className={anyRefreshing ? "spinning" : ""} />
        </button>
        <button className="icon-button danger-hover" onClick={() => void invoke("quit_app")} aria-label="Quit UsageBar" title="Quit">
          <Power size={17} />
        </button>
      </footer>
      {settingsOpen ? (
        <SettingsModal
          onClose={() => setSettingsOpen(false)}
          onShowGuide={() => {
            setSettingsOpen(false);
            setOnboarding(true);
          }}
        />
      ) : null}
      {onboarding ? (
        <Onboarding
          onClose={closeOnboarding}
          providers={[
            {
              label: "Codex",
              connected,
              detail: connected
                ? "Connected through the Codex app server"
                : state.diagnostic ?? "Sign in with the codex CLI, then retry",
              onRetry: () => void refresh(),
            },
            ...(claude.state.connection === "cli_not_found"
              ? []
              : [{
                  label: "Claude Code",
                  connected: claude.state.connection === "connected",
                  detail:
                    claude.state.connection === "connected"
                      ? "Reading the login kept by the claude command-line tool"
                      : claude.state.diagnostic ?? "Sign in with the claude command-line tool",
                  onRetry: () => void claude.refresh(),
                }]),
            ...(cursor.state.connection === "cli_not_found"
              ? []
              : [{
                  label: "Cursor",
                  connected: cursor.state.connection === "connected",
                  detail:
                    cursor.state.connection === "connected"
                      ? "Reading the login kept by the Cursor app"
                      : cursor.state.diagnostic ?? "Sign in through the Cursor app",
                  onRetry: () => void cursor.refresh(),
                }]),
            ...(opencode.state.connection === "cli_not_found"
              ? []
              : [{
                  label: "OpenCode Go",
                  connected: opencode.state.connection === "connected",
                  detail:
                    opencode.state.connection === "connected"
                      ? "Reading the OpenCode Go key from auth.json"
                      : opencode.state.diagnostic ?? "Sign in with /connect and choose OpenCode Go",
                  onRetry: () => void opencode.refresh(),
                }]),
          ]}
        />
      ) : null}
    </main>
  );
}
