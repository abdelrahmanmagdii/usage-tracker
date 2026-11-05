/**
 * Entry point of sim.html: boots the UNMODIFIED app (src/main.tsx) against a
 * simulated backend. Everything is installed at the boundary the app already
 * talks through — Tauri IPC (`window.__TAURI_INTERNALS__`), `Date`, `fetch`,
 * `Notification` and localStorage — before the app module is imported.
 *
 * See docs/SIMULATION.md for the URL parameters and the `window.__sim` API.
 */
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { SimClock, installClock, parseDuration, parseStart, realNow } from "./clock";
import { SIM_PROVIDERS, SCENARIOS, SimModel, scenarioById, type SimConnection, type SimProvider } from "./model";
import { SimBackend, defaultPrefs, serializePrefs, type SimLogEntry, type SimPrefs } from "./backend";
import { installFetch, installNotifications } from "./network";
import { clearAppStorage, seedHistory } from "./seed";
import { mountHud } from "./hud";
import "./sim.css";

type Json = Record<string, unknown>;

const params = new URLSearchParams(location.search);
const flag = (name: string, fallback = false): boolean => {
  if (!params.has(name)) return fallback;
  const value = params.get(name);
  return value === "" || value === "1" || value === "true" || value === "yes" || value === "on";
};
const num = (name: string, fallback: number): number => {
  const value = Number(params.get(name));
  return params.has(name) && Number.isFinite(value) ? value : fallback;
};

// ---- clock -----------------------------------------------------------------
const scenario = scenarioById(params.get("scenario"));
const seed = Math.trunc(num("seed", 1));
const startMs = parseStart(params.get("start")) ?? Math.floor(realNow() / 1000) * 1000;
const clock = new SimClock(startMs, num("speed", 1));
installClock(clock);
const model = new SimModel(scenario, seed, startMs / 1000);
// `advance=2h` opens the app two simulated hours into the scenario (history
// is seeded up to that moment, so the charts carry on without a gap).
if (params.has("advance")) clock.advance(parseDuration(params.get("advance") ?? "0"));

// ---- log -------------------------------------------------------------------
const log: SimLogEntry[] = [];
function record(entry: Omit<SimLogEntry, "at" | "simAt">): void {
  log.push({ at: new Date(realNow()).toISOString(), simAt: new Date(clock.now()).toISOString(), ...entry });
  if (log.length > 2_000) log.splice(0, log.length - 2_000);
  if (entry.kind === "warn") console.warn(`[sim] ${entry.name}`, entry.detail ?? "");
  else if (params.has("verbose")) console.debug(`[sim] ${entry.kind} ${entry.name}`, entry.detail ?? "");
}

// ---- prefs -----------------------------------------------------------------
const prefs: SimPrefs = defaultPrefs();
prefs.onboardingComplete = params.has("onboarding") ? !flag("onboarding") : scenario.onboardingComplete;
prefs.denseLayout = flag("dense");
prefs.combinedTray = flag("combined", true);
prefs.usageAlerts = flag("alerts", true);
if (params.has("providers")) {
  const shown = new Set((params.get("providers") ?? "").split(",").map((id) => id.trim()));
  for (const id of SIM_PROVIDERS) if (!shown.has(id)) prefs.providers[id] = { visible: false };
}

// ---- event bus (replaces mockIPC's shouldMockEvents) -------------------------
// @tauri-apps/api 2.12's built-in event mock never removes listeners on
// unlisten (it reads `args.id`, the API sends `eventId`), so React
// StrictMode's double-mounted effects would get every event twice and warn
// about missing callbacks. This bus implements listen/unlisten/emit properly.
type Internals = { runCallback: (id: number, data: unknown) => void };
const listeners = new Map<number, { event: string; handler: number }>();
let nextListener = 1;
function deliver(event: string, payload: unknown): void {
  window.setTimeout(() => {
    const internals = (window as unknown as { __TAURI_INTERNALS__: Internals }).__TAURI_INTERNALS__;
    for (const [id, listener] of [...listeners]) {
      if (listener.event === event && listeners.has(id)) internals.runCallback(listener.handler, { event, id, payload });
    }
  }, 0);
}
function emit(event: string, payload: unknown): void {
  record({ kind: "emit", name: event, detail: payload });
  deliver(event, payload);
}

