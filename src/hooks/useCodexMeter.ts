import { useMemo } from "react";
import type { CodexBackendState } from "../types/codex";
import { extractResetCredits, extractUsage } from "../lib/rateLimits";
import { providerChannels } from "../lib/providers";
import { useProviderMeter } from "./useProviderMeter";

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

// Unlike the other tools, Codex outside Tauri explains itself instead of
// hiding: it is the popover's lead section, and a blank one looks broken.
const browserState: CodexBackendState = {
  connection: "error",
  diagnostic: "Open this preview through Tauri to connect to the local Codex App Server.",
};

const channels = providerChannels("codex");

export function useCodexMeter({ observeHistory = true }: { observeHistory?: boolean } = {}) {
  const meter = useProviderMeter({
    provider: "codex",
    ...channels,
    preview: previewState,
    fallbackState: browserState,
    observeHistory,
  });
  return useMemo(
    () => ({
      ...meter,
      resetCredits: extractResetCredits(meter.state.rateLimits),
      usage: extractUsage(meter.state.usage),
    }),
    [meter],
  );
}

export type CodexMeter = ReturnType<typeof useCodexMeter>;
