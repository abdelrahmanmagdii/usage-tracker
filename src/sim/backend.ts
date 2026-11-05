/**
 * A stand-in for the Rust backend, served through Tauri's IPC mock.
 *
 * It answers every command the frontend invokes (see src-tauri/src/commands.rs)
 * and emits the same events (`<provider>://state`, `usagebar://prefs`,
 * `usagebar://show-onboarding`, `usagebar://demo`). Preference semantics follow
 * src-tauri/src/prefs.rs: a sparse per-provider map, defaults omitted, and a
 * `usagebar://prefs` broadcast after every change that goes through
 * `tray::apply_preference_change` (but not `complete_onboarding`).
 */
import type { SimClock } from "./clock";
import { SIM_PROVIDERS, type SimModel, type SimProvider } from "./model";
import { currentWindowSamples, projectedRunOut } from "../lib/history";
import type { QuotaSnapshot } from "../types/codex";

type Json = Record<string, unknown>;

export type SimProviderPref = { visible?: boolean; trayWindow?: string };

export type SimPrefs = {
  usageAlerts: boolean;
  paceAlerts: boolean;
  resetAlerts: boolean;
  denseLayout: boolean;
  combinedTray: boolean;
  onboardingComplete: boolean;
  usageAlertThresholds: number[];
  providers: Record<string, SimProviderPref>;
};

export type ProviderStatePayload = {
  connection: string;
  diagnostic: string | null;
  account: unknown;
  rateLimits: unknown;
  updatedAt: number | null;
  usage?: unknown;
};

export type SimLogEntry = { at: string; simAt: string; kind: "invoke" | "emit" | "notify" | "warn"; name: string; detail?: unknown };

export type TrayWindow = {
  id: string;
  label: string;
  usedPercent: number;
  resetsAt?: number;
  durationMins?: number;
};

const DEFAULT_THRESHOLDS = [80, 95];
const MAX_ALERT_THRESHOLDS = 4;
const TRAY_STALE_AFTER_SECS = 15 * 60;

export function defaultPrefs(): SimPrefs {
  return {
    usageAlerts: true,
    paceAlerts: true,
    resetAlerts: true,
    denseLayout: false,
    combinedTray: true,
    onboardingComplete: false,
    usageAlertThresholds: [...DEFAULT_THRESHOLDS],
    providers: {},
  };
}

/** serde output of `AppPrefs`: defaults are skipped like `skip_serializing_if`. */
export function serializePrefs(prefs: SimPrefs): Json {
  const out: Json = {
    usageAlerts: prefs.usageAlerts,
    resetAlerts: prefs.resetAlerts,
    denseLayout: prefs.denseLayout,
    combinedTray: prefs.combinedTray,
    onboardingComplete: prefs.onboardingComplete,
  };
  if (!prefs.paceAlerts) out.paceAlerts = false;
  if (prefs.usageAlertThresholds.join(",") !== DEFAULT_THRESHOLDS.join(",")) {
    out.usageAlertThresholds = [...prefs.usageAlertThresholds];
  }
  const providers: Record<string, SimProviderPref> = {};
  for (const [id, pref] of Object.entries(prefs.providers)) {
    const entry: SimProviderPref = {};
    if (pref.visible === false) entry.visible = false;
    if (pref.trayWindow && pref.trayWindow !== "auto") entry.trayWindow = pref.trayWindow;
    if (Object.keys(entry).length) providers[id] = entry;
  }
  if (Object.keys(providers).length) out.providers = providers;
  return out;
}

/** `prefs::normalize_alert_thresholds`. */
export function normalizeThresholds(input: unknown): number[] {
  if (!Array.isArray(input) || input.length === 0) throw "At least one alert threshold is required.";
  if (input.some((t) => !Number.isInteger(t) || t < 1 || t > 100)) {
    throw "Alert thresholds must be whole percents between 1 and 100.";
  }
  if (input.length > MAX_ALERT_THRESHOLDS) throw `At most ${MAX_ALERT_THRESHOLDS} alert thresholds.`;
  return [...new Set(input as number[])].sort((a, b) => a - b);
}

