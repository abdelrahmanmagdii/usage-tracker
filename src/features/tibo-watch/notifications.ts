import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import type { ResetEvent } from "../../types/codex";
import { formatLeadTime } from "../../lib/time";

const NOTIFIED_KEY = "codex-meter.notified-events.v1";
const FRESH_WINDOW_MS = 2 * 3_600_000;
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

export function selectFreshResetNotifications(
  events: ResetEvent[],
  notifiedIds: Iterable<string>,
  nowMs = Date.now(),
): ResetEvent[] {
  const notified = new Set(notifiedIds);
  return events.filter((event) => {
    if (event.sample || notified.has(event.id)) return false;
    const announced = Date.parse(event.announcedAt);
    return Number.isFinite(announced) && announced <= nowMs && nowMs - announced <= FRESH_WINDOW_MS;
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

export function resetNotificationBody(event: ResetEvent, nowMs = Date.now()): string {
  if (event.source === "detected") {
    return "Your available quota jumped before its scheduled renewal.";
  }
  const occursAt = event.occursAt ? Date.parse(event.occursAt) : Number.NaN;
  if (Number.isFinite(occursAt) && occursAt > nowMs) {
    return `Lands in ~${formatLeadTime(occursAt - nowMs)} — spend what's left of your current quota, it refreshes anyway.`;
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
export async function notifyFreshResets(events: ResetEvent[], nowMs = Date.now()): Promise<void> {
  if (!("__TAURI_INTERNALS__" in window)) return;
  const notified = new Set(readNotified());
  const fresh = selectFreshResetNotifications(events, notified, nowMs);
  if (fresh.length === 0) return;
  let changed = false;
  try {
    let granted = await isPermissionGranted();
    if (!granted) granted = (await requestPermission()) === "granted";
    if (!granted) {
      for (const event of fresh) notified.add(event.id);
      writeNotified(notified);
      return;
    }
    for (const event of fresh.slice(0, MAX_PER_BATCH)) {
      sendNotification({
        title: resetNotificationTitle(event, nowMs),
        body: resetNotificationBody(event, nowMs),
      });
      notified.add(event.id);
      changed = true;
    }
  } catch {
    /* Keep failed events eligible for a later retry (e.g. after installing a signed build). */
  } finally {
    if (changed) writeNotified(notified);
  }
}
