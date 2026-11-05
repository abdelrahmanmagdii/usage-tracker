import { describe, expect, it } from "vitest";
import { SIM_PROVIDERS, SCENARIOS, SimModel, scenarioById } from "./model";
import { SimPaceTracker, collectTrayWindows, normalizeThresholds, serializePrefs, defaultPrefs } from "./backend";
import { buildHistory } from "./seed";
import { parseDuration } from "./clock";

const T0 = Date.parse("2026-11-05T15:00:00Z") / 1000;

function usedPercents(limits: unknown): number[] {
  return collectTrayWindows(limits).map((window) => window.usedPercent);
}

describe("simulation model", () => {
  it("reports each scenario's numbers exactly at the start", () => {
    for (const id of Object.keys(SCENARIOS)) {
      const model = new SimModel(scenarioById(id), 7, T0);
      for (const provider of SIM_PROVIDERS) {
        const limits = model.rateLimits(provider, T0) as { rateLimitsByLimitId: Record<string, Record<string, { usedPercent: number }>> };
        for (const window of model.spec(provider).windows) {
          expect(limits.rateLimitsByLimitId[window.limitId][window.kind].usedPercent).toBe(Math.round(window.used));
        }
      }
    }
  });

  it("is deterministic for a seed and differs across seeds", () => {
    const a = new SimModel(scenarioById("busy"), 3, T0);
    const b = new SimModel(scenarioById("busy"), 3, T0);
    const c = new SimModel(scenarioById("busy"), 4, T0);
    const later = T0 + 5 * 3_600;
    expect(a.rateLimits("claude", later)).toEqual(b.rateLimits("claude", later));
    const differs = SIM_PROVIDERS.some(
      (provider) => JSON.stringify(a.rateLimits(provider, later)) !== JSON.stringify(c.rateLimits(provider, later)),
    );
    expect(differs).toBe(true);
  });

  it("only grows within a window, then renews", () => {
    const model = new SimModel(scenarioById("busy"), 1, T0);
    const fable = model.spec("claude").windows.find((window) => window.limitId === "weekly-scoped-fable")!;
    let previous = -1;
    for (let t = T0; t < T0 + fable.resetsIn; t += 300) {
      const reading = model.reading("claude", fable, t);
      expect(reading.used).toBeGreaterThanOrEqual(previous);
      expect(reading.resetsAt).toBe(T0 + fable.resetsIn);
      previous = reading.used;
    }
    const renewed = model.reading("claude", fable, T0 + fable.resetsIn + 60);
    expect(renewed.used).toBeLessThan(previous);
    expect(renewed.resetsAt).toBe(T0 + fable.resetsIn + 7 * 86_400);
  });

  it("holds still while idle", () => {
    const model = new SimModel(scenarioById("idle"), 1, T0);
    expect(usedPercents(model.rateLimits("codex", T0 + 1_800))).toEqual(usedPercents(model.rateLimits("codex", T0)));
  });

  it("applies scenario connection timelines and manual overrides", () => {
    const model = new SimModel(scenarioById("errors"), 1, T0);
    expect(model.connection("claude", T0).connection).toBe("connected");
    expect(model.connection("claude", T0 + 15 * 60).connection).toBe("not_authenticated");
    expect(model.connection("claude", T0 + 45 * 60).connection).toBe("connected");
    expect(model.connection("devin", T0).refreshError).toMatch(/503/);
    model.events.push({ provider: "cursor", at: 0, until: 60, connection: "error", diagnostic: "x" });
    expect(model.connection("cursor", T0 + 30).connection).toBe("error");
    expect(model.connection("cursor", T0 + 90).connection).toBe("connected");
  });

  it("surprise resets drop usage before the scheduled renewal", () => {
    const model = new SimModel(scenarioById("nearly-out"), 1, T0);
    model.surpriseResets.push({ provider: "codex", at: T0 + 60 });
    expect(Math.max(...usedPercents(model.rateLimits("codex", T0 + 61)))).toBeLessThan(5);
  });

  it("seeds history that ends where the live numbers start", () => {
    const model = new SimModel(scenarioById("default"), 1, T0);
    const history = buildHistory(model, ["codex"], { hours: 12, stepMinutes: 15, endSecs: T0 });
    expect(history.length).toBeGreaterThan(2);
    const weekly = history.filter((sample) => sample.limitId.startsWith("codex:codex:secondary"));
    expect(weekly.at(-1)?.usedPercent).toBe(47);
  });
});

describe("simulated backend helpers", () => {
  it("serializes prefs like serde (defaults skipped)", () => {
    const prefs = defaultPrefs();
    expect(serializePrefs(prefs)).not.toHaveProperty("providers");
    expect(serializePrefs(prefs)).not.toHaveProperty("usageAlertThresholds");
    prefs.providers.cursor = { visible: false, trayWindow: "auto" };
    prefs.usageAlertThresholds = [50];
    expect(serializePrefs(prefs)).toMatchObject({ providers: { cursor: { visible: false } }, usageAlertThresholds: [50] });
    expect(serializePrefs(prefs)).not.toHaveProperty("paceAlerts");
    prefs.paceAlerts = false;
    expect(serializePrefs(prefs)).toMatchObject({ paceAlerts: false });
  });

  it("warns about a fast-burning window once, like alerts.rs", () => {
    // A weekly window rising 2% every 5 minutes from 50%: dry in about two
    // hours, two days before it resets.
    const tracker = new SimPaceTracker();
    const resetsAt = T0 + 2 * 86_400;
    const context = { enabled: true, otherAlert: false, topThreshold: 95 };
    const fired: number[] = [];
    for (let step = 0; step <= 16; step += 1) {
      const window = { id: "weekly:secondary", label: "Weekly", usedPercent: 50 + 2 * step, resetsAt, durationMins: 10_080 };
      if (tracker.observe(window, T0 + step * 300, context)) fired.push(step * 5);
    }
    expect(fired).toEqual([35]);
  });

  it("normalizes alert thresholds like prefs.rs", () => {
    expect(normalizeThresholds([95, 80, 95])).toEqual([80, 95]);
    expect(() => normalizeThresholds([])).toThrow();
    expect(() => normalizeThresholds([0])).toThrow();
    expect(() => normalizeThresholds([1, 2, 3, 4, 5])).toThrow();
  });

  it("parses durations", () => {
    expect(parseDuration("1h30m")).toBe(5_400_000);
    expect(parseDuration("90")).toBe(90_000);
    expect(parseDuration(2)).toBe(2_000);
  });
});
