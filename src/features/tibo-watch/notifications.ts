import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import type { ResetEvent } from "../../types/codex";
import { formatLeadTime } from "../../lib/time";

const NOTIFIED_KEY = "codex-meter.notified-events.v1";
// Announcements stay notifiable for half a day: posts can take hours to
// reach the feed, and a Mac that slept through one should still hear of it.
const ANNOUNCED_WINDOW_MS = 12 * 3_600_000;
const LANDED_WINDOW_MS = 2 * 3_600_000;
const MAX_STORED_IDS = 200;
const MAX_PER_BATCH = 2;

function readNotified(): string[] {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(NOTIFIED_KEY) ?? "[]");
    return Array.isArray(value)
      ? value.filter((id): id is string => typeof id === "string")
      : [];
  } catch {
    return [];
  }
}

function writeNotified(ids: Iterable<string>): void {
  try {
    localStorage.setItem(NOTIFIED_KEY, JSON.stringify([...ids].slice(-MAX_STORED_IDS)));
  } catch {
    /* A notification can still be delivered when local storage is unavailable. */
  }
}

/**
 * Each relay mirrors a tweet under its own post id but keeps the tweet's
 * timestamp, so the timestamp also dedupes notifications across mirrors.
 */
export const announcedKey = (event: ResetEvent): string => `announced:${event.announcedAt}`;

const wasNotified = (event: ResetEvent, notified: Set<string>): boolean =>
  notified.has(event.id) || (event.source !== "detected" && notified.has(announcedKey(event)));

export function selectFreshResetNotifications(
  events: ResetEvent[],
  notifiedIds: Iterable<string>,
  nowMs = Date.now(),
): ResetEvent[] {
  const notified = new Set(notifiedIds);
  return events.filter((event) => {
    if (event.sample || wasNotified(event, notified)) return false;
    const announced = Date.parse(event.announcedAt);
    if (!Number.isFinite(announced) || announced > nowMs) return false;
    const occursAt = event.occursAt ? Date.parse(event.occursAt) : Number.NaN;
    // A scheduled reset ("landing tomorrow 10am") is news until it lands.
    if (Number.isFinite(occursAt) && occursAt > nowMs) return true;
    return nowMs - announced <= ANNOUNCED_WINDOW_MS;
  });
}

/** The dedupe key for the "reset landed" follow-up to an announcement. */
export const landedKey = (event: ResetEvent): string => `${event.id}:landed`;

/**
 * Announced resets whose predicted window (occursAt) has now passed and
 * whose follow-up hasn't been sent yet. This is the second act of an
 * "incoming" notification — the moment the quota is actually fresh again.
 */
export function selectLandedResetNotifications(
  events: ResetEvent[],
  notifiedIds: Iterable<string>,
  nowMs = Date.now(),
): ResetEvent[] {
  const notified = new Set(notifiedIds);
  return events.filter((event) => {
    if (event.sample || notified.has(landedKey(event))) return false;
    const occursAt = event.occursAt ? Date.parse(event.occursAt) : Number.NaN;
    return Number.isFinite(occursAt) && occursAt <= nowMs && nowMs - occursAt <= LANDED_WINDOW_MS;
  });
}

export function resetNotificationTitle(event: ResetEvent, nowMs = Date.now()): string {
  if (event.source === "detected") return "Possible Codex quota reset detected";
  const occursAt = event.occursAt ? Date.parse(event.occursAt) : Number.NaN;
  // The advance warning is the headline: knowing a reset is incoming means
  // remaining quota can be spent freely before it refreshes anyway.
  if (Number.isFinite(occursAt) && occursAt > nowMs) return "⚡ Codex reset incoming";
  return "Codex quota reset announced";
}

/** "Plus, Pro" or "All paid plans", when the post said who it covers. */
export function planSummary(event: ResetEvent): string | null {
  return event.plansAffected?.length ? event.plansAffected.join(", ") : null;
}

export function resetNotificationBody(event: ResetEvent, nowMs = Date.now()): string {
  if (event.source === "detected") {
    return "Your available quota jumped before its scheduled renewal.";
  }
  const occursAt = event.occursAt ? Date.parse(event.occursAt) : Number.NaN;
  const plans = planSummary(event);
  if (Number.isFinite(occursAt) && occursAt > nowMs) {
    return `${plans ? `${plans}: lands` : "Lands"} in ~${formatLeadTime(occursAt - nowMs)} — spend what's left of your current quota, it refreshes anyway.`;
  }
  if (Number.isFinite(occursAt)) return "Quota has been reset.";
  return event.text
    ? `“${event.text.length > 120 ? `${event.text.slice(0, 117)}…` : event.text}”`
    : "A surprise reset was announced.";
}

/**
 * Sends a macOS notification for fresh public announcements and locally
 * detected surprise resets. Only notifications accepted by the native bridge
 * are recorded as delivered; denied permission is remembered for the batch so
 * the app does not repeatedly ask about the same event.
 */
export async function notifyFreshResets(
  events: ResetEvent[],
  nowMs = Date.now(),
  { enabled = true }: { enabled?: boolean } = {},
): Promise<void> {
  if (!enabled || !("__TAURI_INTERNALS__" in window)) return;
  const notified = new Set(readNotified());
  // "Landed" first — quota becoming usable again is the actionable moment.
  // An event whose occursAt already passed isn't also sent as an
  // announcement; the landed notification carries the same news.
  const landed = selectLandedResetNotifications(events, notified, nowMs);
  const landedIds = new Set(landed.map((event) => event.id));
  const fresh = selectFreshResetNotifications(events, notified, nowMs)
    .filter((event) => !landedIds.has(event.id));
  const queue = [
    ...landed.map((event) => ({
      event,
      key: landedKey(event),
      title: "⚡ Codex quota reset has landed",
      body: "The announced reset just took effect — your quota is fresh again.",
    })),
    ...fresh.map((event) => ({
      event,
      key: event.id,
      title: resetNotificationTitle(event, nowMs),
      body: resetNotificationBody(event, nowMs),
    })),
  ].slice(0, MAX_PER_BATCH);
  if (queue.length === 0) return;
  let changed = false;
  try {
    let granted = await isPermissionGranted();
    if (!granted) granted = (await requestPermission()) === "granted";
    if (!granted) {
      for (const item of queue) {
        notified.add(item.key);
        notified.add(announcedKey(item.event));
      }
      writeNotified(notified);
      return;
    }
    for (const item of queue) {
      // Await delivery before recording: an un-awaited rejection used to
      // mark the event notified without ever showing the notification.
      await sendNotification({ title: item.title, body: item.body });
      notified.add(item.key);
      if (item.event.source !== "detected") notified.add(announcedKey(item.event));
      // A landed event is the final word on that announcement.
      if (item.key === landedKey(item.event)) notified.add(item.event.id);
      changed = true;
    }
  } catch {
    /* Keep failed events eligible for a later retry (e.g. after installing a signed build). */
  } finally {
    if (changed) writeNotified(notified);
  }
}

export type TestNotificationResult = "sent" | "denied" | "unavailable";

/** Sends a sample alert so a muted or denied permission shows up now, not on reset day. */
export async function sendTestNotification(): Promise<TestNotificationResult> {
  if (!("__TAURI_INTERNALS__" in window)) return "unavailable";
  try {
    let granted = await isPermissionGranted();
    if (!granted) granted = (await requestPermission()) === "granted";
    if (!granted) return "denied";
    await sendNotification({
      title: "UsageBar notifications work",
      body: "You'll hear about Tibo's resets and your usage alerts like this.",
    });
    return "sent";
  } catch {
    return "unavailable";
  }
}
