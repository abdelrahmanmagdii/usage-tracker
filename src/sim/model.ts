/**
 * The simulated quota model behind sim.html.
 *
 * Every provider has an "activity" signal (how hard the user is driving that
 * tool in each 10-minute slot: idle nights, working-hours sessions, bursts).
 * A quota window's usage is the activity consumed since the window started,
 * scaled so that the window reports exactly the scenario's percent at the
 * simulation start. Windows renew on a fixed period, so moving the clock past
 * `resetsAt` renews them like the real services do.
 *
 * All of it is a pure function of (seed, scenario, start time, sim time, the
 * manual events recorded through `window.__sim`), so screenshots taken with a
 * fixed `seed` and `start` are reproducible.
 *
 * Payload shapes mirror what the Rust backend emits (see
 * src-tauri/src/demo.rs and src-tauri/src/provider.rs): `rateLimitsByLimitId`
 * snapshots with `primary` / `secondary` windows in unix seconds.
 */
import { hash01, hashString } from "./rng";

export const SIM_PROVIDERS = ["codex", "claude", "cursor", "opencode", "devin", "antigravity", "gemini"] as const;
export type SimProvider = (typeof SIM_PROVIDERS)[number];

export type SimConnection =
  | "starting"
  | "connected"
  | "disconnected"
  | "cli_not_found"
  | "not_authenticated"
  | "error";

export type WindowSpec = {
  limitId: string;
  kind: "primary" | "secondary";
  mins: number;
  /** Snapshot-level `windowLabel` (overrides the duration heading). */
  label?: string;
  /** Snapshot-level `limitName`. */
  name?: string;
  /** Percent used at the simulation start. */
  used: number;
  /** Seconds from the simulation start until this window renews. */
  resetsIn: number;
  /** Percent of the window an average stretch of activity uses up. */
  fill?: number;
};

export type ProviderSpec = {
  plan: string;
  accountType: string;
  connection: SimConnection;
  diagnostic?: string;
  /** Relative activity: how much this tool is used next to the others. */
  weight: number;
  /** How often (simulated seconds) the backend re-reads this provider. */
  pollSecs: number;
  windows: WindowSpec[];
};

/** A connection override over a stretch of simulated time. */
export type ConnectionEvent = {
  provider: SimProvider;
  /** Seconds from the simulation start. */
  at: number;
  /** Seconds from the simulation start; open-ended when omitted. */
  until?: number;
  connection: SimConnection;
  diagnostic?: string;
  /** When set, `refresh_<provider>` rejects with this message meanwhile. */
  refreshError?: string;
};

export type Scenario = {
  id: string;
  description: string;
  /** Activity multiplier (1 = a normal working week). */
  activity: number;
  /** Multiplier on the chance that a 30-minute block is a working session. */
  busyness: number;
  /** Activity stops this many seconds before the start (idle scenario). */
  quietBeforeStart?: number;
  onboardingComplete: boolean;
  /** Seed local quota history so sparklines/charts have data. */
  seedHistory: boolean;
  /** Announced reset in the fixture feed, minutes after the start. */
  announcedResetInMinutes?: number;
  providers: Record<SimProvider, ProviderSpec>;
  timeline: ConnectionEvent[];
};

const H = 3_600;
const D = 86_400;
const SLOT = 600; // 10-minute activity slots
const SESSION_SLOTS = 3; // sessions come in 30-minute blocks

function w(
  limitId: string,
  kind: "primary" | "secondary",
  mins: number,
  used: number,
  resetsIn: number,
  extra: Partial<WindowSpec> = {},
): WindowSpec {
  return { limitId, kind, mins, used, resetsIn: Math.round(resetsIn), ...extra };
}

