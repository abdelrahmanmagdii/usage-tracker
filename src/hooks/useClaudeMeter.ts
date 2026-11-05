import type { CodexBackendState } from "../types/codex";
import { providerChannels, type ProviderId } from "../lib/providers";
import { useProviderMeter, type ProviderMeter } from "./useProviderMeter";

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

function devinPreview(): CodexBackendState {
  const now = Date.now() / 1000;
  return {
    connection: "connected",
    updatedAt: Math.floor(Date.now() / 1_000),
    account: { type: "api", planType: "Pro" },
    rateLimits: {
      rateLimitsByLimitId: {
        daily: {
          limitId: "daily",
          windowLabel: "Daily",
          limitName: "Devin plan",
          primary: { usedPercent: 28, windowDurationMins: 1_440, resetsAt: now + 11 * 3_600 },
        },
        weekly: {
          limitId: "weekly",
          windowLabel: "Weekly",
          limitName: "Devin plan",
          secondary: { usedPercent: 55, windowDurationMins: 10_080, resetsAt: now + 4 * 86_400 },
        },
      },
    },
  };
}

function antigravityPreview(): CodexBackendState {
  const now = Date.now() / 1000;
  return {
    connection: "connected",
    updatedAt: Math.floor(Date.now() / 1_000),
    account: { type: "oauth", planType: "Antigravity" },
    rateLimits: {
      rateLimitsByLimitId: {
        "gemini-5h": {
          limitId: "gemini-5h",
          windowLabel: "Gemini 5-hour",
          limitName: "Gemini",
          primary: { usedPercent: 15, windowDurationMins: 300, resetsAt: now + 3.1 * 3_600 },
        },
        "gemini-weekly": {
          limitId: "gemini-weekly",
          windowLabel: "Gemini weekly",
          limitName: "Gemini",
          secondary: { usedPercent: 42, windowDurationMins: 10_080, resetsAt: now + 3 * 86_400 },
        },
        "3p-5h": {
          limitId: "3p-5h",
          windowLabel: "Claude and GPT 5-hour",
          limitName: "Claude and GPT",
          primary: { usedPercent: 8, windowDurationMins: 300, resetsAt: now + 4.2 * 3_600 },
        },
        "3p-weekly": {
          limitId: "3p-weekly",
          windowLabel: "Claude and GPT weekly",
          limitName: "Claude and GPT",
          secondary: { usedPercent: 27, windowDurationMins: 10_080, resetsAt: now + 5 * 86_400 },
        },
        "argon-weekly": {
          limitId: "argon-weekly",
          windowLabel: "Gemini 4 Argon weekly",
          limitName: "Gemini 4 Argon",
          secondary: { usedPercent: 4, windowDurationMins: 10_080, resetsAt: now + 6 * 86_400 },
        },
      },
    },
  };
}
function geminiPreview(): CodexBackendState {
  const now = Date.now() / 1000;
  return {
    connection: "connected",
    updatedAt: Math.floor(Date.now() / 1_000),
    account: { type: "oauth", planType: "Free" },
    rateLimits: {
      rateLimitsByLimitId: {
        "gemini-pro": {
          limitId: "gemini-pro",
          windowLabel: "Pro models",
          limitName: "Gemini Pro",
          primary: { usedPercent: 38, resetsAt: now + 11 * 3_600 },
        },
        "gemini-flash": {
          limitId: "gemini-flash",
          windowLabel: "Flash models",
          limitName: "Gemini Flash",
          secondary: { usedPercent: 12, resetsAt: now + 11 * 3_600 },
        },
      },
    },
  };
}

export type MeterId = Exclude<ProviderId, "codex">;

// Module-level so every hook call gets the same channel names and preview
// function: useProviderMeter re-subscribes when either identity changes.
const PREVIEWS: Record<MeterId, () => CodexBackendState> = {
  claude: claudePreview,
  cursor: cursorPreview,
  opencode: opencodePreview,
  devin: devinPreview,
  antigravity: antigravityPreview,
  gemini: geminiPreview,
};

const CHANNELS = Object.fromEntries(
  Object.keys(PREVIEWS).map((id) => [id, providerChannels(id as MeterId)]),
) as Record<MeterId, ReturnType<typeof providerChannels>>;

function useMeter(provider: MeterId): ProviderMeter {
  return useProviderMeter({ provider, ...CHANNELS[provider], preview: PREVIEWS[provider] });
}

export function useClaudeMeter() {
  return useMeter("claude");
}

export function useCursorMeter() {
  return useMeter("cursor");
}

export function useOpenCodeMeter() {
  return useMeter("opencode");
}

export function useDevinMeter() {
  return useMeter("devin");
}

export function useAntigravityMeter() {
  return useMeter("antigravity");
}

export function useGeminiMeter() {
  return useMeter("gemini");
}
