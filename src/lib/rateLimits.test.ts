import { describe, expect, it } from "vitest";
import {
  extractRateLimitBuckets,
  extractResetCredits,
  extractUsage,
  formatCountdown,
  formatLocalReset,
  remainingFromUsed,
  windowDurationLabel,
} from "./rateLimits";

describe("rate-limit helpers", () => {
  it("converts used percentage to a clamped remaining percentage", () => {
    expect(remainingFromUsed(32)).toBe(68);
    expect(remainingFromUsed(140)).toBe(0);
    expect(remainingFromUsed(-12)).toBe(100);
  });

  it("labels known and generic window durations", () => {
    expect(windowDurationLabel(300)).toBe("5-hour");
    expect(windowDurationLabel(10_080)).toBe("Weekly");
    expect(windowDurationLabel(2_880)).toBe("2-day");
    expect(windowDurationLabel(45)).toBe("45-minute");
  });

  it("formats Unix reset timestamps without polling", () => {
    const now = Date.UTC(2026, 7, 12, 20, 0, 0);
    expect(formatCountdown(now / 1000 + 7_320, now)).toBe("Resets in 2h 2m");
    expect(formatCountdown(now / 1000 + 125, now)).toBe("Resets in 2m 05s");
    expect(formatLocalReset(now / 1000 + 3_600, new Date(now))).toContain("·");
  });

  it("supports multiple buckets and a null secondary", () => {
    const buckets = extractRateLimitBuckets({
      rateLimitsByLimitId: {
        codex: {
          limitId: "codex",
          primary: { usedPercent: 20, windowDurationMins: 300, resetsAt: 100 },
          secondary: null,
        },
        review: {
          limitId: "review",
          limitName: "Code review",
          primary: { usedPercent: 70, windowDurationMins: 10_080, resetsAt: 200 },
          secondary: { usedPercent: 10, windowDurationMins: 60, resetsAt: 300 },
        },
      },
    });
    expect(buckets).toHaveLength(3);
    expect(buckets.map((bucket) => bucket.remainingPercent)).toEqual([80, 30, 90]);
  });

  it("falls back to the legacy single bucket and tolerates malformed payloads", () => {
    expect(extractRateLimitBuckets(null)).toEqual([]);
    expect(extractRateLimitBuckets({ rateLimitsByLimitId: { broken: "oops" } })).toEqual([]);
    expect(
      extractRateLimitBuckets({
        rateLimits: {
          primary: { usedPercent: "25", windowDurationMins: 300 },
          secondary: null,
        },
      })[0]?.remainingPercent,
    ).toBe(75);
  });

  it("parses reset credits and partial usage defensively", () => {
    expect(
      extractResetCredits({ rateLimitResetCredits: { availableCount: "2" } }),
    ).toEqual({ availableCount: 2 });
    expect(extractResetCredits({})).toEqual({ availableCount: 0 });
    expect(
      extractUsage({
        summary: { lifetimeTokens: "1200", currentStreakDays: null },
        dailyUsageBuckets: [{ startDate: "2026-08-12", tokens: 42 }, null],
      }),
    ).toMatchObject({ lifetimeTokens: 1200, dailyUsage: [{ tokens: 42 }] });
  });
});
