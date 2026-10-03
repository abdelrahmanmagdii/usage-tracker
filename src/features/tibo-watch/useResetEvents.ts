import { useEffect, useRef, useState } from "react";
import type { ResetEvent } from "../../types/codex";
import { CombinedResetEventProvider } from "./provider";

const REFRESH_MS = 5 * 60 * 1000;
// Timers freeze while the Mac sleeps, so a short tick checks wall-clock time
// and catches up within a minute of waking instead of up to five.
const TICK_MS = 60 * 1000;
// Focus and wake events can arrive in bursts; one fetch covers them all.
const MIN_GAP_MS = 30 * 1000;
const SHARED_EVENTS_KEY = "codex-meter.shared-reset-events.v1";

function readCachedEvents(): ResetEvent[] {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(SHARED_EVENTS_KEY) ?? "[]");
    return Array.isArray(value) ? (value as ResetEvent[]) : [];
  } catch {
    return [];
  }
}

/** Whether a poll is due, given the last load and whether this is a nudge (focus/wake). */
export function shouldLoad(lastLoadMs: number, nowMs: number, nudge: boolean): boolean {
  const elapsed = nowMs - lastLoadMs;
  return nudge ? elapsed >= MIN_GAP_MS : elapsed >= REFRESH_MS;
}

/**
 * Announced and locally detected resets, polled often enough that an
 * "arriving within the hour" announcement still leaves time to spend the
 * current window, and again as soon as the Mac wakes or the popover opens.
 */
export function useResetEvents(): ResetEvent[] {
  const [events, setEvents] = useState<ResetEvent[]>(readCachedEvents);
  const providerRef = useRef<CombinedResetEventProvider | null>(null);
  providerRef.current ??= new CombinedResetEventProvider();

  useEffect(() => {
    let active = true;
    let lastLoad = 0;
    const load = (nudge = false) => {
      const now = Date.now();
      if (lastLoad && !shouldLoad(lastLoad, now, nudge)) return;
      lastLoad = now;
      void providerRef.current
        ?.listEvents()
        .then((next) => {
          if (!active) return;
          setEvents(next);
          try {
            localStorage.setItem(SHARED_EVENTS_KEY, JSON.stringify(next));
          } catch {
            /* Cache is an optimization; live state is already updated. */
          }
        })
        .catch(() => undefined);
    };
    const nudge = () => load(true);
    const onVisibility = () => {
      if (document.visibilityState === "visible") nudge();
    };
    load();
    const interval = window.setInterval(() => load(), TICK_MS);
    window.addEventListener("focus", nudge);
    window.addEventListener("online", nudge);
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      active = false;
      window.clearInterval(interval);
      window.removeEventListener("focus", nudge);
      window.removeEventListener("online", nudge);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, []);

  return events;
}
