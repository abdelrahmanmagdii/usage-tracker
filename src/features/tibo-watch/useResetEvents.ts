import { useEffect, useRef, useState } from "react";
import { emit, listen } from "@tauri-apps/api/event";
import type { ResetEvent } from "../../types/codex";
import { CombinedResetEventProvider } from "./provider";

const REFRESH_MS = 5 * 60 * 1000;
const SHARED_EVENTS_KEY = "codex-meter.shared-reset-events.v1";

function readSharedEvents(): ResetEvent[] {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(SHARED_EVENTS_KEY) ?? "[]");
    return Array.isArray(value) ? value as ResetEvent[] : [];
  } catch {
    return [];
  }
}

export function useResetEvents({ owner = true }: { owner?: boolean } = {}): ResetEvent[] {
  const [events, setEvents] = useState<ResetEvent[]>(readSharedEvents);
  const providerRef = useRef<CombinedResetEventProvider | null>(null);
  providerRef.current ??= new CombinedResetEventProvider();

  useEffect(() => {
    let active = true;
    if (!owner) {
      if (!("__TAURI_INTERNALS__" in window)) {
        return () => {
          active = false;
        };
      }
      const unlistenPromise = listen<ResetEvent[]>("codex://reset-events", (event) => {
        if (active) setEvents(event.payload);
      });
      return () => {
        active = false;
        void unlistenPromise.then((unlisten) => unlisten());
      };
    }
    const load = () =>
      void providerRef.current
        ?.listEvents()
        .then((next) => {
          if (active) {
            setEvents(next);
            try {
              localStorage.setItem(SHARED_EVENTS_KEY, JSON.stringify(next));
            } catch {
              /* The global event still keeps live companion windows in sync. */
            }
            if ("__TAURI_INTERNALS__" in window) void emit("codex://reset-events", next);
          }
        })
        .catch(() => undefined);
    load();
    const interval = window.setInterval(load, REFRESH_MS);
    return () => {
      active = false;
      window.clearInterval(interval);
    };
  }, [owner]);

  return events;
}
