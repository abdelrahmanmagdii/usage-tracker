export type ConnectionState =
  | "starting"
  | "connected"
  | "disconnected"
  | "cli_not_found"
  | "not_authenticated"
  | "error";

export type CodexBackendState = {
  connection: ConnectionState;
  diagnostic?: string | null;
  account?: unknown;
  rateLimits?: unknown;
  usage?: unknown;
  updatedAt?: number | null;
};

export type RateLimitBucket = {
  id: string;
  limitId: string;
  limitName?: string;
  /** Overrides the duration-derived tile heading (e.g. "Fable" for model-scoped windows). */
  windowLabel?: string;
  windowKind: "primary" | "secondary";
  usedPercent: number;
  remainingPercent: number;
  windowDurationMins?: number;
  resetsAt?: number;
  reached: boolean;
};

export type ResetCredits = {
  availableCount: number;
};

export type UsageSummary = {
  lifetimeTokens?: number;
  peakDailyTokens?: number;
  longestRunningTurnSec?: number;
  currentStreakDays?: number;
  longestStreakDays?: number;
  dailyUsage: Array<{ startDate: string; tokens: number }>;
};

export type QuotaSnapshot = {
  timestamp: string;
  limitId: string;
  usedPercent: number;
  remainingPercent: number;
  windowDurationMins?: number;
  resetsAt?: number;
};

export type ResetEvent = {
  id: string;
  announcedAt: string;
  occurredAt?: string;
  /** When a reset is expected to take effect, if one was announced ahead of time. */
  occursAt?: string;
  sourceUrl?: string;
  source: "tibo" | "openai" | "detected" | "manual";
  text?: string;
  plansAffected?: string[];
  sample?: boolean;
};