// ---- backend ---------------------------------------------------------------
const latency = num("latency", 650);
const backend = new SimBackend({
  model,
  clock,
  prefs,
  demo: flag("demo"),
  autostart: flag("autostart"),
  refreshLatency: [latency * 0.5, latency * 1.5],
  emit,
  log: record,
});

const notifyPermission = (params.get("notify") ?? "granted") as NotificationPermission;
installNotifications(notifyPermission, (notification) =>
  backend.notify(notification.title, notification.body ?? ""),
);
installFetch(
  {
    startMs,
    mode: (params.get("feed") as "fixture" | "live" | "empty" | null) ?? "fixture",
    announcedResetInMinutes: params.has("alert") ? num("alert", 42) || 42 : scenario.announcedResetInMinutes,
  },
  (message) => record({ kind: "warn", name: message }),
);

const unhandled = new Set<string>();
mockWindows(params.has("widget") ? "codex-widget" : "main");
mockIPC(async (cmd, rawArgs) => {
  const args = (rawArgs ?? {}) as Json;
  if (cmd.startsWith("plugin:event|")) {
    if (cmd === "plugin:event|listen") {
      const id = nextListener++;
      listeners.set(id, { event: String(args.event), handler: Number(args.handler) });
      return id;
    }
    if (cmd === "plugin:event|unlisten") {
      listeners.delete(Number(args.eventId));
      return null;
    }
    if (cmd === "plugin:event|emit" || cmd === "plugin:event|emit_to") {
      deliver(String(args.event), args.payload);
      return null;
    }
    return null;
  }
  record({ kind: "invoke", name: cmd, detail: args });
  if (cmd.startsWith("plugin:window|") || cmd.startsWith("plugin:webview|")) return null;
  if (cmd === "plugin:notification|is_permission_granted") return Notification.permission === "granted";
  if (cmd === "plugin:notification|request_permission") return Notification.permission;
  if (cmd === "plugin:notification|notify") {
    const options = (args.options ?? {}) as Json;
    backend.notify(String(options.title ?? ""), String(options.body ?? ""));
    return null;
  }
  if (cmd.startsWith("plugin:notification|")) return [];
  if (cmd === "plugin:app|version") return "0.0.0-sim";
  const result = await backend.handle(cmd, args);
  if (!result.handled) {
    if (!unhandled.has(cmd)) {
      unhandled.add(cmd);
      record({ kind: "warn", name: `unhandled IPC command: ${cmd}`, detail: args });
    }
    throw `command ${cmd} not found`;
  }
  return result.value;
});

// ---- platform & storage ----------------------------------------------------
const platform = params.get("platform");
if (platform === "windows" || platform === "mac") {
  const ua =
    platform === "windows"
      ? "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36 Edg/130.0"
      : "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)";
  Object.defineProperty(navigator, "userAgent", { get: () => ua, configurable: true });
}
if (!flag("keep")) clearAppStorage();
let seeded = 0;
if (flag("history", scenario.seedHistory)) {
  seeded = seedHistory(model, {
    hours: num("historyHours", 12),
    stepMinutes: num("historyStep", 15),
    endSecs: Math.floor(clock.now() / 1000) - 60,
  });
}

const root = document.documentElement;
root.dataset.sim = scenario.id;
const background = params.get("bg") ?? "wallpaper";
root.dataset.simBg = background;

