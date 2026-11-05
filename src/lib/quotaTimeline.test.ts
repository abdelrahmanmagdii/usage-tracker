import { describe, expect, it } from "vitest";
import {
  buildTimeline,
  describeTimeline,
  evenPaceAt,
  readoutAt,
  timelineStops,
  timeTicks,
} from "./quotaTimeline";
import type { QuotaSnapshot } from "../types/codex";

const now = Date.parse("2026-10-03T12:00:00Z");
const HOUR = 3_600_000;
// A 5-hour window that started two hours ago and renews in three.
const resetsAt = (now + 3 * HOUR) / 1000;
const bucket = { remainingPercent: 70, resetsAt, windowDurationMins: 300 };
const sample = (minsAgo: number, used: number, reset = resetsAt): QuotaSnapshot => ({
  timestamp: new Date(now - minsAgo * 60_000).toISOString(),
  limitId: "codex:primary",
  usedPercent: used,
  remainingPercent: 100 - used,
  windowDurationMins: 300,
  resetsAt: reset,
});

describe("buildTimeline", () => {
  it("spans the current window on a time axis and ends at the live value", () => {
    const timeline = buildTimeline([sample(110, 5), sample(60, 20), sample(30, 30)], bucket, now)!;
    expect(timeline.startMs).toBe(now - 2 * HOUR);
    expect(timeline.endMs).toBe(now + 3 * HOUR);
    expect(timeline.points.map((p) => [(p.t - now) / 60_000, p.remaining])).toEqual([
      [-110, 95],
      [-60, 80],
      [-30, 70],
      [0, 70],
    ]);
    expect(timeline.hasEvenPace).toBe(true);
  });

  it("drops samples from the previous window and repeated values", () => {
    const previous = resetsAt - 5 * 3600;
    const timeline = buildTimeline(
      [sample(200, 80, previous), sample(150, 95, previous), sample(100, 10), sample(90, 10), sample(40, 30)],
      bucket,
      now,
    )!;
    expect(timeline.points.map((p) => p.remaining)).toEqual([90, 70, 70]);
  });

  it("keeps a window together when its reset time wobbles by a few seconds", () => {
    const timeline = buildTimeline([sample(100, 10, resetsAt - 2), sample(40, 30, resetsAt + 1)], bucket, now)!;
    expect(timeline.points.map((p) => p.remaining)).toEqual([90, 70, 70]);
  });

  it("still draws the guide and the current point with no history", () => {
    const timeline = buildTimeline([], bucket, now)!;
    expect(timeline.points).toEqual([{ t: now, remaining: 70 }]);
    expect(evenPaceAt(timeline, now)).toBeCloseTo(60);
  });

  it("starts at the first reading when the window length is unknown", () => {
    const unknown = { remainingPercent: 70, resetsAt, windowDurationMins: undefined };
    expect(buildTimeline([], unknown, now)).toBeNull();
    const timeline = buildTimeline([sample(90, 10), sample(30, 30)], unknown, now)!;
    expect(timeline.startMs).toBe(now - 90 * 60_000);
    expect(timeline.hasEvenPace).toBe(false);
    expect(evenPaceAt(timeline, now)).toBeNull();
  });

  it("has no time axis without a reset time", () => {
    expect(buildTimeline([sample(30, 30)], { ...bucket, resetsAt: undefined }, now)).toBeNull();
  });

  it("only keeps a run-out that lands between now and the reset", () => {
    expect(buildTimeline([], bucket, now, now + HOUR)!.runOutMs).toBe(now + HOUR);
    expect(buildTimeline([], bucket, now, now + 4 * HOUR)!.runOutMs).toBeNull();
  });

  it("handles long windows", () => {
    const monthly = { remainingPercent: 60, resetsAt: (now + 18 * 24 * HOUR) / 1000, windowDurationMins: 43_200 };
    const timeline = buildTimeline([], monthly, now)!;
    expect(timeline.endMs - timeline.startMs).toBe(30 * 24 * HOUR);
    expect(evenPaceAt(timeline, now)).toBeCloseTo(60);
  });
});

describe("readoutAt", () => {
  const timeline = buildTimeline([sample(100, 10), sample(40, 30)], bucket, now, now + 2 * HOUR)!;

  it("holds each reading until the next one (step, not interpolation)", () => {
    expect(readoutAt(timeline, now - 70 * 60_000)).toMatchObject({ remaining: 90, kind: "recorded" });
    expect(readoutAt(timeline, now - 40 * 60_000)).toMatchObject({ remaining: 70, kind: "recorded" });
  });

  it("says when nothing was recorded yet", () => {
    expect(readoutAt(timeline, now - 110 * 60_000)).toMatchObject({ remaining: null, kind: "unrecorded" });
  });

  it("follows the projection after now, reaching zero at run-out", () => {
    expect(readoutAt(timeline, now)).toMatchObject({ remaining: 70, kind: "now" });
    expect(readoutAt(timeline, now + HOUR).remaining).toBeCloseTo(35);
    expect(readoutAt(timeline, now + 2 * HOUR).remaining).toBe(0);
  });

  it("has no value ahead of now without a projection", () => {
    const calm = buildTimeline([sample(40, 30)], bucket, now)!;
    expect(readoutAt(calm, now + HOUR)).toMatchObject({ remaining: null, kind: "ahead" });
  });

  it("steps through each change, now, and the run-out", () => {
    expect(timelineStops(timeline)).toEqual([now - 100 * 60_000, now - 40 * 60_000, now, now + 2 * HOUR]);
  });
});

describe("describeTimeline", () => {
  it("compares with even pace and repeats the run-out warning", () => {
    const timeline = buildTimeline([sample(100, 10), sample(40, 30)], bucket, now, now + 2 * HOUR)!;
    const text = describeTimeline(timeline);
    expect(text).toContain("40% of this window has passed and 70% is left, 10 points more than an even pace would leave.");
    expect(text).toContain("20 points used since");
    expect(text).toContain("runs out in 2h, before the reset");
  });

  it("calls out burning faster than even pace", () => {
    const timeline = buildTimeline([sample(100, 10)], { ...bucket, remainingPercent: 40 }, now)!;
    expect(describeTimeline(timeline)).toContain("20 points less than an even pace would leave");
  });
});

describe("timeTicks", () => {
  it("keeps the number of guides small for any window length", () => {
    for (const span of [5 * HOUR, 24 * HOUR, 7 * 24 * HOUR, 30 * 24 * HOUR]) {
      const ticks = timeTicks(now, now + span);
      expect(ticks.length).toBeGreaterThan(0);
      expect(ticks.length).toBeLessThanOrEqual(8);
      expect(ticks.every((t) => t > now && t < now + span)).toBe(true);
    }
  });
});
