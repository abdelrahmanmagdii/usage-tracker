import type {
  RateLimitBucket,
  ResetCredits,
  UsageSummary,
} from "../types/codex";

type JsonRecord = Record<string, unknown>;

export function isRecord(value: unknown): value is JsonRecord {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function finiteNumber(value: unknown): number | undefined {
  if (typeof value === "number" && Number.isFinite(value)) return value;
  if (typeof value === "string" && value.trim() !== "") {
    const parsed = Number(value);
    if (Number.isFinite(parsed)) return parsed;
  }
  return undefined;
}

export function remainingFromUsed(usedPercent: unknown): number {
  const used = finiteNumber(usedPercent) ?? 0;
  return Math.max(0, Math.min(100, 100 - used));
}

export function windowDurationLabel(minutes?: number): string {
  if (!minutes || minutes <= 0) return "Limit";
  if (minutes === 10_080) return "Weekly";
  if (minutes === 1_440) return "Daily";
  if (minutes % 10_080 === 0) return `${minutes / 10_080}-week`;
  if (minutes % 1_440 === 0) return `${minutes / 1_440}-day`;
  if (minutes % 60 === 0) return `${minutes / 60}-hour`;
  return `${minutes}-minute`;
}

function parseWindow(
  value: unknown,
  snapshot: JsonRecord,
  kind: "primary" | "secondary",
  fallbackId: string,
): RateLimitBucket | null {
  if (!isRecord(value)) return null;
  const usedPercent = finiteNumber(value.usedPercent);
  if (usedPercent === undefined) return null;
  const windowDurationMins = finiteNumber(value.windowDurationMins);
  const resetsAt = finiteNumber(value.resetsAt);
  const limitId =
    (typeof snapshot.limitId === "string" && snapshot.limitId) || fallbackId;
  const limitName =
    typeof snapshot.limitName === "string" ? snapshot.limitName : undefined;
  const windowLabel =
    typeof snapshot.windowLabel === "string" ? snapshot.windowLabel : undefined;
  return {
    id: `${limitId}:${kind}:${windowDurationMins ?? "unknown"}`,
    limitId,
    limitName,
    windowLabel,
    windowKind: kind,
    usedPercent: Math.max(0, Math.min(100, usedPercent)),
    remainingPercent: remainingFromUsed(usedPercent),
    windowDurationMins,
    resetsAt,
    reached:
      snapshot.rateLimitReachedType != null ||
      snapshot.spendControlReached === true ||
      usedPercent >= 100,
  };
}

function bucketsFromSnapshot(value: unknown, fallbackId: string): RateLimitBucket[] {
  if (!isRecord(value)) return [];
  return [
    parseWindow(value.primary, value, "primary", fallbackId),
    parseWindow(value.secondary, value, "secondary", fallbackId),
  ].filter((bucket): bucket is RateLimitBucket => bucket !== null);
}

export function extractRateLimitBuckets(payload: unknown): RateLimitBucket[] {
  if (!isRecord(payload)) return [];
  const byId = payload.rateLimitsByLimitId;
  if (isRecord(byId)) {
    const buckets = Object.entries(byId).flatMap(([limitId, snapshot]) =>
      bucketsFromSnapshot(snapshot, limitId),
    );
    if (buckets.length > 0) return buckets;
  }
  return bucketsFromSnapshot(payload.rateLimits, "codex");
}

export function extractResetCredits(payload: unknown): ResetCredits {
  if (!isRecord(payload) || !isRecord(payload.rateLimitResetCredits)) {
    return { availableCount: 0 };
  }
  return {
    availableCount: Math.max(
      0,
      Math.trunc(finiteNumber(payload.rateLimitResetCredits.availableCount) ?? 0),
    ),
  };
}

export function extractUsage(payload: unknown): UsageSummary | null {
  if (!isRecord(payload) || !isRecord(payload.summary)) return null;
  const summary = payload.summary;
  const dailyUsage = Array.isArray(payload.dailyUsageBuckets)
    ? payload.dailyUsageBuckets.flatMap((bucket) => {
        if (!isRecord(bucket) || typeof bucket.startDate !== "string") return [];
        const tokens = finiteNumber(bucket.tokens);
        return tokens === undefined ? [] : [{ startDate: bucket.startDate, tokens }];
      })
    : [];
  return {
    lifetimeTokens: finiteNumber(summary.lifetimeTokens),
    peakDailyTokens: finiteNumber(summary.peakDailyTokens),
    longestRunningTurnSec: finiteNumber(summary.longestRunningTurnSec),
    currentStreakDays: finiteNumber(summary.currentStreakDays),
    longestStreakDays: finiteNumber(summary.longestStreakDays),
    dailyUsage,
  };
}

export function formatCountdown(resetsAt?: number, nowMs = Date.now()): string {
  if (!resetsAt) return "Reset time unavailable";
  const seconds = Math.max(0, Math.floor(resetsAt - nowMs / 1000));
  if (seconds <= 0) return "Reset due";
  const days = Math.floor(seconds / 86_400);
  const hours = Math.floor((seconds % 86_400) / 3_600);
  const minutes = Math.floor((seconds % 3_600) / 60);
  const secs = seconds % 60;
  if (days > 0) return `Resets in ${days}d ${hours}h`;
  if (hours > 0) return `Resets in ${hours}h ${minutes}m`;
  return `Resets in ${minutes}m ${String(secs).padStart(2, "0")}s`;
}

export function formatLocalReset(resetsAt?: number, now = new Date()): string {
  if (!resetsAt) return "";
  const reset = new Date(resetsAt * 1000);
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const target = new Date(reset.getFullYear(), reset.getMonth(), reset.getDate());
  const dayDelta = Math.round((target.getTime() - today.getTime()) / 86_400_000);
  let dayLabel: string;
  if (dayDelta === 0) dayLabel = reset.getHours() >= 17 ? "Tonight" : "Today";
  else if (dayDelta === 1) dayLabel = "Tomorrow";
  else dayLabel = reset.toLocaleDateString(undefined, { weekday: "long" });
  const time = reset.toLocaleTimeString(undefined, {
    hour: "numeric",
    minute: "2-digit",
  });
  return `${dayLabel} · ${time}`;
}

export function formatCompactNumber(value?: number): string {
  if (value === undefined) return "—";
  return new Intl.NumberFormat(undefined, {
    notation: "compact",
    maximumFractionDigits: 1,
  }).format(value);
}
