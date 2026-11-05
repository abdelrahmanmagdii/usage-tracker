import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ChevronUp, Clock3, PlugZap, Power, RefreshCw, Settings2, ShieldCheck, Ticket } from "lucide-react";
import { MeterMark } from "./components/MeterMark";
import { meterTone } from "./components/EdgeMeter";
import { QuotaSection } from "./components/QuotaSection";
import { ConnectionStateView } from "./components/ConnectionState";
import { UsageDetails } from "./components/UsageDetails";
import { TiboWatch } from "./components/TiboWatch";
import { ResetAlert } from "./components/ResetAlert";
import { SettingsModal } from "./components/SettingsModal";
import { ProviderSection } from "./components/ProviderSection";
import { MissingProviders } from "./components/MissingProviders";
import { Onboarding } from "./components/Onboarding";
import { Freshness } from "./components/Freshness";
import { useProviderMeters } from "./hooks/useProviderMeters";
import { NowProvider, WithNow, useNow } from "./hooks/useNow";
import {
  DEFAULT_PREFS,
  PROVIDER_CATALOG,
  isPresent,
  isVisible,
  missingProviders,
  normalizePrefs,
  onboardingStatus,
  type AppPrefs,
} from "./lib/providers";
import { useResetEvents } from "./features/tibo-watch/useResetEvents";
import { upcomingReset } from "./features/tibo-watch/provider";
import { notifyFreshResets } from "./features/tibo-watch/notifications";
import { formatCountdown, windowDurationLabel } from "./lib/rateLimits";
import { demoMode } from "./lib/demo";
import { isLiveReading } from "./lib/time";
import { previewPrefs, previewResetEvent } from "./lib/preview";
import type { RateLimitBucket } from "./types/codex";
import { deviceName, trayName } from "./lib/platform";

const inTauri = () => "__TAURI_INTERNALS__" in window;

/** The tools that render through ProviderSection; Codex has its own block. */
const SECTION_PROVIDERS = PROVIDER_CATALOG.filter((provider) => provider.id !== "codex");

function headerResetText(bucket: RateLimitBucket, now: number): string {
  const countdown = bucket.resetsAt
    ? formatCountdown(bucket.resetsAt, now)
        .replace(/^Resets in /, "Renews in ")
        .replace(/^Reset due$/, "Renewing now")
        .replace(/^Reset time not reported$/, "Renewal time not reported")
    : "";
  return countdown || "Renewal unavailable";
}

/** The header's countdown line: the one piece of the shell that ticks. */
function HeaderRenewal({ bucket }: { bucket: RateLimitBucket }) {
  const now = useNow();
  return (
    <strong data-tauri-drag-region><Clock3 size={12} strokeWidth={2} aria-hidden="true" />{headerResetText(bucket, now)}</strong>
  );
}

export default function App() {
  // The clock sits above the popover so a tick re-renders only the
  // countdowns, not the shell, the meters' derived lists or an open sheet.
  return (
    <NowProvider>
      <Popover />
    </NowProvider>
  );
}