function durationLabel(mins: number | undefined): string {
  if (mins === 300) return "5-hour";
  if (mins === 1_440) return "Daily";
  if (mins === 10_080) return "Weekly";
  if (mins && mins > 0 && mins % 10_080 === 0) return `${mins / 10_080}-week`;
  if (mins && mins > 0 && mins % 1_440 === 0) return `${mins / 1_440}-day`;
  if (mins && mins > 0 && mins % 60 === 0) return `${mins / 60}-hour`;
  return "Limit";
}

/** `tray::collect_windows`: every window, shortest first. */
export function collectTrayWindows(payload: unknown): TrayWindow[] {
  const byId = (payload as Json | null | undefined)?.rateLimitsByLimitId as Record<string, Json> | undefined;
  const out: TrayWindow[] = [];
  for (const [limitId, snapshot] of Object.entries(byId ?? {})) {
    for (const kind of ["primary", "secondary"] as const) {
      const window = snapshot[kind] as Json | undefined;
      if (!window || typeof window.usedPercent !== "number") continue;
      const mins = typeof window.windowDurationMins === "number" ? window.windowDurationMins : undefined;
      out.push({
        id: `${limitId}:${kind}`,
        label:
          (window.windowLabel as string | undefined) ??
          (snapshot.windowLabel as string | undefined) ??
          durationLabel(mins),
        usedPercent: Math.max(0, Math.min(100, window.usedPercent)),
        resetsAt: typeof window.resetsAt === "number" ? Math.max(0, Math.floor(window.resetsAt)) : undefined,
        durationMins: mins,
      });
    }
  }
  return out.sort(
    (a, b) => (a.durationMins ?? Infinity) - (b.durationMins ?? Infinity) || a.id.localeCompare(b.id),
  );
}

function formatCountdown(total: number): string {
  const days = Math.floor(total / 86_400);
  if (days > 0) return `${days}d ${Math.floor((total % 86_400) / 3_600)}h`;
  const hours = Math.floor(total / 3_600);
  const minutes = Math.floor((total % 3_600) / 60);
  const seconds = total % 60;
  const pad = (n: number) => String(n).padStart(2, "0");
  return hours > 0 ? `${hours}:${pad(minutes)}:${pad(seconds)}` : `${minutes}:${pad(seconds)}`;
}

// The pace-alert rules of alerts.rs. The projection itself is the
// popover's `projectedRunOut`, which alerts.rs ports.
const PACE_LOOKBACK_MS = 3 * 3_600_000;
const PACE_ALERT_HORIZON_SECS = 3 * 3_600;
const PACE_ALERT_MIN_MARGIN_SECS = 30 * 60;
const PACE_ALERT_MIN_MARGIN_FRACTION = 0.1;
const PACE_ALERT_MIN_SPAN_MS = 30 * 60_000;
const PACE_ALERT_CONFIRMATIONS = 2;
const PACE_MAX_SAMPLES = 256;
const RENEWAL_MIN_RESET_ADVANCE_SECS = 15 * 60;

export type PaceWarning = { runsOutIn: number; beforeReset: number };

/**
 * `alerts::pace_warning`: a projection worth a notification, before the
 * once-per-window and confirmation rules. `reached` is approximated by a
 * full window (tray windows do not carry the provider's reached flag).
 */
export function paceWarning(
  samples: readonly QuotaSnapshot[],
  window: TrayWindow,
  nowSecs: number,
  topThreshold: number,
): PaceWarning | null {
  if (window.usedPercent >= topThreshold || window.resetsAt === undefined) return null;
  const nowMs = nowSecs * 1000;
  const bucket = { usedPercent: window.usedPercent, resetsAt: window.resetsAt, reached: window.usedPercent >= 100 };
  const runOutMs = projectedRunOut(samples, bucket, nowMs);
  if (runOutMs === null) return null;
  const usable = currentWindowSamples(samples, window.resetsAt).filter(
    (sample) => nowMs - Date.parse(sample.timestamp) <= PACE_LOOKBACK_MS,
  );
  if (usable.length < 2 || nowMs - Date.parse(usable[0].timestamp) < PACE_ALERT_MIN_SPAN_MS) return null;
  const runOut = Math.max(nowSecs, Math.floor(runOutMs / 1000));
  const runsOutIn = runOut - nowSecs;
  const beforeReset = Math.max(0, window.resetsAt - runOut);
  const minMargin = Math.max(
    PACE_ALERT_MIN_MARGIN_SECS,
    (window.durationMins && window.durationMins > 0 ? window.durationMins * 60 : 0) * PACE_ALERT_MIN_MARGIN_FRACTION,
  );
  return runsOutIn <= PACE_ALERT_HORIZON_SECS && beforeReset >= minMargin ? { runsOutIn, beforeReset } : null;
}

