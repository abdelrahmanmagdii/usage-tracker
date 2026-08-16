import { useEffect, useRef, useState } from "react";
import type { ResetEvent } from "../../types/codex";
import { CombinedResetEventProvider } from "./provider";

const REFRESH_MS = 5 * 60 * 1000;
const SHARED_EVENTS_KEY = "codex-meter.shared-reset-events.v1";

function readCachedEvents(): ResetEvent[] {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(SHARED_EVENTS_KEY) ?? "[]");
    return Array.isArray(value) ? (value as ResetEvent[]) : [];
  } catch {
    return [];
  }
}

/**
 * Announced and locally detected resets, polled often enough that an
 * "arriving within the hour" announcement still leaves time to spend the
 * current window.
 */
export function useResetEvents(): ResetEvent[] {
  const [events, setEvents] = useState<ResetEvent[]>(readCachedEvents);
  const providerRef = useRef<CombinedResetEventProvider | null>(null);
  providerRef.current ??= new CombinedResetEventProvider();

  useEffect(() => {
    let active = true;
    const load = () =>
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
    load();
    const interval = window.setInterval(load, REFRESH_MS);
    return () => {
      active = false;
      window.clearInterval(interval);
    };
  }, []);

  return events;
}
