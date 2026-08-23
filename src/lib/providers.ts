import type { CodexBackendState } from "../types/codex";

export const PROVIDER_IDS = ["codex", "claude", "cursor", "opencode"] as const;
export type ProviderId = (typeof PROVIDER_IDS)[number];

export type ProviderPref = {
  visible?: boolean;
  trayWindow?: string;
};

export type AppPrefs = {
  usageAlerts: boolean;
  combinedTray: boolean;
  onboardingComplete: boolean;
  providers: Record<string, ProviderPref>;
};

export const AUTO_WINDOW = "auto";

export const DEFAULT_PREFS: AppPrefs = {
  usageAlerts: true,
  combinedTray: true,
  onboardingComplete: false,
  providers: {},
};

export function normalizePrefs(raw: Partial<AppPrefs> | null | undefined): AppPrefs {
  return {
    ...DEFAULT_PREFS,
    ...raw,
    providers: raw?.providers ?? {},
  };
}

export const PROVIDER_CATALOG: Array<{
  id: ProviderId;
  label: string;
  accent: string;
}> = [
  { id: "codex", label: "Codex", accent: "codex" },
  { id: "claude", label: "Claude Code", accent: "claude" },
  { id: "cursor", label: "Cursor", accent: "cursor" },
  { id: "opencode", label: "OpenCode Go", accent: "opencode" },
];

export function isVisible(prefs: AppPrefs, id: ProviderId): boolean {
  return prefs.providers[id]?.visible !== false;
}

export function trayWindow(prefs: AppPrefs, id: ProviderId): string {
  return prefs.providers[id]?.trayWindow ?? AUTO_WINDOW;
}

export function withVisible(prefs: AppPrefs, id: ProviderId, visible: boolean): AppPrefs {
  return {
    ...prefs,
    providers: {
      ...prefs.providers,
      [id]: { ...prefs.providers[id], visible },
    },
  };
}

export function withTrayWindow(prefs: AppPrefs, id: ProviderId, window: string): AppPrefs {
  return {
    ...prefs,
    providers: {
      ...prefs.providers,
      [id]: { ...prefs.providers[id], trayWindow: window },
    },
  };
}

export function isPresent(state: CodexBackendState): boolean {
  return state.connection !== "cli_not_found" && state.connection !== "starting";
}