/** `alerts::PaceTracker`: history and firing state for one window. */
export class SimPaceTracker {
  samples: QuotaSnapshot[] = [];
  private streak = 0;
  private warnedFor: number | undefined;

  observe(
    window: TrayWindow,
    nowSecs: number,
    context: { enabled: boolean; otherAlert: boolean; topThreshold: number },
  ): PaceWarning | null {
    this.record(window, nowSecs);
    const warning = paceWarning(this.samples, window, nowSecs, context.topThreshold);
    const warned =
      this.warnedFor !== undefined &&
      window.resetsAt !== undefined &&
      window.resetsAt < this.warnedFor + RENEWAL_MIN_RESET_ADVANCE_SECS;
    if (!context.enabled || !warning || warned) {
      this.streak = 0;
      return null;
    }
    this.streak += 1;
    if (this.streak < PACE_ALERT_CONFIRMATIONS || context.otherAlert) return null;
    this.warnedFor = window.resetsAt;
    this.streak = 0;
    return warning;
  }

  private record(window: TrayWindow, nowSecs: number): void {
    const nowMs = nowSecs * 1000;
    const sample: QuotaSnapshot = {
      timestamp: new Date(nowMs).toISOString(),
      limitId: window.id,
      usedPercent: window.usedPercent,
      remainingPercent: 100 - window.usedPercent,
      windowDurationMins: window.durationMins,
      resetsAt: window.resetsAt,
    };
    const last = this.samples.at(-1);
    const unchanged = last && last.usedPercent === sample.usedPercent && last.resetsAt === sample.resetsAt;
    if (last && !unchanged && currentWindowSamples([last], sample.resetsAt).length === 0) {
      // A new window starts a fresh history.
      this.samples = [];
      this.streak = 0;
    }
    if (!unchanged) this.samples.push(sample);
    while (this.samples.length > 1 && nowMs - Date.parse(this.samples[0].timestamp) > PACE_LOOKBACK_MS) {
      this.samples.shift();
    }
    if (this.samples.length > PACE_MAX_SAMPLES) this.samples.splice(0, this.samples.length - PACE_MAX_SAMPLES);
  }
}

const emptyState = (connection: string, diagnostic?: string): ProviderStatePayload => ({
  connection,
  diagnostic: diagnostic ?? null,
  account: null,
  rateLimits: null,
  updatedAt: null,
});

export type BackendOptions = {
  model: SimModel;
  clock: SimClock;
  prefs: SimPrefs;
  demo: boolean;
  autostart: boolean;
  /** Real-time latency range for refresh commands, ms. */
  refreshLatency: [number, number];
  emit: (event: string, payload: unknown) => void;
  log: (entry: Omit<SimLogEntry, "at" | "simAt">) => void;
};

/** What the Rust side would hold in its managers and prefs store. */
export class SimBackend {
  readonly model: SimModel;
  readonly clock: SimClock;
  prefs: SimPrefs;
  autostart: boolean;
  resetIncomingUntil: number | null = null;
  readonly states = new Map<SimProvider, ProviderStatePayload>();
  readonly notifications: Array<{ simAt: string; title: string; body: string }> = [];
  private readonly lastGood = new Map<SimProvider, { rateLimits: unknown; updatedAt: number; usage?: unknown }>();
  private readonly nextPoll = new Map<SimProvider, number>();
  private readonly seen = new Map<string, number>();
  private readonly pace = new Map<string, SimPaceTracker>();
  private timer = 0;

  constructor(private readonly options: BackendOptions) {
    this.model = options.model;
    this.clock = options.clock;
    this.prefs = options.prefs;
    this.autostart = options.autostart;
    const t = this.nowSecs();
    for (const provider of SIM_PROVIDERS) {
      this.recoverLastGood(provider, t);
      this.poll(provider, false);
    }
  }

  nowSecs(): number {
    return Math.floor(this.clock.now() / 1000);
  }

