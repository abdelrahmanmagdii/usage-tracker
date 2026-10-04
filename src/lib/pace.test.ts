import { describe, expect, it } from "vitest";
import { projectedRunOut } from "./history";
import { freshness } from "./time";
import { shouldLoad } from "../features/tibo-watch/useResetEvents";
import type { QuotaSnapshot } from "../types/codex";

const now = Date.parse("2026-10-03T12:00:00Z");
const resetsAt = now / 1000 + 4 * 3600;
const sample = (minsAgo: number, used: number, reset = resetsAt): QuotaSnapshot => ({
  timestamp: new Date(now - minsAgo * 60_000).toISOString(),
  limitId: "codex:primary",
  usedPercent: used,
  remainingPercent: 100 - used,
  resetsAt: reset,
});

describe("projectedRunOut", () => {
  it("projects exhaustion before the reset at the recent burn rate", () => {
    // 20% in the last hour, 40% left → empty in ~2h, before the 4h reset.
    const runOut = projectedRunOut([sample(60, 40), sample(30, 50)], { usedPercent: 60, resetsAt, reached: false }, now);
    expect(runOut).toBe(now + 2 * 3_600_000);
  });

  it("stays quiet when the window renews first", () => {
    expect(projectedRunOut([sample(60, 40), sample(0, 45)], { usedPercent: 45, resetsAt, reached: false }, now)).toBeNull();
  });

  it("ignores samples from a previous window and too-short spans", () => {
    expect(projectedRunOut([sample(60, 10, resetsAt - 18_000), sample(1, 50)], { usedPercent: 60, resetsAt, reached: false }, now)).toBeNull();
    expect(projectedRunOut([sample(5, 40), sample(1, 50)], { usedPercent: 60, resetsAt, reached: false }, now)).toBeNull();
  });
});

describe("freshness", () => {
  it("labels recent readings and flags stale ones", () => {
    expect(freshness(now / 1000 - 120, now)).toEqual({ label: "Updated 2m ago", stale: false });
    expect(freshness(now / 1000 - 20 * 60, now)).toEqual({ label: "Updated 20m ago", stale: true });
    expect(freshness(null, now)).toBeNull();
  });
});

describe("shouldLoad", () => {
  it("polls every five minutes, and on focus/wake after a short gap", () => {
    expect(shouldLoad(now - 4 * 60_000, now, false)).toBe(false);
    expect(shouldLoad(now - 5 * 60_000, now, false)).toBe(true);
    expect(shouldLoad(now - 10_000, now, true)).toBe(false);
    expect(shouldLoad(now - 45_000, now, true)).toBe(true);
  });
});
