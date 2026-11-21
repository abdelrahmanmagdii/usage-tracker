import { invoke } from "@tauri-apps/api/core";

export type NotificationPermissionState =
  | "notDetermined"
  | "denied"
  | "granted"
  | "unavailable"
  | "unknown";

export type NotificationSource = "test" | "reset";

export type NotificationDeliveryError = {
  source: string;
  message: string;
  at: number;
};

export type NotificationStatus = {
  permission: NotificationPermissionState;
  lastError: NotificationDeliveryError | null;
};

const UNAVAILABLE: NotificationStatus = { permission: "unavailable", lastError: null };

const inTauri = () => typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

const asStatus = (value: unknown): NotificationStatus => {
  const status = (value ?? {}) as Partial<NotificationStatus>;
  const known: NotificationPermissionState[] = ["notDetermined", "denied", "granted", "unavailable", "unknown"];
  return {
    permission: known.includes(status.permission as NotificationPermissionState)
      ? (status.permission as NotificationPermissionState)
      : "unavailable",
    lastError: status.lastError ?? null,
  };
};

/** The real permission. Never prompts. */
export async function getNotificationStatus(): Promise<NotificationStatus> {
  if (!inTauri()) return UNAVAILABLE;
  try {
    return asStatus(await invoke("get_notification_status"));
  } catch {
    return UNAVAILABLE;
  }
}

/** Asks the system for permission. Call this only from an explicit user action. */
export async function requestNotificationPermission(): Promise<NotificationStatus> {
  if (!inTauri()) return UNAVAILABLE;
  try {
    return asStatus(await invoke("request_notification_permission"));
  } catch {
    return UNAVAILABLE;
  }
}

/** Resolves only when the system accepted the notification; rejects with the reason otherwise. */
export async function postUsageNotification(
  source: NotificationSource,
  title: string,
  body: string,
): Promise<void> {
  if (!inTauri()) throw new Error("Notifications are unavailable here.");
  await invoke("post_usage_notification", { source, title, body });
}

/** Whether a background alert may be posted without asking. */
export const canPostInBackground = (permission: NotificationPermissionState): boolean =>
  permission === "granted" || permission === "unknown";