  start(): void {
    window.clearInterval(this.timer);
    this.timer = window.setInterval(() => this.tick(), 200);
    this.clock.onChange(() => this.tick());
  }

  stop(): void {
    window.clearInterval(this.timer);
  }

  visible(provider: SimProvider): boolean {
    return this.prefs.providers[provider]?.visible !== false;
  }

  /** Re-read every due provider; hidden providers are not polled (lib.rs). */
  tick(): void {
    const t = this.nowSecs();
    for (const provider of SIM_PROVIDERS) {
      if (!this.visible(provider)) continue;
      // A scheduled poll, or a connection change (sign-out, recovery, the
      // Codex App Server coming up) that the real managers report at once.
      const changed = this.model.connection(provider, t).connection !== this.states.get(provider)?.connection;
      if (changed || t >= (this.nextPoll.get(provider) ?? 0)) this.poll(provider, true);
    }
  }

  /** Reads a provider "now", stores and (optionally) emits its state. */
  poll(provider: SimProvider, emit: boolean): ProviderStatePayload {
    const t = this.nowSecs();
    const spec = this.model.spec(provider);
    this.nextPoll.set(provider, t + spec.pollSecs);
    const { connection, diagnostic } = this.model.connection(provider, t);
    let state: ProviderStatePayload;
    const account = { type: spec.accountType, planType: spec.plan };
    if (connection === "connected") {
      const rateLimits = this.model.rateLimits(provider, t);
      const usage = provider === "codex" ? this.model.codexUsage(t) : undefined;
      this.lastGood.set(provider, { rateLimits, updatedAt: t, usage });
      state = { connection, diagnostic: null, account, rateLimits, updatedAt: t };
      if (usage) state.usage = usage;
      this.observeAlerts(provider, rateLimits);
    } else if (connection === "cli_not_found" || connection === "starting") {
      state = emptyState(connection, diagnostic);
    } else {
      const last = this.lastGood.get(provider);
      state = {
        connection,
        diagnostic: diagnostic ?? null,
        account: last ? account : null,
        rateLimits: last?.rateLimits ?? null,
        updatedAt: last?.updatedAt ?? null,
      };
      if (last?.usage) state.usage = last.usage;
    }
    this.states.set(provider, state);
    if (emit) this.options.emit(`${provider}://state`, state);
    return state;
  }

  /** A provider that starts out failing still shows its last reading. */
  private recoverLastGood(provider: SimProvider, t: number): void {
    const spec = this.model.spec(provider);
    if (this.model.connection(provider, t).connection === "connected") return;
    for (let back = 1; back <= 240; back += 1) {
      const at = t - back * spec.pollSecs;
      if (this.model.connection(provider, at).connection !== "connected") continue;
      const rateLimits = this.model.rateLimits(provider, at);
      this.lastGood.set(provider, {
        rateLimits,
        updatedAt: at,
        usage: provider === "codex" ? this.model.codexUsage(at) : undefined,
      });
      this.observeAlerts(provider, rateLimits, true);
      return;
    }
  }

  /**
   * `alerts::UsageAlerts::observe`: first sighting seeds silently. Pace
   * warnings need `usageAlerts` and `paceAlerts`, and wait a refresh when a
   * threshold or fresh-window alert just went out for the same window.
   */
  private observeAlerts(provider: SimProvider, rateLimits: unknown, seedOnly = false): void {
    const thresholds = [...this.prefs.usageAlertThresholds].sort((a, b) => b - a);
    const topThreshold = thresholds[0] ?? 100;
    const t = this.nowSecs();
    for (const window of collectTrayWindows(rateLimits)) {
      const key = `${provider}:${window.id}`;
      const previous = this.seen.get(key);
      this.seen.set(key, window.usedPercent);
      const live = previous !== undefined && !seedOnly;
      const enabled = live && this.prefs.usageAlerts;
      const crossed =
        enabled && previous !== undefined
          ? thresholds.find((level) => previous < level && window.usedPercent >= level)
          : undefined;
      const fresh = enabled && previous !== undefined && crossed === undefined && previous >= 50 && window.usedPercent < 10;
      const label = window.label;
      let tracker = this.pace.get(key);
      if (!tracker) this.pace.set(key, (tracker = new SimPaceTracker()));
      const warning = tracker.observe(window, t, {
        enabled: enabled && this.prefs.paceAlerts,
        otherAlert: crossed !== undefined || fresh,
        topThreshold,
      });
      if (warning) {
        const runsOutIn = Math.max(1, Math.ceil(warning.runsOutIn / 60)) * 60;
        this.notify(
          `${provider}: ${label} window runs out in ~${formatCountdown(runsOutIn)}`,
          `${Math.round(window.usedPercent)}% used · at this pace, empty ${formatCountdown(warning.beforeReset)} before it resets`,
        );
      }
      if (crossed !== undefined) {
        const left = window.resetsAt && window.resetsAt > t ? ` · resets in ${formatCountdown(window.resetsAt - t)}` : "";
        this.notify(`${provider}: ${label} window at ${crossed}%`, `${window.usedPercent}% used${left}`);
      } else if (fresh) {
        this.notify(`${provider}: fresh ${label} window`, `Usage is back down to ${window.usedPercent}%`);
      }
    }
  }

