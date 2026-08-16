import { describe, expect, it } from "vitest";
import { describeAge, formatLeadTime, relativeTime } from "./time";

describe("describeAge", () => {
  const now = Date.parse("2026-08-16T12:00:00Z");

  it("reads the age of last-known numbers in the largest useful unit", () => {
    expect(describeAge(now / 1_000 - 20, now)).toBe("just now");
    expect(describeAge(now / 1_000 - 42 * 60, now)).toBe("42m ago");
    expect(describeAge(now / 1_000 - 5 * 3_600, now)).toBe("5h ago");
    expect(describeAge(now / 1_000 - 3 * 86_400, now)).toBe("3d ago");
  });

  it("stays silent when the meter has never had numbers", () => {
    expect(describeAge(undefined, now)).toBeNull();
    expect(describeAge(null, now)).toBeNull();
    expect(describeAge(Number.NaN, now)).toBeNull();
  });

  it("does not report a negative age when the clock jumps backwards", () => {
    expect(describeAge(now / 1_000 + 600, now)).toBe("just now");
  });
});

describe("existing time helpers", () => {
  it("keeps relative and lead-time formatting intact", () => {
    const now = Date.parse("2026-08-16T12:00:00Z");
    expect(relativeTime(undefined, now)).toBe("No reset sightings yet");
    expect(relativeTime("2026-08-16T09:30:00Z", now)).toBe("2h ago");
    expect(formatLeadTime(45 * 60_000)).toBe("45m");
    expect(formatLeadTime(2 * 3_600_000 + 15 * 60_000)).toBe("2h 15m");
  });
});