/** The demo-mode numbers from src-tauri/src/demo.rs. */
function baseProviders(): Record<SimProvider, ProviderSpec> {
  return {
    codex: {
      plan: "Plus",
      accountType: "oauth",
      connection: "connected",
      weight: 1,
      pollSecs: 30,
      windows: [
        w("codex", "primary", 300, 18, 2.2 * H, { name: "Codex subscription" }),
        w("codex", "secondary", 10_080, 47, 3.25 * D, { name: "Codex subscription" }),
      ],
    },
    claude: {
      plan: "Max",
      accountType: "oauth",
      connection: "connected",
      weight: 1.25,
      pollSecs: 60,
      windows: [
        w("session", "primary", 300, 6, 4.6 * H),
        w("weekly-all", "secondary", 10_080, 38, 1.2 * D, { name: "All models" }),
        w("weekly-scoped-fable", "secondary", 10_080, 91, 47 * 60, { label: "Fable", name: "Weekly limit", fill: 40 }),
      ],
    },
    cursor: {
      plan: "Pro",
      accountType: "oauth",
      connection: "connected",
      weight: 0.6,
      pollSecs: 300,
      windows: [
        w("plan", "primary", 43_200, 74, 12 * D, { label: "Monthly", name: "Cursor plan" }),
        w("auto", "secondary", 43_200, 15, 12 * D, { label: "Auto", name: "Auto + Composer" }),
      ],
    },
    opencode: {
      plan: "Go",
      accountType: "api",
      connection: "connected",
      weight: 0.5,
      pollSecs: 300,
      windows: [
        w("rolling", "primary", 300, 22, 3.4 * H),
        w("weekly", "secondary", 10_080, 31, 4 * D),
        w("monthly", "secondary", 43_200, 8, 20 * D, { label: "Monthly" }),
      ],
    },
    devin: {
      plan: "Pro",
      accountType: "oauth",
      connection: "connected",
      weight: 0.45,
      pollSecs: 300,
      windows: [
        w("daily", "primary", 1_440, 58, 11 * H, { label: "Daily", name: "Devin plan" }),
        w("weekly", "secondary", 10_080, 44, 4 * D, { label: "Weekly", name: "Devin plan" }),
      ],
    },
    antigravity: {
      plan: "Antigravity",
      accountType: "oauth",
      connection: "connected",
      weight: 0.55,
      pollSecs: 120,
      windows: [
        w("gemini-5h", "primary", 300, 12, 3.1 * H, { label: "Gemini 5-hour", name: "Gemini" }),
        w("gemini-weekly", "secondary", 10_080, 46, 3 * D, { label: "Gemini weekly", name: "Gemini" }),
        w("3p-5h", "primary", 300, 9, 4.2 * H, { label: "Claude and GPT 5-hour", name: "Claude and GPT" }),
        w("3p-weekly", "secondary", 10_080, 33, 5 * D, { label: "Claude and GPT weekly", name: "Claude and GPT" }),
        w("argon-weekly", "secondary", 10_080, 3, 6 * D, { label: "Gemini 4 Argon weekly", name: "Gemini 4 Argon" }),
      ],
    },
    gemini: {
      plan: "Free",
      accountType: "oauth",
      connection: "connected",
      weight: 0.4,
      pollSecs: 300,
      windows: [
        w("gemini-pro", "primary", 1_440, 41, 11 * H, { label: "Pro models", name: "Gemini Pro" }),
        w("gemini-flash", "secondary", 1_440, 17, 11 * H, { label: "Flash models", name: "Gemini Flash" }),
      ],
    },
  };
}

function withUsed(spec: ProviderSpec, used: number[]): ProviderSpec {
  return {
    ...spec,
    windows: spec.windows.map((window, index) => ({ ...window, used: used[index] ?? window.used })),
  };
}

function scenarioDefault(): Scenario {
  return {
    id: "default",
    description: "demo.rs numbers, a normal working week; Claude's Fable week renews 47 minutes in.",
    activity: 1,
    busyness: 1,
    onboardingComplete: true,
    seedHistory: true,
    providers: baseProviders(),
    timeline: [],
  };
}

function scenarioBusy(): Scenario {
  const base = baseProviders();
  return {
    id: "busy",
    description: "Heavy use: every tool in a long session, bursts, an announced Codex reset in 42 minutes.",
    activity: 2.2,
    busyness: 1.8,
    onboardingComplete: true,
    seedHistory: true,
    announcedResetInMinutes: 42,
    providers: {
      ...base,
      codex: withUsed(base.codex, [64, 71]),
      claude: withUsed(base.claude, [58, 66, 91]),
      cursor: withUsed(base.cursor, [82, 37]),
      opencode: withUsed(base.opencode, [55, 49, 23]),
      devin: withUsed(base.devin, [77, 61]),
      antigravity: withUsed(base.antigravity, [48, 62, 39, 51, 12]),
      gemini: withUsed(base.gemini, [69, 40]),
    },
    timeline: [],
  };
}

