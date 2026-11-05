import type { QuotaSnapshot, RateLimitBucket, ResetEvent } from "../types/codex";

const HISTORY_KEY = "codex-meter.quota-history.v1";
const EVENT_KEY = "codex-meter.detected-events.v1";
// Six providers × a few windows each fill the cap faster than one did, and
// the store is only ~60KB of JSON at this size.
const MAX_HISTORY = 900;

function parseArray<T>(raw: string | null): T[] {
  try {
    const value = JSON.parse(raw ?? "[]");
    return Array.isArray(value) ? (value as T[]) : [];
  } catch {
    return [];
  }
}

function readArray<T>(key: string): T[] {
  try {
    return parseArray<T>(localStorage.getItem(key));
  } catch {
    return [];
  }
}

// Every quota tile reads its own series, so parsing the whole store per tile
// re-parsed ~60KB of JSON a dozen times per refresh. The parsed store is kept
// keyed by the exact raw string: our own writes refresh it directly, and a
// write from another window (popover vs. dashboard) shows up as a different
// string, so it can never serve stale samples. Comparing strings is far
// cheaper than JSON.parse.
let historyCache: { raw: string | null; byLimit: Map<string, QuotaSnapshot[]> } | null = null;

function indexByLimit(history: QuotaSnapshot[]): Map<string, QuotaSnapshot[]> {
  const byLimit = new Map<string, QuotaSnapshot[]>();
  for (const entry of history) {
    if (!entry || typeof entry.limitId !== "string") continue;
    const list = byLimit.get(entry.limitId);
    if (list) list.push(entry);
    else byLimit.set(entry.limitId, [entry]);
  }
  return byLimit;
}

function cachedHistory(): Map<string, QuotaSnapshot[]> {
  let raw: string | null;
  try {
    raw = localStorage.getItem(HISTORY_KEY);
  } catch {
    raw = null;
  }
  if (!historyCache || historyCache.raw !== raw) {
    historyCache = { raw, byLimit: indexByLimit(parseArray<QuotaSnapshot>(raw)) };
  }
  return historyCache.byLimit;
}

export function isPossibleSurpriseReset(
  previous: QuotaSnapshot,
  current: QuotaSnapshot,
): boolean {
  const observedAt = Date.parse(current.timestamp) / 1000;
  return (
    previous.limitId === current.limitId &&
    previous.windowDurationMins === current.windowDurationMins &&
    previous.resetsAt !== undefined &&
    observedAt < previous.resetsAt - 60 &&
    current.remainingPercent - previous.remainingPercent >= 40 &&
    current.usedPercent <= 40
  );
}

export function observeBuckets(provider: string, buckets: RateLimitBucket[]): ResetEvent[] {
  const history = readArray<QuotaSnapshot>(HISTORY_KEY);
  const detected = readArray<ResetEvent>(EVENT_KEY);
  const newEvents: ResetEvent[] = [];
  const timestamp = new Date().toISOString();

  for (const bucket of buckets) {
    const snapshot: QuotaSnapshot = {
      timestamp,
      // Bucket ids are only unique within one provider ("weekly" exists on
      // both OpenCode and Devin), so history keys carry the provider too.
      limitId: `${provider}:${bucket.id}`,
      usedPercent: bucket.usedPercent,
      remainingPercent: bucket.remainingPercent,
      windowDurationMins: bucket.windowDurationMins,
      resetsAt: bucket.resetsAt,
    };
    const previous = [...history]
      .reverse()
      .find((entry) => entry.limitId === snapshot.limitId);
    const meaningful =
      !previous ||
      previous.usedPercent !== snapshot.usedPercent ||
      previous.resetsAt !== snapshot.resetsAt;
    if (!meaningful) continue;
    if (previous && isPossibleSurpriseReset(previous, snapshot)) {
      const event: ResetEvent = {
        id: `detected-${Date.now()}-${bucket.id}`,
        announcedAt: timestamp,
        occurredAt: timestamp,
        source: "detected",
        text: "Possible surprise reset detected locally",
      };
      detected.push(event);
      newEvents.push(event);
    }
    history.push(snapshot);
  }

  // History only feeds sparklines and reset detection. A full or blocked
  // store must not throw into the hook that applies fresh meter state.
  try {
    const kept = history.slice(-MAX_HISTORY);
    const raw = JSON.stringify(kept);
    localStorage.setItem(HISTORY_KEY, raw);
    // Seed the read cache from what was just written so the tiles that
    // re-render for this observation do not parse it back again.
    historyCache = { raw, byLimit: indexByLimit(kept) };
    localStorage.setItem(EVENT_KEY, JSON.stringify(detected.slice(-100)));
  } catch {
    /* The live meter is already updated; this sample is simply not kept. */
  }
  return newEvents;
}

