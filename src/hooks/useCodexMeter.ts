import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { CodexBackendState } from "../types/codex";
import {
  extractRateLimitBuckets,
  extractResetCredits,
  extractUsage,
} from "../lib/rateLimits";
import { observeBuckets } from "../lib/history";

const initialState: CodexBackendState = { connection: "starting" };

function previewState(): CodexBackendState {
  const now = Date.now() / 1000;
  return {
    connection: "connected",
    updatedAt: Math.floor(Date.now() / 1_000), // the backend reports seconds
    rateLimits: {
      rateLimitsByLimitId: {
        codex: {
          limitId: "codex",
          limitName: "Codex subscription",
          primary: { usedPercent: 18, windowDurationMins: 300, resetsAt: now + 3.2 * 3_600 },
          secondary: { usedPercent: 36, windowDurationMins: 10_080, resetsAt: now + 4.8 * 86_400 },
        },
      },
      rateLimitResetCredits: { availableCount: 1 },
    },
    usage: {
      summary: { lifetimeTokens: 2_470_000, peakDailyTokens: 184_000, currentStreakDays: 12, longestStreakDays: 21 },
      dailyUsageBuckets: [41_000, 72_000, 56_000, 94_000, 132_000, 81_000, 118_000].map((tokens, index) => ({
        startDate: new Date(Date.now() - (6 - index) * 86_400_000).toISOString(),
        tokens,
      })),
    },
  };
}

export function useCodexMeter({ observeHistory = true }: { observeHistory?: boolean } = {}) {
  const [state, setState] = useState<CodexBackendState>(initialState);
  const [refreshing, setRefreshing] = useState(false);
  const observedSignature = useRef("");

  const applyState = useCallback((next: CodexBackendState) => {
    setState(next);
    const buckets = extractRateLimitBuckets(next.rateLimits);
    const signature = buckets
      .map((bucket) => `${bucket.id}:${bucket.usedPercent}:${bucket.resetsAt ?? ""}`)
      .join("|");
    if (observeHistory && signature && signature !== observedSignature.current) {
      observeBuckets(buckets);
      observedSignature.current = signature;
    }
  }, [observeHistory]);

  const refresh = useCallback(async () => {
    setRefreshing(true);
    try {
      const next = await invoke<CodexBackendState>("refresh_codex");
      applyState(next);
    } catch (error) {
      setState((current) => ({
        ...current,
        connection: "error",
        diagnostic: error instanceof Error ? error.message : String(error),
      }));
    } finally {
      setRefreshing(false);
    }
  }, [applyState]);

  useEffect(() => {
    let active = true;
    if (!("__TAURI_INTERNALS__" in window)) {
      if (import.meta.env.DEV && new URLSearchParams(window.location.search).has("preview")) {
        applyState(previewState());
        return () => {
          active = false;
        };
      }
      setState({
        connection: "error",
        diagnostic: "Open this preview through Tauri to connect to the local Codex App Server.",
      });
      return () => {
        active = false;
      };
    }
    void invoke<CodexBackendState>("get_codex_state")
      .then((next) => active && applyState(next))
      .catch(() => undefined);
    const unlistenPromise = listen<CodexBackendState>("codex://state", (event) => {
      if (active) applyState(event.payload);
    });
    return () => {
      active = false;
      void unlistenPromise.then((unlisten) => unlisten());
    };
  }, [applyState, refresh]);

  return useMemo(
    () => ({
      state,
      buckets: extractRateLimitBuckets(state.rateLimits),
      resetCredits: extractResetCredits(state.rateLimits),
      usage: extractUsage(state.usage),
      refreshing,
      refresh,
    }),
    [state, refreshing, refresh],
  );
}
