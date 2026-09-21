import type { CodexBackendState } from "../types/codex";

export const PROVIDER_IDS = ["codex", "claude", "cursor", "opencode", "devin", "antigravity"] as const;
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
  { id: "devin", label: "Devin", accent: "devin" },
  { id: "antigravity", label: "Antigravity", accent: "antigravity" },
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

export function hasShownUsage(state: CodexBackendState): boolean {
  return state.updatedAt != null || state.rateLimits != null || state.usage != null;
}

/** A tool the user does not have must stay hidden. A refresh failure is not a new meter. */
export function stateAfterRefreshFailure(current: CodexBackendState, error: unknown): CodexBackendState {
  if (current.connection === "cli_not_found" || !hasShownUsage(current)) {
    return { connection: "cli_not_found" };
  }
  return {
    ...current,
    connection: "error",
    diagnostic: error instanceof Error ? error.message : String(error),
  };
}

export const PROVIDER_SETUP: Record<ProviderId, string> = {
  codex: "Install the Codex CLI, sign in, and leave codex on your PATH.",
  claude: "Run claude once and sign in. The Claude desktop app uses a different login.",
  cursor: "Sign in through the Cursor app on this Mac.",
  opencode: "In OpenCode, run /connect and choose OpenCode Go.",
  devin: "Run devin auth login so the CLI stores a login on this Mac.",
  antigravity: "Open the Antigravity app and sign in. UsageBar reads quota from the running app.",
};

export type MissingProvider = {
  id: ProviderId;
  label: string;
  detail: string;
};

/** Providers the popover is not showing, with the step that brings each one back. */
export function missingProviders(
  prefs: AppPrefs,
  states: Partial<Record<ProviderId, CodexBackendState>>,
): MissingProvider[] {
  return PROVIDER_CATALOG.flatMap((provider) => {
    if (!isVisible(prefs, provider.id)) {
      return [{ id: provider.id, label: provider.label, detail: "Turn this meter on in Settings." }];
    }
    const connection = states[provider.id]?.connection;
    if (connection === "cli_not_found") {
      return [{ id: provider.id, label: provider.label, detail: PROVIDER_SETUP[provider.id] }];
    }
    return [];
  });
}