export function readDetectedEvents(): ResetEvent[] {
  return readArray<ResetEvent>(EVENT_KEY);
}

/**
 * Recorded samples for one quota, oldest first. The array is shared with the
 * read cache, so callers must treat it as read-only.
 */
export function historySeries(provider: string, bucketId: string): readonly QuotaSnapshot[] {
  return cachedHistory().get(`${provider}:${bucketId}`) ?? [];
}

// Reset timestamps can wobble by a few seconds between polls (some providers
// report fractional or rounded times). A renewal moves them by a whole
// window, so a small tolerance keeps one window together without ever
// merging two.
const SAME_WINDOW_TOLERANCE_S = 90;

/** Whether a sample belongs to the window that renews at `resetsAt`. */
export function inSameWindow(sample: Pick<QuotaSnapshot, "resetsAt">, resetsAt?: number): boolean {
  return (
    resetsAt !== undefined &&
    sample.resetsAt !== undefined &&
    Math.abs(sample.resetsAt - resetsAt) <= SAME_WINDOW_TOLERANCE_S
  );
}

/**
 * Samples from the window that renews at `resetsAt`, oldest first. Anything
 * from an earlier window is dropped, so a renewal never reads as negative
 * usage or bleeds into the current window's chart.
 */
export function currentWindowSamples(
  samples: readonly QuotaSnapshot[],
  resetsAt: number | undefined,
): QuotaSnapshot[] {
  return samples
    .filter((s) => inSameWindow(s, resetsAt) && Number.isFinite(Date.parse(s.timestamp)))
    .sort((a, b) => Date.parse(a.timestamp) - Date.parse(b.timestamp));
}

// Look back this far for the burn rate: long enough to smooth one burst,
// short enough to follow today's pace rather than last week's.
const PACE_LOOKBACK_MS = 3 * 3_600_000;
const PACE_MIN_SPAN_MS = 10 * 60_000;

/**
 * When the window runs dry at the recent burn rate, if that is before it
 * renews. Only samples from the current window count, so a renewal never
 * reads as negative usage. Returns epoch ms or null.
 */
export function projectedRunOut(
  samples: readonly QuotaSnapshot[],
  bucket: Pick<RateLimitBucket, "usedPercent" | "resetsAt" | "reached">,
  nowMs = Date.now(),
): number | null {
  if (bucket.reached || !bucket.resetsAt || bucket.usedPercent >= 100) return null;
  const window = currentWindowSamples(samples, bucket.resetsAt).filter(
    (s) => nowMs - Date.parse(s.timestamp) <= PACE_LOOKBACK_MS,
  );
  if (window.length < 2) return null;
  const first = window[0];
  const firstMs = Date.parse(first.timestamp);
  const spanMs = nowMs - firstMs;
  const usedDelta = bucket.usedPercent - first.usedPercent;
  if (!Number.isFinite(firstMs) || spanMs < PACE_MIN_SPAN_MS || usedDelta <= 0) return null;
  const runOut = nowMs + ((100 - bucket.usedPercent) / usedDelta) * spanMs;
  return runOut < bucket.resetsAt * 1000 ? runOut : null;
}
