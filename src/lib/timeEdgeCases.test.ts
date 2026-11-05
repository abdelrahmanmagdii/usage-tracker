import { describe, expect, it } from "vitest";
import { formatLeadTime, freshness, isLiveReading, relativeTime } from "./time";
import { formatCountdown, formatLocalReset, windowDurationLabel } from "./rateLimits";

describe("time helpers on malformed input", () => {
  const now = Date.parse("2026-08-16T12:00:00Z");

  it("treats an unparseable timestamp as no sighting instead of NaN", () => {
    expect(relativeTime("not a date", now)).toBe("No reset sightings yet");
    expect(relativeTime("2026-13-45T99:00:00Z", now)).not.toContain("NaN");
  });

  it("never prints NaN for a lead time computed from a bad timestamp", () => {
    expect(formatLeadTime(Number.NaN)).toBe("soon");
    expect(formatLeadTime(Date.parse("garbage") - now)).not.toContain("NaN");
  });

  it("keeps formatting long and past lead times sensibly", () => {
    expect(formatLeadTime(-5 * 60_000)).toBe("1m");
    expect(formatLeadTime(36 * 3_600_000)).toBe("1 day");
    expect(formatLeadTime(30 * 86_400_000)).toBe("30 days");
  });

  it("flags a reading stale at exactly fifteen minutes", () => {
    expect(freshness(now / 1_000 - 15 * 60 + 1, now)?.stale).toBe(false);
    expect(freshness(now / 1_000 - 15 * 60, now)?.stale).toBe(true);
  });
});

describe("formatCountdown edge cases", () => {
  const now = Date.UTC(2026, 7, 12, 20, 0, 0);

  it("says plainly when the reset time is missing or unusable", () => {
    expect(formatCountdown(undefined, now)).toBe("Reset time not reported");
    expect(formatCountdown(0, now)).toBe("Reset time not reported");
    expect(formatCountdown(Number.NaN, now)).toBe("Reset time not reported");
  });

  it("reports a reset that is already due rather than a negative countdown", () => {
    expect(formatCountdown(now / 1000 - 600, now)).toBe("Reset due");
    expect(formatCountdown(now / 1000, now)).toBe("Reset due");
  });

  it("counts very long windows in days", () => {
    expect(formatCountdown(now / 1000 + 30 * 86_400 + 3_600, now)).toBe("Resets in 30d 1h");
  });

  it("labels unknown durations generically", () => {
    expect(windowDurationLabel(undefined)).toBe("Limit");
    expect(windowDurationLabel(Number.NaN)).toBe("Limit");
    expect(windowDurationLabel(-5)).toBe("Limit");
    expect(windowDurationLabel(43_200)).toBe("30-day");
  });
});

describe("formatLocalReset", () => {
  // Local-time maths: build both instants from local components so the test
  // holds in any timezone the suite runs in.
  const at = (y: number, m: number, d: number, h: number, min = 0) =>
    new Date(y, m, d, h, min).getTime() / 1000;
  const now = new Date(2026, 7, 12, 10, 0); // Wednesday 12 Aug 2026, 10:00 local

  it("names today, tonight, and tomorrow", () => {
    expect(formatLocalReset(at(2026, 7, 12, 14), now)).toMatch(/^Today · /);
    expect(formatLocalReset(at(2026, 7, 12, 21), now)).toMatch(/^Tonight · /);
    expect(formatLocalReset(at(2026, 7, 13, 9), now)).toMatch(/^Tomorrow · /);
  });

  it("uses a bare weekday only within the coming week", () => {
    const friday = formatLocalReset(at(2026, 7, 14, 9), now);
    expect(friday.split(" · ")[0]).toBe(new Date(2026, 7, 14).toLocaleDateString(undefined, { weekday: "long" }));
  });

  it("dates a reset more than a week out so it is not read as this week", () => {
    const label = formatLocalReset(at(2026, 8, 2, 9), now).split(" · ")[0];
    expect(label).not.toBe(new Date(2026, 8, 2).toLocaleDateString(undefined, { weekday: "long" }));
    expect(label).toBe(
      new Date(2026, 8, 2).toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" }),
    );
  });

  it("does not present a reset time already behind us as an upcoming weekday", () => {
    expect(formatLocalReset(at(2026, 7, 11, 9), now)).toMatch(/^Yesterday · /);
    const lastWeek = formatLocalReset(at(2026, 7, 7, 9), now).split(" · ")[0];
    expect(lastWeek).not.toBe(new Date(2026, 7, 7).toLocaleDateString(undefined, { weekday: "long" }));
  });

  it("handles the day after a DST change as tomorrow", () => {
    // US and EU both change clocks in late October / early November and March.
    expect(formatLocalReset(at(2026, 10, 1, 9), new Date(2026, 9, 31, 23, 30))).toMatch(/^Tomorrow · /);
    expect(formatLocalReset(at(2026, 2, 29, 9), new Date(2026, 2, 28, 23, 30))).toMatch(/^Tomorrow · /);
  });

  it("returns nothing for a missing or out-of-range timestamp", () => {
    expect(formatLocalReset(undefined, now)).toBe("");
    expect(formatLocalReset(Number.NaN, now)).toBe("");
    expect(formatLocalReset(1e20, now)).toBe("");
  });
});

describe("isLiveReading", () => {
  const now = Date.parse("2026-11-05T15:00:00Z");
  it("is live only when connected with a reading under 15 minutes old", () => {
    expect(isLiveReading("connected", now / 1000 - 60, now)).toBe(true);
    expect(isLiveReading("connected", now / 1000 - 15 * 60, now)).toBe(false);
    expect(isLiveReading("error", now / 1000 - 60, now)).toBe(false);
    expect(isLiveReading("not_authenticated", now / 1000 - 60, now)).toBe(false);
    expect(isLiveReading("connected", undefined, now)).toBe(false);
    expect(isLiveReading("connected", null, now)).toBe(false);
  });
});
