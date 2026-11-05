import type { QuotaSnapshot, RateLimitBucket } from "../types/codex";
import { currentWindowSamples } from "./history";
import { formatLeadTime } from "./time";

/** One reading: `remaining` holds from `t` until the next point. */
export type TimelinePoint = { t: number; remaining: number };

/**
 * Everything the quota chart draws, in epoch ms and percent remaining. The
 * x domain is the current window itself (start → reset), so position on the
 * chart is time, not sample index.
 */
export type QuotaTimeline = {
  startMs: number;
  endMs: number;
  /** `now`, clamped into the window. Always the last point's time. */
  nowMs: number;
  /** Step series from the first recorded reading to now (live value last). */
  points: TimelinePoint[];
  /**
   * Whether the window's length is known, so "even pace" (using the quota
   * evenly from start to reset) can be drawn and compared against.
   */
  hasEvenPace: boolean;
  /** Projected run-out before the reset, from `projectedRunOut`, or null. */
  runOutMs: number | null;
};

const clampPercent = (value: number) => Math.min(100, Math.max(0, value));

/**
 * Builds the chart model for the window that renews at `bucket.resetsAt`.
 * Samples from earlier windows are ignored. Returns null when there is no
 * time axis to draw against (no reset time, or an unknown window length with
 * nothing recorded yet).
 */
export function buildTimeline(
  samples: readonly QuotaSnapshot[],
  bucket: Pick<RateLimitBucket, "remainingPercent" | "resetsAt" | "windowDurationMins">,
  nowMs: number,
  runOutMs: number | null = null,
): QuotaTimeline | null {
  if (!bucket.resetsAt || !Number.isFinite(bucket.resetsAt)) return null;
  const endMs = bucket.resetsAt * 1000;
  const window = currentWindowSamples(samples, bucket.resetsAt);
  const durationMs =
    bucket.windowDurationMins && bucket.windowDurationMins > 0 ? bucket.windowDurationMins * 60_000 : null;
  // Without a known length the chart starts at the first reading instead.
  const startMs = durationMs ? endMs - durationMs : window.length ? Date.parse(window[0].timestamp) : null;
  if (startMs === null || endMs - startMs < 60_000) return null;
  const now = Math.min(endMs, Math.max(startMs, nowMs));

  // History is only written when usage changes, so each reading holds until
  // the next one: consecutive equal values add nothing to a step line.
  const points: TimelinePoint[] = [];
  for (const sample of window) {
    const t = Math.max(startMs, Date.parse(sample.timestamp));
    if (t > now) break;
    const remaining = clampPercent(sample.remainingPercent);
    if (points.length && points[points.length - 1].remaining === remaining) continue;
    points.push({ t, remaining });
  }
  const live = clampPercent(bucket.remainingPercent);
  // The live reading always closes the line at "now", even when it equals
  // the last sample, so the hold up to now is drawn.
  if (points.length && points[points.length - 1].t === now) points[points.length - 1].remaining = live;
  else points.push({ t: now, remaining: live });
  if (!durationMs && points.length < 2) return null;

  return {
    startMs,
    endMs,
    nowMs: now,
    points,
    hasEvenPace: durationMs !== null,
    runOutMs: runOutMs !== null && runOutMs > now && runOutMs < endMs ? runOutMs : null,
  };
}

/** Percent that would be left at `t` if the quota were used evenly. */
export function evenPaceAt(timeline: QuotaTimeline, t: number): number | null {
  if (!timeline.hasEvenPace) return null;
  return clampPercent((100 * (timeline.endMs - t)) / (timeline.endMs - timeline.startMs));
}

export type Readout = {
  t: number;
  /** Remaining at `t`, or null before the first reading / with no projection. */
  remaining: number | null;
  kind: "recorded" | "now" | "projected" | "runout" | "unrecorded" | "ahead";
  evenPace: number | null;
};

