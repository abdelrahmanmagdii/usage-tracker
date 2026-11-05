import { afterEach, describe, expect, it, vi } from "vitest";
import { isPossibleSurpriseReset, observeBuckets } from "./history";
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
