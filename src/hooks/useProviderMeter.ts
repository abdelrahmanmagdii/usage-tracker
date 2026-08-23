import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { CodexBackendState } from "../types/codex";
import { extractRateLimitBuckets } from "../lib/rateLimits";

const initialState: CodexBackendState = { connection: "starting" };

export function useProviderMeter({
  getCommand,
  refreshCommand,
  event,
  preview,
}: {
  getCommand: string;
  refreshCommand: string;
  event: string;
  preview: () => CodexBackendState;
}) {
  const [state, setState] = useState<CodexBackendState>(initialState);
  const [refreshing, setRefreshing] = useState(false);

  const refresh = useCallback(async () => {
    setRefreshing(true);
    try {
      const next = await invoke<CodexBackendState>(refreshCommand);
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
  }, [refreshCommand]);

  useEffect(() => {
    let active = true;
    if (!("__TAURI_INTERNALS__" in window)) {
      if (import.meta.env.DEV && new URLSearchParams(window.location.search).has("preview")) {
        setState(preview());
      } else {
        setState({ connection: "cli_not_found" });
      }
      return () => {
        active = false;
      };
    }
    void invoke<CodexBackendState>(getCommand)
      .then((next) => active && setState(next))
      .catch(() => undefined);
    const unlistenPromise = listen<CodexBackendState>(event, (payload) => {
      if (active) setState(payload.payload);
    });
    return () => {
      active = false;
      void unlistenPromise.then((off) => off());
    };
  }, [event, getCommand, preview]);

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