/** What the chart says at time `t`: recorded, live, projected, or unknown. */
export function readoutAt(timeline: QuotaTimeline, t: number): Readout {
  const { points, nowMs, runOutMs } = timeline;
  const evenPace = evenPaceAt(timeline, t);
  if (t < points[0].t) return { t, remaining: null, kind: "unrecorded", evenPace };
  if (t >= nowMs) {
    const live = points[points.length - 1].remaining;
    if (t === nowMs) return { t, remaining: live, kind: "now", evenPace };
    if (runOutMs !== null) {
      const share = Math.min(1, (t - nowMs) / (runOutMs - nowMs));
      return { t, remaining: live * (1 - share), kind: t === runOutMs ? "runout" : "projected", evenPace };
    }
    return { t, remaining: null, kind: "ahead", evenPace };
  }
  let value = points[0].remaining;
  for (const point of points) {
    if (point.t > t) break;
    value = point.remaining;
  }
  return { t, remaining: value, kind: "recorded", evenPace };
}

/** Times worth stepping through with the keyboard: each change, now, run-out. */
export function timelineStops(timeline: QuotaTimeline): number[] {
  const stops = timeline.points.map((point) => point.t);
  if (timeline.runOutMs !== null) stops.push(timeline.runOutMs);
  return stops;
}

const HOUR = 3_600_000;
const DAY = 24 * HOUR;

/** Axis/tooltip time, as precise as the window's length calls for. */
export function formatChartTime(ms: number, spanMs: number, detailed = false): string {
  const date = new Date(ms);
  if (spanMs <= 26 * HOUR) return date.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
  if (spanMs <= 8 * DAY) {
    return date.toLocaleString(undefined, detailed
      ? { weekday: "short", hour: "numeric", minute: "2-digit" }
      : { weekday: "short", hour: "numeric" });
  }
  return date.toLocaleString(undefined, detailed
    ? { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" }
    : { month: "short", day: "numeric" });
}

/**
 * Faint vertical guides on local clock boundaries, spaced so a window gets
 * at most ~8: hours on a 5-hour window, midnights on a week, Mondays on a
 * month. They make idle nights and busy afternoons legible at a glance.
 */
export function timeTicks(startMs: number, endMs: number): number[] {
  const span = endMs - startMs;
  const ticks: number[] = [];
  const cursor = new Date(startMs);
  if (span <= 26 * HOUR) {
    const every = [1, 2, 3, 6].find((hours) => span / (hours * HOUR) <= 8) ?? 6;
    cursor.setMinutes(0, 0, 0);
    while (cursor.getTime() <= startMs || cursor.getHours() % every !== 0) cursor.setHours(cursor.getHours() + 1);
    for (; cursor.getTime() < endMs; cursor.setHours(cursor.getHours() + every)) ticks.push(cursor.getTime());
    return ticks;
  }
  const weekly = span > 8 * DAY;
  cursor.setHours(0, 0, 0, 0);
  while (cursor.getTime() <= startMs || (weekly && cursor.getDay() !== 1)) cursor.setDate(cursor.getDate() + 1);
  for (; cursor.getTime() < endMs; cursor.setDate(cursor.getDate() + (weekly ? 7 : 1))) ticks.push(cursor.getTime());
  return ticks;
}

/**
 * The trend in one or two sentences, for screen readers: how much of the
 * window has passed, how much is left, how that compares with even pace,
 * and whether the current pace runs out before the reset.
 */
export function describeTimeline(timeline: QuotaTimeline): string {
  const { startMs, endMs, nowMs, points, runOutMs } = timeline;
  const remaining = Math.round(points[points.length - 1].remaining);
  const parts: string[] = [];
  const even = evenPaceAt(timeline, nowMs);
  if (even !== null) {
    const elapsed = Math.round((100 * (nowMs - startMs)) / (endMs - startMs));
    const gap = Math.round(remaining - even);
    const comparison =
      Math.abs(gap) < 3
        ? "about where an even pace would be"
        : gap > 0
          ? `${gap} points more than an even pace would leave`
          : `${-gap} points less than an even pace would leave`;
    parts.push(`${elapsed}% of this window has passed and ${remaining}% is left, ${comparison}.`);
  } else {
    parts.push(`${remaining}% left.`);
  }
  if (points.length > 1) {
    const used = Math.round(points[0].remaining - remaining);
    const since = formatChartTime(points[0].t, endMs - startMs, true);
    if (used > 0) parts.push(`${used} points used since ${since}.`);
    else if (used === 0) parts.push(`No change since ${since}.`);
  }
  if (runOutMs !== null) parts.push(`At this pace it runs out in ${formatLeadTime(runOutMs - nowMs)}, before the reset.`);
  return parts.join(" ");
}