function scenarioIdle(): Scenario {
  const base = baseProviders();
  return {
    id: "idle",
    description: "Nothing used for six hours: meters hold still, countdowns run, windows renew.",
    activity: 1,
    busyness: 1,
    quietBeforeStart: 6 * H,
    onboardingComplete: true,
    seedHistory: true,
    providers: {
      ...base,
      codex: withUsed(base.codex, [0, 22]),
      claude: withUsed(base.claude, [0, 19, 35]),
      opencode: withUsed(base.opencode, [0, 12, 5]),
      antigravity: withUsed(base.antigravity, [0, 21, 0, 14, 1]),
    },
    timeline: [],
  };
}

function scenarioNearlyOut(): Scenario {
  const base = baseProviders();
  return {
    id: "nearly-out",
    description: "Most windows past 85%, a few at the limit; usage alerts fire as they cross 95%.",
    activity: 1.4,
    busyness: 1.3,
    onboardingComplete: true,
    seedHistory: true,
    providers: {
      ...base,
      codex: withUsed(base.codex, [93, 88]),
      claude: withUsed(base.claude, [97, 89, 100]),
      cursor: withUsed(base.cursor, [96, 71]),
      opencode: withUsed(base.opencode, [100, 92, 64]),
      devin: withUsed(base.devin, [91, 86]),
      antigravity: withUsed(base.antigravity, [94, 87, 99, 90, 41]),
      gemini: withUsed(base.gemini, [100, 83]),
    },
    timeline: [],
  };
}

function scenarioErrors(): Scenario {
  const base = baseProviders();
  return {
    id: "errors",
    description:
      "Devin's API fails, Gemini is signed out, Antigravity is not running; Claude signs out at +10m and recovers at +40m; Codex's App Server restarts at +20m.",
    activity: 1,
    busyness: 1,
    onboardingComplete: true,
    seedHistory: true,
    providers: {
      ...base,
      antigravity: { ...base.antigravity, connection: "cli_not_found" },
      gemini: {
        ...base.gemini,
        connection: "not_authenticated",
        diagnostic: "Gemini CLI is signed in with a personal Google account, which no longer gets quota access.",
      },
    },
    timeline: [
      {
        provider: "devin",
        at: -15 * 60,
        until: 25 * 60,
        connection: "error",
        diagnostic: "Devin returned HTTP 503",
        refreshError: "Devin returned HTTP 503",
      },
      {
        provider: "claude",
        at: 10 * 60,
        until: 40 * 60,
        connection: "not_authenticated",
        diagnostic: "Claude Code's stored login has expired.",
      },
      {
        provider: "codex",
        at: 20 * 60,
        until: 20 * 60 + 90,
        connection: "starting",
      },
      {
        provider: "cursor",
        at: 60 * 60,
        until: 90 * 60,
        connection: "error",
        diagnostic: "Cursor usage request timed out",
      },
    ],
  };
}

function scenarioFreshInstall(): Scenario {
  const base = baseProviders();
  const absent = (spec: ProviderSpec): ProviderSpec => ({ ...spec, connection: "cli_not_found" });
  return {
    id: "fresh-install",
    description: "First launch: onboarding open, no local history, only Codex and Claude Code signed in.",
    activity: 1,
    busyness: 1,
    onboardingComplete: false,
    seedHistory: false,
    providers: {
      ...base,
      cursor: absent(base.cursor),
      opencode: absent(base.opencode),
      devin: absent(base.devin),
      antigravity: absent(base.antigravity),
      gemini: absent(base.gemini),
    },
    // The Codex App Server takes a moment to answer on first launch.
    timeline: [{ provider: "codex", at: -1, until: 3, connection: "starting" }],
  };
}

export const SCENARIOS: Record<string, () => Scenario> = {
  default: scenarioDefault,
  busy: scenarioBusy,
  idle: scenarioIdle,
  "nearly-out": scenarioNearlyOut,
  errors: scenarioErrors,
  "fresh-install": scenarioFreshInstall,
};