function Popover() {
  const meters = useProviderMeters();
  const { state, buckets, resetCredits, usage, refresh } = meters.codex;
  const [prefs, setPrefs] = useState<AppPrefs>(
    () => previewPrefs(window.location.search) ?? DEFAULT_PREFS,
  );
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [onboarding, setOnboarding] = useState(false);
  const [demo, setDemo] = useState(false);
  const liveResetEvents = useResetEvents();
  const resetEvents = useMemo(() => {
    const preview = previewResetEvent(Date.now(), window.location.search);
    return preview ? [preview, ...liveResetEvents] : liveResetEvents;
  }, [liveResetEvents]);

  useEffect(() => {
    if (previewPrefs(window.location.search)) {
      document.documentElement.dataset.preview = "";
    }
  }, []);

  // Recording scripts drive the real window over `usagebar://demo`. The
  // listener is inert unless the process was started in demo mode.
  useEffect(() => {
    let active = true;
    void demoMode().then((on) => {
      if (!active) return;
      setDemo(on);
      if (on) document.documentElement.dataset.demo = "";
    });
    if (!inTauri()) {
      return () => {
        active = false;
      };
    }
    const unlisten = listen<{ action: string; amount?: number | null }>("usagebar://demo", (event) => {
      const { action, amount } = event.payload;
      if (action === "settings") setSettingsOpen(true);
      else if (action === "close-settings") setSettingsOpen(false);
      else if (action === "scroll" || action === "scroll-top") {
        const scroller = document.querySelector(".content-scroll");
        if (!(scroller instanceof HTMLElement)) return;
        if (action === "scroll-top") scroller.scrollTo({ top: 0, behavior: "smooth" });
        else scroller.scrollBy({ top: amount ?? 200, behavior: "smooth" });
      }
    });
    return () => {
      active = false;
      delete document.documentElement.dataset.demo;
      void unlisten.then((off) => off());
    };
  }, []);

  // First run shows the walkthrough; the tray's Setup Guide item reopens it.
  useEffect(() => {
    if (!inTauri()) {
      setOnboarding(new URLSearchParams(window.location.search).has("onboarding"));
      return;
    }
    let active = true;
    // A prefs event can beat the initial read (the user toggles a meter in
    // the tray menu while the webview boots). The read is then older than
    // what is on screen, so it must not roll the toggle back.
    let heardEvent = false;
    void invoke<AppPrefs>("get_app_prefs")
      .then((next) => {
        if (!active) return;
        if (!heardEvent) setPrefs(normalizePrefs(next));
        setOnboarding(!next.onboardingComplete);
      })
      .catch(() => undefined);
    const unlistenOnboarding = listen("usagebar://show-onboarding", () => {
      if (active) setOnboarding(true);
    });
    const unlistenPrefs = listen<AppPrefs>("usagebar://prefs", (event) => {
      if (!active) return;
      heardEvent = true;
      setPrefs(normalizePrefs(event.payload));
    });
    return () => {
      active = false;
      void unlistenOnboarding.then((off) => off());
      void unlistenPrefs.then((off) => off());
    };
  }, []);

  const closeOnboarding = useCallback(() => {
    setOnboarding(false);
    if (inTauri()) void invoke("complete_onboarding");
  }, []);
  useEffect(() => {
    void notifyFreshResets(resetEvents, Date.now(), { enabled: prefs.resetAlerts });
  }, [resetEvents, prefs.resetAlerts]);
  useEffect(() => {
    if (!inTauri()) return;
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
      // A control that already used the key (an IME closing its candidate
      // list, a field clearing itself) owns this press. A held key repeats:
      // one press must close one layer, not every layer and then the window.
      if (event.defaultPrevented || event.isComposing || event.repeat) return;
      if (settingsOpen) {
        setSettingsOpen(false);
        return;
      }
      if (onboarding) {
        closeOnboarding();
        return;
      }
      if (inTauri()) void invoke("hide_window");
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [settingsOpen, onboarding, closeOnboarding]);

  const showCodex = isVisible(prefs, "codex");
  const connected = state.connection === "connected";
  const visibleBuckets = PROVIDER_CATALOG.flatMap((provider) =>
    isVisible(prefs, provider.id) ? meters[provider.id].buckets : [],
  );
  const mostCooked = visibleBuckets.reduce<(typeof visibleBuckets)[number] | undefined>(
    (lowest, bucket) => !lowest || bucket.remainingPercent < lowest.remainingPercent ? bucket : lowest,
    undefined,
  );
  const anyRefreshing = PROVIDER_CATALOG.some((provider) => meters[provider.id].refreshing);
  // Any shown tool still on its first read means the header is waiting, not
  // offline. Codex used to be the only one asked, so with Codex hidden the
  // header said "Offline" while every other meter was still connecting.
  const anyConnecting = PROVIDER_CATALOG.some(
    (provider) => isVisible(prefs, provider.id) && meters[provider.id].state.connection === "starting",
  );

  // Every provider renders nothing while its login is absent (cli_not_found)
  // or still starting, so a Mac with no tools — or every meter hidden — would
  // show a blank card between the header and footer. Detect that exact case
  // and say what to do next instead.
  const codexRenders = showCodex && state.connection !== "cli_not_found";
  const othersRender = SECTION_PROVIDERS.some(
    (provider) => isVisible(prefs, provider.id) && isPresent(meters[provider.id].state),
  );
  const nothingEnabled = PROVIDER_CATALOG.every((provider) => !isVisible(prefs, provider.id));

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
        aria-label={`Hide UsageBar to the ${trayName}`}
        onClick={() => {
          if (inTauri()) void invoke("hide_window");
        }}
      >
        <ChevronUp size={15} strokeWidth={2.5} aria-hidden="true" />
        <span className="dismiss-label">Hide to {trayName}</span>
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
          if (inTauri()) {
            void getCurrentWindow().startDragging();
          }
        }}
      >
        <div className="header-toolbar" data-tauri-drag-region>
          <div className="brand-row" data-tauri-drag-region>
            <MeterMark />
            <div data-tauri-drag-region>
              <h1 data-tauri-drag-region>UsageBar</h1>
              <p data-tauri-drag-region>{demo ? "Demo data" : "Usage meter"}</p>
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
                <HeaderRenewal bucket={mostCooked} />
              </div>
            </div>
          ) : (
            <div className="status-summary status-summary--connection" title="UsageBar status" data-tauri-drag-region>
              <span className="connection-orb" aria-hidden="true" />
              <div className="status-copy" data-tauri-drag-region>
                <span className="status-eyebrow" data-tauri-drag-region>Status</span>
                <strong data-tauri-drag-region>{anyConnecting ? "Connecting…" : "Offline"}</strong>
              </div>
            </div>
          )}
        </div>
      </header>

      <div className="content-scroll">
        {showCodex ? <WithNow>{(now) => <ResetAlert now={now} events={resetEvents} />}</WithNow> : null}
        {showCodex && !connected ? (
          <ConnectionStateView
            state={state}
            onRetry={() => void refresh()}
            compact={othersRender}
          />
        ) : showCodex && buckets.length ? (
          <>
            <WithNow>
              {(now) => (
                <>
                  <div className="section-label provider-label codex-label">
                    <span>Codex</span>
                    <Freshness updatedAt={state.updatedAt} now={now} />
                  </div>
                  <div className="quota-list">
                    {buckets.map((bucket) => (
                      <QuotaSection
                        key={bucket.id}
                        provider="codex"
                        bucket={bucket}
                        now={now}
                        dense={prefs.denseLayout}
                        live={isLiveReading(state.connection, state.updatedAt, now)}
                      />
                    ))}
                  </div>
                </>
              )}
            </WithNow>
            {resetCredits.availableCount > 0 ? (
              <div className="reset-credit" title="Reset credits on your Codex account. Each one renews a window on demand.">
                <Ticket size={15} aria-hidden="true" /><strong>{resetCredits.availableCount}</strong> reset credit{resetCredits.availableCount === 1 ? "" : "s"} on your account
              </div>
            ) : null}
            {usage ? <UsageDetails usage={usage} /> : null}
          </>
        ) : showCodex ? (
          <div className="state-panel glass-tile" role="status">
            <ShieldCheck size={22} aria-hidden="true" />
            <h2>No rate-limit buckets returned</h2>
            <p>Codex is connected, but this account did not report a quota window.</p>
            <button className="secondary-button" onClick={() => void refresh()}>Refresh</button>
          </div>
        ) : null}
        {showCodex && (connected || othersRender) ? (
          <WithNow>{(now) => <TiboWatch now={now} events={resetEvents} />}</WithNow>
        ) : null}
        {SECTION_PROVIDERS.map(({ id, label, icon: Icon, signedOutHint }) =>
          isVisible(prefs, id) ? (
            <ProviderSection
              key={id}
              id={id}
              label={label}
              icon={Icon ? <Icon size={14} aria-hidden="true" /> : null}
              meter={meters[id]}
              dense={prefs.denseLayout}
              signedOutHint={signedOutHint ?? ""}
            />
          ) : null,
        )}
        {!codexRenders && !othersRender ? (
          <div className="state-panel glass-tile" role="status">
            <PlugZap size={22} strokeWidth={1.8} aria-hidden="true" />
            <h2>{nothingEnabled ? "Every meter is hidden" : "No signed-in tools found"}</h2>
            <p>
              {nothingEnabled
                ? "Turn a tool back on in Settings to see its quota here."
                : `UsageBar reads the logins Codex, Claude Code, Cursor, OpenCode Go, Devin, Antigravity, and Gemini CLI already keep on this ${deviceName}. Sign in to one of them, then refresh.`}
            </p>
            <button className="secondary-button" onClick={() => setOnboarding(true)}>
              Open setup guide
            </button>
          </div>
        ) : null}
        <MissingProviders
          items={missingProviders(
            prefs,
            Object.fromEntries(PROVIDER_CATALOG.map(({ id }) => [id, meters[id].state])),
          )}
        />
      </div>

      <footer className="app-footer">
        <button className="footer-action" onClick={() => setSettingsOpen(true)}>
          <Settings2 size={16} aria-hidden="true" /> Settings
        </button>
        <button
          className="icon-button"
          onClick={() => {
            for (const provider of PROVIDER_CATALOG) {
              if (isVisible(prefs, provider.id)) void meters[provider.id].refresh();
            }
          }}
          disabled={anyRefreshing}
          aria-label="Refresh usage data"
          title="Refresh"
        >
          <RefreshCw size={17} className={anyRefreshing ? "spinning" : ""} aria-hidden="true" />
        </button>
        <button className="icon-button danger-hover" onClick={() => {
            if (inTauri()) void invoke("quit_app");
          }} aria-label="Quit UsageBar" title="Quit">
          <Power size={17} aria-hidden="true" />
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
          providers={PROVIDER_CATALOG.flatMap((provider) => {
            const meter = meters[provider.id];
            const status = onboardingStatus(provider, meter.state);
            return status
              ? [{ label: provider.label, ...status, onRetry: () => void meter.refresh() }]
              : [];
          })}
        />
      ) : null}
    </main>
  );
}
