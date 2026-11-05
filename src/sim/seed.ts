/**
 * Local-storage seeding: the quota history the popover's per-window charts
 * read (`codex-meter.quota-history.v1`, written by src/lib/history.ts).
 *
 * Samples come from the same model the simulated backend serves, so the
 * history leads straight into the live numbers. Bucket ids are computed with
 * the app's own `extractRateLimitBuckets`, so they match whatever key the
 * charts look up.
 */
import { extractRateLimitBuckets } from "../lib/rateLimits";
import { SIM_PROVIDERS, type SimModel, type SimProvider } from "./model";

export const HISTORY_KEY = "codex-meter.quota-history.v1";
const MAX_HISTORY = 900; // src/lib/history.ts keeps the newest 900 samples

/** Keys the app owns. Cleared at boot so every run starts the same way. */
export function clearAppStorage(): void {
  try {
    for (const key of Object.keys(localStorage)) {
      if (key.startsWith("codex-meter.") || key.startsWith("usagebar")) localStorage.removeItem(key);
    }
  } catch {
    /* storage unavailable: the app copes, so does the harness */
  }
}

type Sample = {
  timestamp: string;
  limitId: string;
  usedPercent: number;
  remainingPercent: number;
  windowDurationMins?: number;
  resetsAt?: number;
};

export function buildHistory(
  model: SimModel,
  providers: SimProvider[],
  { hours, stepMinutes, endSecs }: { hours: number; stepMinutes: number; endSecs: number },
): Sample[] {
  const samples: Sample[] = [];
  const last = new Map<string, Sample>();
  const step = Math.max(1, Math.round(stepMinutes * 60));
  const begin = endSecs - Math.round(hours * 3_600);
  for (let t = begin; t <= endSecs; t += step) {
    for (const provider of providers) {
      if (model.connection(provider, t).connection !== "connected") continue;
      const buckets = extractRateLimitBuckets(model.rateLimits(provider, t));
      for (const bucket of buckets) {
        const sample: Sample = {
          timestamp: new Date(t * 1000).toISOString(),
          limitId: `${provider}:${bucket.id}`,
          usedPercent: bucket.usedPercent,
          remainingPercent: bucket.remainingPercent,
          windowDurationMins: bucket.windowDurationMins,
          resetsAt: bucket.resetsAt,
        };
        const previous = last.get(sample.limitId);
        // history.ts records only meaningful changes; so does the seed.
        if (previous && previous.usedPercent === sample.usedPercent && previous.resetsAt === sample.resetsAt) continue;
        last.set(sample.limitId, sample);
        samples.push(sample);
      }
    }
  }
  return samples.slice(-MAX_HISTORY);
}

export function seedHistory(model: SimModel, options: { hours: number; stepMinutes: number; endSecs: number }): number {
  const providers = SIM_PROVIDERS.filter((provider) => model.spec(provider).connection !== "cli_not_found");
  const samples = buildHistory(model, providers, options);
  try {
    localStorage.setItem(HISTORY_KEY, JSON.stringify(samples));
  } catch {
    return 0;
  }
  return samples.length;
}