export function scenarioById(id: string | null | undefined): Scenario {
  const make = SCENARIOS[id ?? "default"] ?? SCENARIOS.default;
  return make();
}

export type WindowReading = {
  used: number;
  resetsAt: number;
  windowStart: number;
};

function defaultFill(mins: number): number {
  if (mins <= 300) return 32;
  if (mins <= 1_440) return 45;
  if (mins <= 10_080) return 65;
  return 70;
}

/**
 * Deterministic usage model for one scenario. Times are unix seconds;
 * `t0` is the simulation start (the moment the scenario's numbers hold).
 */
export class SimModel {
  readonly scenario: Scenario;
  readonly seed: number;
  readonly t0: number;
  /** Manual connection overrides added at runtime (`__sim.fail(...)`). */
  readonly events: ConnectionEvent[] = [];
  /** Early renewals added at runtime (`__sim.surpriseReset(...)`), unix seconds. */
  readonly surpriseResets: Array<{ provider: SimProvider; at: number }> = [];
  private readonly cache = new Map<SimProvider, Map<number, number>>();
  private readonly scale = new Map<string, number>();

  constructor(scenario: Scenario, seed: number, t0: number) {
    this.scenario = scenario;
    this.seed = seed >>> 0;
    this.t0 = Math.floor(t0);
  }

  spec(provider: SimProvider): ProviderSpec {
    return this.scenario.providers[provider];
  }

  /** Activity (units per hour) in one 10-minute slot. */
  activity(provider: SimProvider, slot: number, baseline = false): number {
    const scenario = this.scenario;
    if (!baseline) {
      if (scenario.activity <= 0) return 0;
      if (scenario.quietBeforeStart !== undefined && slot * SLOT >= this.t0 - scenario.quietBeforeStart) return 0;
    }
    let cache = this.cache.get(provider);
    if (!cache) {
      cache = new Map();
      this.cache.set(provider, cache);
    }
    const key = baseline ? -slot - 1 : slot;
    const hit = cache.get(key);
    if (hit !== undefined) return hit;
    if (cache.size > 250_000) cache.clear();

    const pid = hashString(provider);
    const sec = slot * SLOT;
    const hour = Math.floor((sec % D) / H); // UTC: deterministic on any machine
    const dow = (Math.floor(sec / D) + 4) % 7; // 1970-01-01 was a Thursday
    let chance = hour >= 9 && hour < 19 ? 0.62 : hour >= 19 ? 0.3 : hour >= 7 ? 0.25 : 0.04;
    if (dow === 0 || dow === 6) chance *= 0.45;
    const busyness = baseline ? 1 : scenario.busyness;
    chance = Math.min(0.97, chance * busyness);
    let value = 0;
    if (hash01(this.seed, pid, Math.floor(slot / SESSION_SLOTS), 1) < chance) {
      value = 0.35 + 1.3 * hash01(this.seed, pid, slot, 2);
      if (hash01(this.seed, pid, slot, 3) < 0.07) value *= 4; // a burst
      value *= this.spec(provider).weight * (baseline ? 1 : scenario.activity);
    }
    cache.set(key, value);
    return value;
  }

  /** Activity consumed between two unix times. */
  consumed(provider: SimProvider, from: number, to: number, baseline = false): number {
    if (to <= from) return 0;
    let total = 0;
    const first = Math.floor(from / SLOT);
    const last = Math.floor((to - 1e-9) / SLOT);
    for (let slot = first; slot <= last; slot += 1) {
      const a = Math.max(from, slot * SLOT);
      const b = Math.min(to, (slot + 1) * SLOT);
      if (b > a) total += (this.activity(provider, slot, baseline) * (b - a)) / H;
    }
    return total;
  }

  /** Units of activity that make up 100% of a window. */
  private windowScale(provider: SimProvider, window: WindowSpec): number {
    const key = `${provider}:${window.limitId}:${window.kind}`;
    const cached = this.scale.get(key);
    if (cached !== undefined) return cached;
    // Baseline (normal-week) rate, so busier scenarios burn faster.
    const week = this.consumed(provider, this.t0 - 7 * D, this.t0, true);
    const ratePerHour = Math.max(week / (7 * 24), 0.05);
    const fill = window.fill ?? defaultFill(window.mins);
    const scale = (ratePerHour * (window.mins / 60) * 100) / fill;
    this.scale.set(key, scale);
    return scale;
  }

