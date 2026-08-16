import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Clock3, Power, RefreshCw, Settings2, Share2, ShieldCheck, Ticket } from "lucide-react";
import { MeterMark } from "./components/MeterMark";
import { meterTone } from "./components/EdgeMeter";
import { QuotaSection } from "./components/QuotaSection";
import { ConnectionStateView } from "./components/ConnectionState";
import { UsageDetails } from "./components/UsageDetails";
import { TiboWatch } from "./components/TiboWatch";
import { ShareModal } from "./components/ShareModal";
import { ResetAlert } from "./components/ResetAlert";
import { SettingsModal } from "./components/SettingsModal";
import { ClaudeSection } from "./components/ClaudeSection";
import { Onboarding } from "./components/Onboarding";
import { useCodexMeter } from "./hooks/useCodexMeter";
import { useClaudeMeter } from "./hooks/useClaudeMeter";
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
  const [now, setNow] = useState(Date.now());
  const [sharing, setSharing] = useState(false);
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
    void invoke<{ onboardingComplete: boolean }>("get_app_prefs")
      .then((prefs) => active && setOnboarding(!prefs.onboardingComplete))
      .catch(() => undefined);
    const unlisten = listen("usagebar://show-onboarding", () => {
      if (active) setOnboarding(true);
    });
    return () => {
      active = false;
      void unlisten.then((off) => off());
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
  const connected = state.connection === "connected";
  const mostCooked = buckets.reduce<(typeof buckets)[number] | undefined>(
    (lowest, bucket) => !lowest || bucket.remainingPercent < lowest.remainingPercent ? bucket : lowest,
    undefined,
  );

  return (
    <main className="app-shell" data-tauri-drag-region>
      <header
        className="app-header"
        data-tauri-drag-region
        onMouseDown={(event) => {
          if (event.button === 0 && "__TAURI_INTERNALS__" in window) {
            void getCurrentWindow().startDragging();
          }
        }}
      >
        <div className="header-toolbar" data-tauri-drag-region>
          <div className="brand-row" data-tauri-drag-region>
            <MeterMark />
              <div data-tauri-drag-region>
                <h1 data-tauri-drag-region>Codex</h1>
                <p data-tauri-drag-region>Usage meter</p>
            </div>
          </div>
          <span className="toolbar-divider" aria-hidden="true" />
          {connected && mostCooked ? (
            <div
              className={`status-summary tone-${meterTone(mostCooked)}`}
              title={`${windowDurationLabel(mostCooked.windowDurationMins)} quota window`}
              data-tauri-drag-region
            >
              <div className="available-value" data-tauri-drag-region>
                <strong data-tauri-drag-region>{Math.round(mostCooked.usedPercent)}</strong><span data-tauri-drag-region>%</span>
              </div>
              <div className="status-copy" data-tauri-drag-region>
                <span className="status-eyebrow" data-tauri-drag-region><i aria-hidden="true" />Used · {windowDurationLabel(mostCooked.windowDurationMins)}</span>
                <strong data-tauri-drag-region><Clock3 size={12} strokeWidth={2} aria-hidden="true" />{headerResetText(mostCooked, now)}</strong>
              </div>
            </div>
          ) : (
            <div className="status-summary status-summary--connection" title="Codex App Server connection" data-tauri-drag-region>
              <span className="connection-orb" aria-hidden="true" />
              <div className="status-copy" data-tauri-drag-region>
                <span className="status-eyebrow" data-tauri-drag-region>Codex status</span>
                <strong data-tauri-drag-region>{state.connection === "starting" ? "Connecting…" : "Offline"}</strong>
              </div>
            </div>
          )}
        </div>
      </header>

      <div className="content-scroll">
        {!connected ? (
          <ConnectionStateView state={state} onRetry={() => void refresh()} />
        ) : buckets.length ? (
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
        ) : (
          <div className="state-panel glass-tile" role="status">
            <ShieldCheck size={22} aria-hidden="true" />
            <h2>No rate-limit buckets returned</h2>
            <p>Codex is connected, but this account did not report a quota window.</p>
            <button className="secondary-button" onClick={() => void refresh()}>Refresh</button>
          </div>
        )}
        <ClaudeSection meter={claude} now={now} />
      </div>

      <footer className="app-footer">
        <button className="footer-action share-action" disabled={!mostCooked} onClick={() => setSharing(true)}>
          <Share2 size={16} aria-hidden="true" /> Share
        </button>
        <button
          className="icon-button"
          onClick={() => {
            void refresh();
            void claude.refresh();
          }}
          disabled={refreshing || claude.refreshing}
          aria-label="Refresh usage data"
          title="Refresh"
        >
          <RefreshCw size={17} className={refreshing || claude.refreshing ? "spinning" : ""} />
        </button>
        <button className="icon-button" onClick={() => setSettingsOpen(true)} aria-label="Open settings" title="Settings">
          <Settings2 size={17} />
        </button>
        <button className="icon-button danger-hover" onClick={() => void invoke("quit_app")} aria-label="Quit UsageBar" title="Quit">
          <Power size={17} />
        </button>
      </footer>
      {sharing && mostCooked ? <ShareModal bucket={mostCooked} onClose={() => setSharing(false)} /> : null}
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
          codex={{
            label: "Codex",
            connected,
            detail: connected
              ? "Connected through the Codex app server"
              : state.diagnostic ?? "Sign in with the codex CLI, then retry",
            onRetry: () => void refresh(),
          }}
          claude={
            claude.state.connection === "cli_not_found"
              ? null
              : {
                  label: "Claude Code",
                  connected: claude.state.connection === "connected",
                  detail:
                    claude.state.connection === "connected"
                      ? "Reading your existing Claude Code login"
                      : claude.state.diagnostic ?? "Open Claude Code once to sign in",
                  onRetry: () => void claude.refresh(),
                }
          }
        />
      ) : null}
    </main>
  );
}