// ---- window.__sim ----------------------------------------------------------
function provider(id: string): SimProvider {
  if (!(SIM_PROVIDERS as readonly string[]).includes(id)) throw new Error(`Unknown provider ${id}`);
  return id as SimProvider;
}
const sinceStart = () => Math.floor(clock.now() / 1000) - model.t0;
const overrideFor = (id: string, connection: SimConnection, diagnostic: string | undefined, forSecs?: number, refreshError?: string) => {
  model.events.push({
    provider: provider(id),
    at: sinceStart(),
    until: forSecs === undefined ? undefined : sinceStart() + forSecs,
    connection,
    diagnostic,
    refreshError,
  });
  backend.poll(provider(id), true);
};

const sim = {
  scenario: scenario.id,
  description: scenario.description,
  scenarios: Object.keys(SCENARIOS),
  seed,
  start: new Date(startMs).toISOString(),
  model,
  backend,
  clock,
  log,
  seededHistory: seeded,
  /** Simulated time, ISO. */
  now: () => new Date(clock.now()).toISOString(),
  speed: (value?: number) => {
    if (value !== undefined) clock.setSpeed(value);
    return clock.speed;
  },
  pause: () => clock.setSpeed(0),
  play: (speed = 1) => clock.setSpeed(speed),
  /** Jump ahead: `advance("2h")`, `advance(90)` (seconds), `advance("1d6h")`. */
  advance: (amount: string | number) => {
    clock.advance(parseDuration(amount));
    return sim.now();
  },
  travelTo: (when: string | number) => {
    const target = typeof when === "number" ? when : parseStart(when);
    if (target === null) throw new Error(`Bad time ${when}`);
    clock.set(target);
    return sim.now();
  },
  state: (id: string) => backend.states.get(provider(id)),
  states: () => Object.fromEntries(backend.states),
  refresh: (id?: string) => {
    for (const each of id ? [provider(id)] : SIM_PROVIDERS) backend.poll(each, true);
  },
  /** Sign a provider out (optionally for `forSecs` simulated seconds). */
  signOut: (id: string, forSecs?: number) => overrideFor(id, "not_authenticated", undefined, forSecs),
  fail: (id: string, message = "Simulated failure", forSecs?: number) => overrideFor(id, "error", message, forSecs, message),
  uninstall: (id: string) => overrideFor(id, "cli_not_found", undefined),
  setConnection: (id: string, connection: SimConnection, diagnostic?: string, forSecs?: number) =>
    overrideFor(id, connection, diagnostic, forSecs),
  recover: (id: string) => overrideFor(id, "connected", undefined),
  /** Renew every window of a provider now, before its scheduled reset. */
  surpriseReset: (id: string) => {
    model.surpriseResets.push({ provider: provider(id), at: Math.floor(clock.now() / 1000) });
    backend.poll(provider(id), true);
  },
  emit,
  prefs: () => serializePrefs(backend.prefs),
  setPrefs: (patch: Partial<SimPrefs>) => backend.updatePrefs((current) => Object.assign(current, patch)),
  openSettings: () => emit("usagebar://demo", { action: "settings", amount: null }),
  closeSettings: () => emit("usagebar://demo", { action: "close-settings", amount: null }),
  scroll: (amount = 200) => emit("usagebar://demo", { action: "scroll", amount }),
  showOnboarding: () => emit("usagebar://show-onboarding", null),
  tray: () => backend.trayTitles(),
  notifications: backend.notifications,
  unhandled: () => [...unhandled],
};
(window as unknown as { __sim: typeof sim }).__sim = sim;

backend.start();
if (flag("hud")) mountHud(sim);

// ---- boot the real app -------------------------------------------------------
void import("../main").then(() => {
  if (flag("settings")) window.setTimeout(() => sim.openSettings(), 50);
});

const markReady = () => {
  if (document.querySelector(".app-shell, .codex-widget, [data-widget], #root > *")) {
    requestAnimationFrame(() => requestAnimationFrame(() => (root.dataset.simReady = "")));
    return true;
  }
  return false;
};
if (!markReady()) {
  const observer = new MutationObserver(() => markReady() && observer.disconnect());
  observer.observe(document.getElementById("root") ?? document.body, { childList: true, subtree: true });
}