  notify(title: string, body: string): void {
    this.notifications.push({ simAt: new Date(this.clock.now()).toISOString(), title, body });
    this.options.log({ kind: "notify", name: title, detail: body });
  }

  updatePrefs(mutate: (prefs: SimPrefs) => void, broadcast = true): void {
    mutate(this.prefs);
    if (broadcast) this.options.emit("usagebar://prefs", serializePrefs(this.prefs));
  }

  appPrefs(): Json {
    const out = serializePrefs(this.prefs);
    // commands::get_app_prefs: demo mode never shows the walkthrough.
    if (this.options.demo) out.onboardingComplete = true;
    return out;
  }

  /** The menu-bar title(s) the Rust tray would show right now. */
  trayTitles(): { combined: boolean; titles: Array<{ provider: SimProvider; title: string }>; title: string } {
    const now = this.nowSecs();
    const segments: Array<{ provider: SimProvider; remaining: number; resetsAt?: number; stale: boolean; incoming: boolean }> = [];
    for (const provider of SIM_PROVIDERS) {
      const state = this.states.get(provider);
      if (!this.visible(provider) || !state || state.connection === "cli_not_found") continue;
      const windows = collectTrayWindows(state.rateLimits);
      const pref = this.prefs.providers[provider]?.trayWindow ?? "auto";
      const selected =
        (pref !== "auto" ? windows.find((window) => window.id === pref) : undefined) ??
        windows.reduce<TrayWindow | undefined>((best, window) => (!best || window.usedPercent > best.usedPercent ? window : best), undefined);
      if (!selected) continue;
      segments.push({
        provider,
        remaining: Math.max(0, Math.min(100, 100 - selected.usedPercent)),
        resetsAt: selected.resetsAt,
        stale: state.updatedAt != null && now - state.updatedAt >= TRAY_STALE_AFTER_SECS,
        incoming: provider === "codex" && this.resetIncomingUntil !== null && this.resetIncomingUntil > now,
      });
    }
    const decorate = (text: string, seg: (typeof segments)[number]) => {
      let out = seg.stale && text ? `~${text}` : text;
      if (seg.incoming) out = out ? `⚡ ${out}` : "⚡";
      return out;
    };
    const full = (seg: (typeof segments)[number]) => {
      const percent = `${Math.round(seg.remaining)}%`;
      const base = seg.resetsAt && seg.resetsAt > now ? `${percent} · ${formatCountdown(seg.resetsAt - now)}` : percent;
      return decorate(base, seg);
    };
    const titles = segments.map((seg) => ({ provider: seg.provider, title: full(seg) }));
    let title: string;
    if (!this.prefs.combinedTray) title = titles.map((entry) => entry.title).join("  |  ");
    else if (segments.length === 1) title = full(segments[0]);
    else title = segments.map((seg) => decorate(`${Math.round(seg.remaining)}%`, seg)).join(" · ");
    return { combined: this.prefs.combinedTray, titles, title };
  }

  private latency(): Promise<void> {
    const [lo, hi] = this.options.refreshLatency;
    const ms = lo + Math.random() * Math.max(0, hi - lo);
    return new Promise((resolve) => window.setTimeout(resolve, ms));
  }

