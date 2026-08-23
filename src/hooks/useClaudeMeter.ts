import { useCallback } from "react";
import type { CodexBackendState } from "../types/codex";
import { useProviderMeter } from "./useProviderMeter";

function claudePreview(): CodexBackendState {
  const now = Date.now() / 1000;
  return {
    connection: "connected",
    updatedAt: Math.floor(Date.now() / 1_000),
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

function cursorPreview(): CodexBackendState {
  const now = Date.now() / 1000;
  return {
    connection: "connected",
    updatedAt: Math.floor(Date.now() / 1_000),
    account: { type: "oauth", planType: "pro" },
    rateLimits: {
      rateLimitsByLimitId: {
        plan: {
          limitId: "plan",
          windowLabel: "Monthly",
          limitName: "Cursor plan",
          primary: { usedPercent: 41, windowDurationMins: 43_200, resetsAt: now + 18 * 86_400 },
        },
        auto: {
          limitId: "auto",
          windowLabel: "Auto",
          limitName: "Auto + Composer",
          secondary: { usedPercent: 12, windowDurationMins: 43_200, resetsAt: now + 18 * 86_400 },
        },
      },
    },
  };
}

function opencodePreview(): CodexBackendState {
  const now = Date.now() / 1000;
  return {
    connection: "connected",
    updatedAt: Math.floor(Date.now() / 1_000),
    account: { type: "api", planType: "go" },
    rateLimits: {
      rateLimitsByLimitId: {
        rolling: {
          limitId: "rolling",
          primary: { usedPercent: 18, windowDurationMins: 300, resetsAt: now + 3.4 * 3_600 },
        },
        weekly: {
          limitId: "weekly",
          secondary: { usedPercent: 27, windowDurationMins: 10_080, resetsAt: now + 4 * 86_400 },
        },
        monthly: {
          limitId: "monthly",
          windowLabel: "Monthly",
          secondary: { usedPercent: 11, windowDurationMins: 43_200, resetsAt: now + 20 * 86_400 },
        },
      },
    },
  };
}

export function useClaudeMeter() {
  const preview = useCallback(claudePreview, []);
  return useProviderMeter({
    getCommand: "get_claude_state",
    refreshCommand: "refresh_claude",
    event: "claude://state",
    preview,
  });
}

export function useCursorMeter() {
  const preview = useCallback(cursorPreview, []);
  return useProviderMeter({
    getCommand: "get_cursor_state",
    refreshCommand: "refresh_cursor",
    event: "cursor://state",
    preview,
  });
}

export function useOpenCodeMeter() {
  const preview = useCallback(opencodePreview, []);
  return useProviderMeter({
    getCommand: "get_opencode_state",
    refreshCommand: "refresh_opencode",
    event: "opencode://state",
    preview,
  });
}
