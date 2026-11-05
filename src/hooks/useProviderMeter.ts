import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { CodexBackendState } from "../types/codex";
import { extractRateLimitBuckets } from "../lib/rateLimits";
import { observeBuckets } from "../lib/history";
import { stateAfterRefreshFailure } from "../lib/providers";
import { isDevPreview } from "../lib/preview";

const initialState: CodexBackendState = { connection: "starting" };
/** Outside Tauri there is no backend: the tool reads as absent and hides. */
const absentState: CodexBackendState = { connection: "cli_not_found" };

/**
 * True when `next` is an older reading than the one on screen. Both sides
 * need a timestamp: a snapshot without one (a connection change, a missing
 * login) always applies, and so does an equal one, which is how the backend
 * reports "same data, new connection state".
 */
export function isOlderSnapshot(
  next: CodexBackendState,
  shownUpdatedAt: number | null | undefined,
): boolean {
  return typeof next.updatedAt === "number" && typeof shownUpdatedAt === "number" && next.updatedAt < shownUpdatedAt;
}

export function useProviderMeter({
  provider,
  getCommand,
  refreshCommand,
  event,
  preview,
  fallbackState = absentState,
  observeHistory = true,
}: {
  provider: string;
  getCommand: string;
  refreshCommand: string;
  event: string;
  /** Sample data for `?preview` in Vite dev. Must be a stable reference. */
  preview: () => CodexBackendState;
  /** State outside Tauri when not previewing. Must be a stable reference. */
  fallbackState?: CodexBackendState;
  /** Off for secondary windows (the Windows widget) so one reading is not
   * recorded twice in the shared history. */
  observeHistory?: boolean;
}) {
  const [state, setState] = useState<CodexBackendState>(initialState);
  const [refreshing, setRefreshing] = useState(false);
  const observedSignature = useRef("");
  // Read synchronously by applyState: the initial `get_*_state` and the
  // first `*://state` event race at startup, and a state updater would see
  // the winner too late to keep its stale twin out of the history.
  const shownUpdatedAt = useRef<number | null | undefined>(undefined);

  // History (sparklines + surprise-reset detection) records only on real
  // state changes, not on failures or repeated identical payloads.
  const applyState = useCallback((next: CodexBackendState) => {
    // The initial invoke can resolve after an event that already carried a
    // newer reading; letting it land would roll the meter back until the
    // next poll.
    if (isOlderSnapshot(next, shownUpdatedAt.current)) return;
    shownUpdatedAt.current = next.updatedAt;
    setState(next);
    const buckets = extractRateLimitBuckets(next.rateLimits);
    const signature = buckets
      .map((bucket) => `${bucket.id}:${bucket.usedPercent}:${bucket.resetsAt ?? ""}`)
      .join("|");
    if (observeHistory && signature && signature !== observedSignature.current) {
      observeBuckets(provider, buckets);
      observedSignature.current = signature;
    }
  }, [provider, observeHistory]);

  const refresh = useCallback(async () => {
    setRefreshing(true);
    try {
      const next = await invoke<CodexBackendState>(refreshCommand);
      applyState(next);
    } catch (error) {
      setState((current) => stateAfterRefreshFailure(current, error));
    } finally {
      setRefreshing(false);
    }
  }, [refreshCommand, applyState]);

  useEffect(() => {
    let active = true;
    if (!("__TAURI_INTERNALS__" in window)) {
      if (isDevPreview(window.location.search)) {
        applyState(preview());
      } else {
        setState(fallbackState);
      }
      return () => {
        active = false;
      };
    }
    void invoke<CodexBackendState>(getCommand)
      .then((next) => {
        if (active) applyState(next);
      })
      .catch(() => undefined);
    const unlistenPromise = listen<CodexBackendState>(event, (payload) => {
      if (active) applyState(payload.payload);
    });
    return () => {
      active = false;
      void unlistenPromise.then((off) => off());
    };
  }, [event, getCommand, preview, fallbackState, applyState]);

  return useMemo(
    () => ({
      state,
      buckets: extractRateLimitBuckets(state.rateLimits),
      refreshing,
      refresh,
    }),
    [state, refreshing, refresh],
  );
}

export type ProviderMeter = ReturnType<typeof useProviderMeter>;
