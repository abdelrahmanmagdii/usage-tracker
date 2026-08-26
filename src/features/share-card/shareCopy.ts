import type { RateLimitBucket } from "../../types/codex";
import { formatCountdown, windowDurationLabel } from "../../lib/rateLimits";

/** Public site LinkedIn and X unfurl. Trailing slash matches the Pages canonical URL. */
export const SITE_URL = "https://abdelrahmanmagdii.github.io/usage-tracker/";
export const REPO_URL = "https://github.com/abdelrahmanmagdii/usage-tracker";

const X_LIMIT = 280;

function windowName(bucket: RateLimitBucket): string {
  return bucket.windowLabel ?? windowDurationLabel(bucket.windowDurationMins);
}

function subject(bucket: RateLimitBucket): string {
  const window = windowName(bucket);
  return bucket.limitName ? `${window} ${bucket.limitName}` : window;
}

function remaining(bucket: RateLimitBucket): number {
  return Math.round(bucket.remainingPercent);
}

function resetLine(bucket: RateLimitBucket, nowMs: number): string {
  return formatCountdown(bucket.resetsAt, nowMs)
    .replace(/^Resets /, "resets ")
    .replace(/^Reset /, "reset ");
}

export function xShareText(bucket: RateLimitBucket, nowMs = Date.now()): string {
  const head = `${remaining(bucket)}% left on my ${subject(bucket)} window, ${resetLine(bucket, nowMs)}.`;
  const tail = `\n\nUsageBar keeps Codex, Claude, Cursor & OpenCode quotas in the Mac menu bar. Local only.\n${SITE_URL}`;
  if (head.length + tail.length <= X_LIMIT) return `${head}${tail}`;
  const short = `\n\nUsageBar — local quota meters for the Mac menu bar.\n${SITE_URL}`;
  if (head.length + short.length <= X_LIMIT) return `${head}${short}`;
  return `${remaining(bucket)}% left · ${SITE_URL}`.slice(0, X_LIMIT);
}

export function linkedInShareText(bucket: RateLimitBucket, nowMs = Date.now()): string {
  return [
    `${remaining(bucket)}% left on my ${subject(bucket)} window — ${resetLine(bucket, nowMs)}.`,
    "",
    "UsageBar is a Mac menu bar meter for Codex, Claude Code, Cursor, and OpenCode Go. Quota stays on-device: no API keys, no account scraping, nothing sent to us.",
    "",
    SITE_URL,
  ].join("\n");
}

export function xIntentUrl(text: string): string {
  return `https://x.com/intent/tweet?text=${encodeURIComponent(text)}`;
}

export function linkedInShareUrl(url: string = SITE_URL): string {
  return `https://www.linkedin.com/sharing/share-offsite/?url=${encodeURIComponent(url)}`;
}
