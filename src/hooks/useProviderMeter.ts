import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { CodexBackendState } from "../types/codex";
import { extractRateLimitBuckets } from "../lib/rateLimits";
import { observeBuckets } from "../lib/history";
import { stateAfterRefreshFailure } from "../lib/providers";

const initialState: CodexBackendState = { connection: "starting" };

export function useProviderMeter({
  provider,
  getCommand,
  refreshCommand,
  event,
  preview,
}: {
  provider: string;
  getCommand: string;
  refreshCommand: string;
  event: string;
  preview: () => CodexBackendState;
}) {
  const [state, setState] = useState<CodexBackendState>(initialState);
  const [refreshing, setRefreshing] = useState(false);
  const observedSignature = useRef("");

  // History (sparklines + surprise-reset detection) records only on real
  // state changes, not on failures or repeated identical payloads.
  const applyState = useCallback((next: CodexBackendState) => {
    setState(next);
    const buckets = extractRateLimitBuckets(next.rateLimits);
    const signature = buckets
      .map((bucket) => `${bucket.id}:${bucket.usedPercent}:${bucket.resetsAt ?? ""}`)
      .join("|");
    if (signature && signature !== observedSignature.current) {
      observeBuckets(provider, buckets);
      observedSignature.current = signature;
    }
  }, [provider]);

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
      if (import.meta.env.DEV && new URLSearchParams(window.location.search).has("preview")) {
        applyState(preview());
      } else {
        setState({ connection: "cli_not_found" });
      }
      return () => {
        active = false;
      };
    }
    void invoke<CodexBackendState>(getCommand)
      .then((next) => active && applyState(next))
      .catch(() => undefined);
    const unlistenPromise = listen<CodexBackendState>(event, (payload) => {
      if (active) applyState(payload.payload);
    });
    return () => {
      active = false;
      void unlistenPromise.then((off) => off());
    };
  }, [event, getCommand, preview, applyState]);

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