  private providerArg(args: Json): SimProvider {
    const provider = String(args.provider);
    if (!(SIM_PROVIDERS as readonly string[]).includes(provider)) throw `Unknown provider: ${provider}`;
    return provider as SimProvider;
  }

  /** The command table. Returns `undefined` for commands it does not know. */
  async handle(cmd: string, args: Json): Promise<{ handled: boolean; value?: unknown }> {
    const ok = (value?: unknown) => ({ handled: true, value });
    const stateMatch = /^(get|refresh)_([a-z]+)_state$|^refresh_([a-z]+)$/.exec(cmd);
    if (stateMatch) {
      const provider = (stateMatch[2] ?? stateMatch[3]) as SimProvider;
      if ((SIM_PROVIDERS as readonly string[]).includes(provider)) {
        if (cmd.startsWith("get_")) return ok(this.states.get(provider) ?? this.poll(provider, false));
        await this.latency();
        const { refreshError } = this.model.connection(provider, this.nowSecs());
        if (refreshError) throw refreshError;
        return ok(this.poll(provider, true));
      }
    }
    switch (cmd) {
      case "get_app_prefs":
        return ok(this.appPrefs());
      case "is_demo_mode":
        return ok(this.options.demo);
      case "get_tray_windows":
        return ok(
          Object.fromEntries(
            SIM_PROVIDERS.map((provider) => [
              provider,
              collectTrayWindows(this.states.get(provider)?.rateLimits).map((window) => ({
                ...window,
                resetsAt: window.resetsAt ?? null,
                durationMins: window.durationMins ?? null,
              })),
            ]),
          ),
        );
      case "set_tray_window": {
        const provider = this.providerArg(args);
        this.updatePrefs((prefs) => {
          prefs.providers[provider] = { ...prefs.providers[provider], trayWindow: String(args.window) };
        });
        return ok(null);
      }
      case "set_provider_visible": {
        const provider = this.providerArg(args);
        const visible = Boolean(args.visible);
        this.updatePrefs((prefs) => {
          prefs.providers[provider] = { ...prefs.providers[provider], visible };
        });
        if (visible) for (const id of SIM_PROVIDERS) if (this.visible(id)) this.poll(id, true);
        return ok(null);
      }
      case "set_usage_alerts":
        this.updatePrefs((prefs) => void (prefs.usageAlerts = Boolean(args.enabled)));
        return ok(null);
      case "set_pace_alerts":
        this.updatePrefs((prefs) => void (prefs.paceAlerts = Boolean(args.enabled)));
        return ok(null);
      case "set_reset_alerts":
        this.updatePrefs((prefs) => void (prefs.resetAlerts = Boolean(args.enabled)));
        return ok(null);
      case "set_dense_layout":
        this.updatePrefs((prefs) => void (prefs.denseLayout = Boolean(args.enabled)));
        return ok(null);
      case "set_combined_tray":
        this.updatePrefs((prefs) => void (prefs.combinedTray = Boolean(args.enabled)));
        return ok(null);
      case "set_usage_alert_thresholds": {
        const thresholds = normalizeThresholds(args.thresholds);
        this.updatePrefs((prefs) => void (prefs.usageAlertThresholds = thresholds));
        return ok(null);
      }
      case "get_autostart":
        return ok(this.autostart);
      case "set_autostart":
        this.autostart = Boolean(args.enabled);
        this.updatePrefs(() => undefined);
        return ok(null);
      case "complete_onboarding":
        // Persisted, but not broadcast (commands::complete_onboarding).
        this.updatePrefs((prefs) => void (prefs.onboardingComplete = true), false);
        return ok(null);
      case "set_reset_incoming":
        this.resetIncomingUntil = typeof args.until === "number" ? args.until : null;
        return ok(null);
      case "open_post_url": {
        const url = String(args.url ?? "");
        const allowed = ["https://bsky.app/", "https://x.com/", "https://twitter.com/"].some((prefix) => url.startsWith(prefix));
        if (!allowed || /\s/.test(url)) throw "Only reset post links can be opened.";
        return ok(null);
      }
      case "hide_window":
      case "show_dashboard":
      case "quit_app":
      case "set_widget_expanded":
      case "drag_codex_widget":
      case "hide_codex_widget":
      case "reset_widget_position":
        return ok(null);
      default:
        return { handled: false };
    }
  }
}
