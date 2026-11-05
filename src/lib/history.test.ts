import { afterEach, describe, expect, it, vi } from "vitest";
import {
  currentWindowSamples,
  historySeries,
  inSameWindow,
  isPossibleSurpriseReset,
  observeBuckets,
} from "./history";
import type { QuotaSnapshot } from "../types/codex";

const base: QuotaSnapshot = {
  timestamp: "2026-08-12T20:00:00Z",
  limitId: "codex:primary:10080",
  usedPercent: 82,
  remainingPercent: 18,
  windowDurationMins: 10_080,
  resetsAt: Date.parse("2026-08-15T20:00:00Z") / 1000,
};

describe("surprise reset detection", () => {
  it("detects a dramatic pre-deadline quota jump", () => {
    expect(
      isPossibleSurpriseReset(base, {
        ...base,
        timestamp: "2026-08-12T21:00:00Z",
        usedPercent: 5,
        remainingPercent: 95,
      }),
    ).toBe(true);
  });

  it("does not flag normal consumption or a scheduled reset", () => {
    expect(
      isPossibleSurpriseReset(base, {
        ...base,
        timestamp: "2026-08-12T21:00:00Z",
        usedPercent: 85,
        remainingPercent: 15,
      }),
    ).toBe(false);
    expect(
      isPossibleSurpriseReset(base, {
        ...base,
        timestamp: "2026-08-16T21:00:00Z",
        usedPercent: 0,
        remainingPercent: 100,
      }),
    ).toBe(false);
  });
});

describe("recording history", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("keeps going when local storage refuses the write", () => {
    vi.stubGlobal("localStorage", {
      getItem: () => null,
      setItem: () => {
        throw new DOMException("The quota has been exceeded.", "QuotaExceededError");
      },
    });
    const bucket = {
      id: "session:primary",
      limitId: "session",
      windowKind: "primary" as const,
      usedPercent: 40,
      remainingPercent: 60,
      windowDurationMins: 300,
      reached: false,
    };
    expect(() => observeBuckets("claude", [bucket])).not.toThrow();
  });
});

describe("reading history", () => {
  const bucket = {
    id: "session:primary",
    limitId: "session",
    windowKind: "primary" as const,
    usedPercent: 40,
    remainingPercent: 60,
    windowDurationMins: 300,
    resetsAt: 2_000_000_000,
    reached: false,
  };

  function memoryStorage() {
    const store = new Map<string, string>();
    return {
      store,
      getItem: vi.fn((key: string) => store.get(key) ?? null),
      setItem: vi.fn((key: string, value: string) => void store.set(key, value)),
    };
  }

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it("parses the store once, not once per tile", () => {
    const storage = memoryStorage();
    vi.stubGlobal("localStorage", storage);
    observeBuckets("claude", [bucket, { ...bucket, id: "weekly:secondary", windowDurationMins: 10_080 }]);
    const parse = vi.spyOn(JSON, "parse");
    const session = historySeries("claude", "session:primary");
    const weekly = historySeries("claude", "weekly:secondary");
    expect(session).toHaveLength(1);
    expect(weekly).toHaveLength(1);
    expect(historySeries("claude", "session:primary")).toBe(session);
    expect(historySeries("codex", "session:primary")).toEqual([]);
    expect(parse).not.toHaveBeenCalled();
  });

  it("sees new observations and writes from another window", () => {
    const storage = memoryStorage();
    vi.stubGlobal("localStorage", storage);
    observeBuckets("claude", [bucket]);
    observeBuckets("claude", [{ ...bucket, usedPercent: 45, remainingPercent: 55 }]);
    expect(historySeries("claude", "session:primary").map((s) => s.usedPercent)).toEqual([40, 45]);
    // The dashboard and the popover share the store: a write that did not go
    // through this module must still be picked up.
    const external = JSON.parse(storage.store.get("codex-meter.quota-history.v1")!) as QuotaSnapshot[];
    external.push({ ...external[1], usedPercent: 50, remainingPercent: 50 });
    storage.store.set("codex-meter.quota-history.v1", JSON.stringify(external));
    expect(historySeries("claude", "session:primary").map((s) => s.usedPercent)).toEqual([40, 45, 50]);
  });

  it("treats an unreadable store as empty", () => {
    vi.stubGlobal("localStorage", {
      getItem: () => {
        throw new DOMException("Blocked", "SecurityError");
      },
      setItem: () => undefined,
    });
    expect(historySeries("claude", "session:primary")).toEqual([]);
    vi.stubGlobal("localStorage", { getItem: () => "{not json", setItem: () => undefined });
    expect(historySeries("claude", "session:primary")).toEqual([]);
  });
});

describe("currentWindowSamples", () => {
  const at = (iso: string, resetsAt?: number): QuotaSnapshot => ({ ...base, timestamp: iso, resetsAt });

  it("keeps one window, oldest first, despite small reset-time wobble", () => {
    const reset = base.resetsAt!;
    const samples = [
      at("2026-08-12T22:00:00Z", reset + 30),
      at("2026-08-12T21:00:00Z", reset),
      at("2026-08-12T20:00:00Z", reset - 7 * 86_400),
      at("2026-08-12T23:00:00Z", undefined),
    ];
    expect(currentWindowSamples(samples, reset).map((s) => s.timestamp)).toEqual([
      "2026-08-12T21:00:00Z",
      "2026-08-12T22:00:00Z",
    ]);
    expect(inSameWindow(samples[0], undefined)).toBe(false);
  });
});
