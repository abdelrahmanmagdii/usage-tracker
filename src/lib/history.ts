import type { QuotaSnapshot, RateLimitBucket, ResetEvent } from "../types/codex";

const HISTORY_KEY = "codex-meter.quota-history.v1";
const EVENT_KEY = "codex-meter.detected-events.v1";
// Six providers × a few windows each fill the cap faster than one did, and
// the store is only ~60KB of JSON at this size.
const MAX_HISTORY = 900;

function readArray<T>(key: string): T[] {
  try {
    const value = JSON.parse(localStorage.getItem(key) ?? "[]");
    return Array.isArray(value) ? (value as T[]) : [];
  } catch {
    return [];
  }
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

  localStorage.setItem(HISTORY_KEY, JSON.stringify(history.slice(-MAX_HISTORY)));
  localStorage.setItem(EVENT_KEY, JSON.stringify(detected.slice(-100)));
  return newEvents;
}

export function readDetectedEvents(): ResetEvent[] {
  return readArray<ResetEvent>(EVENT_KEY);
}

/** Recorded samples for one window, oldest first, for the tile sparkline. */
export function historySeries(provider: string, bucketId: string): QuotaSnapshot[] {
  const key = `${provider}:${bucketId}`;
  return readArray<QuotaSnapshot>(HISTORY_KEY).filter((entry) => entry.limitId === key);
}