  /** Percent used and renewal time of one window at unix time `t`. */
  reading(provider: SimProvider, window: WindowSpec, t: number): WindowReading {
    const period = window.mins * 60;
    const firstEnd = this.t0 + window.resetsIn; // end of the window live at t0
    const firstStart = firstEnd - period;
    const k = Math.floor((t - firstStart) / period);
    const start = firstStart + k * period;
    const end = start + period;
    const scale = this.windowScale(provider, window);

    const early = this.surpriseResets
      .filter((reset) => reset.provider === provider && reset.at >= start && reset.at <= t)
      .reduce<number | null>((latest, reset) => (latest === null || reset.at > latest ? reset.at : latest), null);

    let used: number;
    if (early !== null) {
      used = (100 * this.consumed(provider, early, t)) / scale;
    } else if (k === 0 && t < this.t0) {
      // Before the start, ramp up to the scenario's number along the
      // activity curve, so history reads like real use.
      const total = this.consumed(provider, start, this.t0);
      used =
        total > 0
          ? (window.used * this.consumed(provider, start, t)) / total
          : (window.used * (t - start)) / Math.max(1, this.t0 - start);
    } else if (k === 0) {
      used = window.used + (100 * this.consumed(provider, this.t0, t)) / scale;
    } else {
      used = (100 * this.consumed(provider, start, t)) / scale;
    }
    return {
      used: Math.max(0, Math.min(100, Math.round(used))),
      resetsAt: end,
      windowStart: start,
    };
  }

  /** The `rateLimits` payload a backend would report at `t`. */
  rateLimits(provider: SimProvider, t: number): Record<string, unknown> {
    const byId: Record<string, Record<string, unknown>> = {};
    for (const window of this.spec(provider).windows) {
      const snapshot = (byId[window.limitId] ??= { limitId: window.limitId });
      if (window.label) snapshot.windowLabel = window.label;
      if (window.name) snapshot.limitName = window.name;
      const reading = this.reading(provider, window, t);
      snapshot[window.kind] = {
        usedPercent: reading.used,
        windowDurationMins: window.mins,
        resetsAt: reading.resetsAt,
      };
    }
    const payload: Record<string, unknown> = { rateLimitsByLimitId: byId };
    if (provider === "codex") payload.rateLimitResetCredits = { availableCount: 1 };
    return payload;
  }

  /** Codex's token usage block: last seven days, from the same activity. */
  codexUsage(t: number): Record<string, unknown> {
    const today = Math.floor(t / D) * D;
    const buckets = Array.from({ length: 7 }, (_, index) => {
      const dayStart = today - (6 - index) * D;
      const units = this.consumed("codex", dayStart, Math.min(dayStart + D, t));
      return {
        startDate: new Date(dayStart * 1000).toISOString().replace(".000Z", "Z"),
        tokens: Math.round(units * 9_500 / 1000) * 1000,
      };
    });
    const week = buckets.reduce((sum, bucket) => sum + bucket.tokens, 0);
    return {
      summary: {
        lifetimeTokens: 2_470_000 + week,
        peakDailyTokens: Math.max(184_000, ...buckets.map((bucket) => bucket.tokens)),
        currentStreakDays: 12,
        longestStreakDays: 21,
      },
      dailyUsageBuckets: buckets,
    };
  }

  /** The connection a provider has at `t`, after scenario and manual events. */
  connection(provider: SimProvider, t: number): Omit<ConnectionEvent, "provider" | "at" | "until"> {
    const spec = this.spec(provider);
    let current: Omit<ConnectionEvent, "provider" | "at" | "until"> = {
      connection: spec.connection,
      diagnostic: spec.diagnostic,
    };
    for (const event of [...this.scenario.timeline, ...this.events]) {
      if (event.provider !== provider) continue;
      const from = this.t0 + event.at;
      const until = event.until === undefined ? Infinity : this.t0 + event.until;
      if (t >= from && t < until) {
        current = { connection: event.connection, diagnostic: event.diagnostic, refreshError: event.refreshError };
      }
    }
    return current;
  }
}
