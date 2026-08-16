import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { CodexBackendState } from "../types/codex";
import { extractRateLimitBuckets } from "../lib/rateLimits";

const initialState: CodexBackendState = { connection: "starting" };

function previewState(): CodexBackendState {
  const now = Date.now() / 1000;
  return {
    connection: "connected",
    updatedAt: Math.floor(Date.now() / 1_000), // the backend reports seconds
    account: { type: "oauth", planType: "max" },
    rateLimits: {
      rateLimitsByLimitId: {
        session: {
          limitId: "session",
          primary: { usedPercent: 5, windowDurationMins: 300, resetsAt: now + 4.6 * 3_600 },
        },
        "weekly-all": {
          limitId: "weekly-all",
          limitName: "All models",
          secondary: { usedPercent: 41, windowDurationMins: 10_080, resetsAt: now + 1.2 * 86_400 },
        },
        "weekly-scoped-fable": {
          limitId: "weekly-scoped-fable",
          windowLabel: "Fable",
          limitName: "Weekly limit",
          secondary: { usedPercent: 63, windowDurationMins: 10_080, resetsAt: now + 1.2 * 86_400 },
        },
      },
    },
  };
}

/** Mirrors useCodexMeter for the Claude Code provider; no local history yet. */
export function useClaudeMeter() {
  const [state, setState] = useState<CodexBackendState>(initialState);
  const [refreshing, setRefreshing] = useState(false);

  const refresh = useCallback(async () => {
    setRefreshing(true);
    try {
      const next = await invoke<CodexBackendState>("refresh_claude");
      setState(next);
    } catch (error) {
      setState((current) => ({
        ...current,
        connection: "error",
        diagnostic: error instanceof Error ? error.message : String(error),
      }));
    } finally {
      setRefreshing(false);
    }
  }, []);

  useEffect(() => {
    let active = true;
    if (!("__TAURI_INTERNALS__" in window)) {
      if (import.meta.env.DEV && new URLSearchParams(window.location.search).has("preview")) {
        setState(previewState());
      } else {
        setState({ connection: "cli_not_found" });
      }
      return () => {
        active = false;
      };
    }
    void invoke<CodexBackendState>("get_claude_state")
      .then((next) => active && setState(next))
      .catch(() => undefined);
    const unlistenPromise = listen<CodexBackendState>("claude://state", (event) => {
      if (active) setState(event.payload);
    });
    return () => {
      active = false;
      void unlistenPromise.then((unlisten) => unlisten());
    };
  }, []);

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
