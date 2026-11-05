import { Diamond, Gem, MousePointer2, Sparkle, Sparkles, Terminal, type LucideIcon } from "lucide-react";
import type { CodexBackendState } from "../types/codex";
import { deviceName, isWindows } from "./platform";

export const PROVIDER_IDS = ["codex", "claude", "cursor", "opencode", "devin", "antigravity", "gemini"] as const;
export type ProviderId = (typeof PROVIDER_IDS)[number];

export type ProviderPref = {
  visible?: boolean;
  trayWindow?: string;
};

export type AppPrefs = {
  usageAlerts: boolean;
  /** Notifications when Tibo announces a reset. */
  resetAlerts: boolean;
  /** One-row quota cards instead of the roomy default. */
  denseLayout: boolean;
  combinedTray: boolean;
  onboardingComplete: boolean;
  /** Percent-used levels that fire a usage alert (ascending). */
  usageAlertThresholds?: number[];
  providers: Record<string, ProviderPref>;
};

export const AUTO_WINDOW = "auto";

export const DEFAULT_PREFS: AppPrefs = {
  usageAlerts: true,
  resetAlerts: true,
  denseLayout: false,
  combinedTray: true,
  onboardingComplete: false,
  usageAlertThresholds: [80, 95],
  providers: {},
};

export function normalizePrefs(raw: Partial<AppPrefs> | null | undefined): AppPrefs {
  return {
    ...DEFAULT_PREFS,
    ...raw,
    providers: raw?.providers ?? {},
  };
}

/** Alert levels as the settings UI wants them: ascending, finite, in range. */
export function alertThresholds(prefs: AppPrefs): number[] {
  const list = (prefs.usageAlertThresholds ?? [80, 95])
    .filter((t) => Number.isInteger(t) && t >= 1 && t <= 100)
    .sort((a, b) => a - b);
  return list.length ? [...new Set(list)] : [80, 95];
}

export type ProviderInfo = {
  id: ProviderId;
  label: string;
  accent: string;
  /** Glyph beside the section label. Codex has none: its section leads the
   * popover and carries the plain uppercase label. */
  icon: LucideIcon | null;
  /** The step that puts this tool on the popover ("Don't see your provider?"). */
  setup: string;
  /** Finishes "Showing the last known usage — …" once the stored login has
   * expired. Codex has none: ConnectionStateView owns its signed-out copy. */
  signedOutHint: string | null;
  /** Setup-guide status line: what UsageBar reads once connected, and the
   * step to take when the backend gave no diagnostic. */
  onboarding: { connected: string; signIn: string };
};

/**
 * Every provider the popover knows, in display order. App.tsx renders the
 * sections, the refresh-all button, the setup guide and the missing list
 * from this one table, so a new provider is one entry here plus its hook.
 */
export const PROVIDER_CATALOG: readonly ProviderInfo[] = [
  {
    id: "codex",
    label: "Codex",
    accent: "codex",
    icon: null,
    setup: "Install the Codex CLI, sign in, and leave codex on your PATH.",
    signedOutHint: null,
    onboarding: {
      connected: "Connected through the Codex app server",
      signIn: "Sign in with the codex CLI, then retry",
    },
  },
  {
    id: "claude",
    label: "Claude Code",
    accent: "claude",
    icon: Sparkles,
    setup: "Run claude once and sign in. The Claude desktop app uses a different login.",
    signedOutHint: isWindows
      ? "the stored Claude Code login is stale. Open Claude Code to renew its login, then click Refresh. The desktop Claude app uses a different login than the `claude` CLI."
      : "the stored Claude Code login is stale. Click Refresh — macOS may ask once to read the Keychain item Claude Code already keeps. Always Allow stops that sheet. The desktop Claude app uses a different login than the `claude` CLI.",
    onboarding: {
      connected: "Reading the login kept by the claude command-line tool",
      signIn: "Sign in with the claude command-line tool",
    },
  },
  {
    id: "cursor",
    label: "Cursor",
    accent: "cursor",
    icon: MousePointer2,
    // deviceName, not "Mac": the Windows build shows this list too.
    setup: `Sign in through the Cursor app on this ${deviceName}.`,
    signedOutHint: "the stored Cursor login was rejected. Sign in through the Cursor app, then retry.",
    onboarding: {
      connected: "Reading the login kept by the Cursor app",
      signIn: "Sign in through the Cursor app",
    },
  },
  {
    id: "opencode",
    label: "OpenCode Go",
    accent: "opencode",
    icon: Terminal,
    setup: "In OpenCode, run /connect and choose OpenCode Go.",
    signedOutHint: "the stored OpenCode Go key was rejected. Sign in again with `/connect` and choose OpenCode Go.",
    onboarding: {
      connected: "Reading the OpenCode Go key from auth.json",
      signIn: "Sign in with /connect and choose OpenCode Go",
    },
  },
  {
    id: "devin",
    label: "Devin",
    accent: "devin",
    icon: Diamond,
    setup: `Run devin auth login so the CLI stores a login on this ${deviceName}.`,
    signedOutHint: "the stored Devin CLI login was rejected. Sign in again with `devin auth login`.",
    onboarding: {
      connected: "Reading the login kept by the Devin CLI",
      signIn: "Sign in with `devin auth login`",
    },
  },
  {
    id: "antigravity",
    label: "Antigravity",
    accent: "antigravity",
    icon: Sparkle,
    setup: "Open the Antigravity app and sign in. UsageBar reads quota from the running app.",
    signedOutHint: "open the Antigravity app and sign in there. UsageBar reads quota from the running app.",
    onboarding: {
      connected: "Reading the login kept by Antigravity",
      signIn: "Open the Antigravity app and sign in",
    },
  },
  {
    id: "gemini",
    label: "Gemini CLI",
    accent: "gemini",
    // Antigravity already wears the Sparkle, and two identical glyphs in one
    // popover read as one tool. The gem nods at the name instead.
    icon: Gem,
    setup: "Run gemini once and sign in with a paid/Workspace Google account (personal accounts no longer get quota access).",
    signedOutHint: "run `gemini` and sign in with a paid/Workspace Google account. Personal accounts no longer get quota access.",
    onboarding: {
      connected: "Reading the login kept by the gemini CLI",
      signIn: "Run `gemini` and sign in",
    },
  },
];

/** The backend's command and event names, which follow one scheme per tool. */
export function providerChannels(id: ProviderId) {
  return {
    getCommand: `get_${id}_state`,
    refreshCommand: `refresh_${id}`,
    event: `${id}://state`,
  };
}

/**
 * The setup guide's line for one tool, or null when the tool is absent from
 * this machine. Codex is always listed: the guide is built around it, so a
 * missing Codex login is itself the step to show.
 */
export function onboardingStatus(
  info: ProviderInfo,
  state: CodexBackendState,
): { connected: boolean; detail: string } | null {
  if (info.id !== "codex" && state.connection === "cli_not_found") return null;
  const connected = state.connection === "connected";
  return {
    connected,
    detail: connected ? info.onboarding.connected : state.diagnostic ?? info.onboarding.signIn,
  };
}

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

export const PROVIDER_SETUP = Object.fromEntries(
  PROVIDER_CATALOG.map((provider) => [provider.id, provider.setup]),
) as Record<ProviderId, string>;

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
      return [{ id: provider.id, label: provider.label, detail: provider.setup }];
    }
    return [];
  });
}
