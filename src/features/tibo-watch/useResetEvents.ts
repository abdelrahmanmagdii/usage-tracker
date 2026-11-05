import { useEffect, useRef, useState } from "react";
import type { ResetEvent } from "../../types/codex";
import { demoMode } from "../../lib/demo";
import { CombinedResetEventProvider } from "./provider";

const REFRESH_MS = 5 * 60 * 1000;
// Timers freeze while the Mac sleeps, so a short tick checks wall-clock time
// and catches up within a minute of waking instead of up to five.
const TICK_MS = 60 * 1000;
// Focus and wake events can arrive in bursts; one fetch covers them all.
const MIN_GAP_MS = 30 * 1000;
const SHARED_EVENTS_KEY = "codex-meter.shared-reset-events.v1";

/**
 * Last poll's events, shown until the first live poll lands. Entries are
 * checked for the fields every consumer dereferences, so one corrupt entry
 * cannot crash the popover on launch. They are not re-normalized: that would
 * drop the `sample` flag that keeps sample events out of notifications.
 */
export function readCachedEvents(): ResetEvent[] {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(SHARED_EVENTS_KEY) ?? "[]");
    return Array.isArray(value)
      ? value.filter(
          (event): event is ResetEvent =>
            typeof event === "object" &&
            event !== null &&
            typeof (event as ResetEvent).id === "string" &&
            typeof (event as ResetEvent).announcedAt === "string" &&
            typeof (event as ResetEvent).source === "string",
        )
      : [];
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
    // A focus nudge can start a poll while a slow one is still out. Results
    // apply in the order the polls started, so an older response arriving
    // late never replaces a newer one.
    let requests = 0;
    let applied = 0;
    const load = (nudge = false) => {
      const now = Date.now();
      if (lastLoad && !shouldLoad(lastLoad, now, nudge)) return;
      lastLoad = now;
      const request = ++requests;
      void providerRef.current
        ?.listEvents()
        .then((next) => {
          if (!active || request < applied) return;
          applied = request;
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
    // Demo mode is sample quotas only: do not call the reset feed or Bluesky.
    let interval = 0;
    void demoMode().then((demo) => {
      if (!active || demo) return;
      load();
      interval = window.setInterval(() => load(), TICK_MS);
      window.addEventListener("focus", nudge);
      window.addEventListener("online", nudge);
      document.addEventListener("visibilitychange", onVisibility);
    });
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
