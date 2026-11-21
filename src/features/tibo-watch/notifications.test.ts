// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ResetEvent } from "../../types/codex";
import type { NotificationPermissionState } from "../../lib/notificationDelivery";

const delivery = vi.hoisted(() => ({
  permission: "granted" as NotificationPermissionState,
  getNotificationStatus: vi.fn(),
  requestNotificationPermission: vi.fn(),
  postUsageNotification: vi.fn(),
}));
vi.mock("../../lib/notificationDelivery", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../lib/notificationDelivery")>()),
  getNotificationStatus: delivery.getNotificationStatus,
  requestNotificationPermission: delivery.requestNotificationPermission,
  postUsageNotification: delivery.postUsageNotification,
}));

import { codexEvents, eventsForScopes } from "./provider";
import {
  announcedKey,
  landedKey,
  resetNotificationBody,
  resetNotificationTitle,
  selectFreshResetNotifications,
  notifyFreshResets,
  selectLandedResetNotifications,
  sendTestNotification,
} from "./notifications";

const now = Date.parse("2026-08-13T12:00:00Z");

function event(partial: Partial<ResetEvent> & { id: string }): ResetEvent {
  return {
    announcedAt: "2026-08-13T11:30:00Z",
    source: "tibo",
    ...partial,
  };
}

describe("selectFreshResetNotifications", () => {
  it("includes public and locally detected fresh resets", () => {
    const selected = selectFreshResetNotifications(
      [event({ id: "public" }), event({ id: "local", source: "detected" })],
      [],
      now,
    );
    expect(selected.map((entry) => entry.id)).toEqual(["public", "local"]);
  });

  it("excludes samples, old events, future announcements, and delivered ids", () => {
    const selected = selectFreshResetNotifications(
      [
        event({ id: "sample", sample: true }),
        event({ id: "old", announcedAt: "2026-08-12T20:00:00Z" }),
        event({ id: "future", announcedAt: "2026-08-13T13:00:00Z" }),
        event({ id: "delivered" }),
      ],
      ["delivered"],
      now,
    );
    expect(selected).toEqual([]);
  });
});

describe("selectFreshResetNotifications windows", () => {
  it("keeps announcements notifiable for hours, since the feed can lag", () => {
    const selected = selectFreshResetNotifications(
      [event({ id: "late", announcedAt: "2026-08-13T05:00:00Z" })],
      [],
      now,
    );
    expect(selected.map((entry) => entry.id)).toEqual(["late"]);
  });

  it("keeps a scheduled reset notifiable until it lands, however early it was posted", () => {
    const selected = selectFreshResetNotifications(
      [event({ id: "tomorrow", announcedAt: "2026-08-12T02:00:00Z", occursAt: "2026-08-13T17:00:00Z" })],
      [],
      now,
    );
    expect(selected.map((entry) => entry.id)).toEqual(["tomorrow"]);
  });

  it("treats the same tweet from another mirror as already delivered", () => {
    const first = event({ id: "tibo-bsky-mirror-a" });
    const selected = selectFreshResetNotifications(
      [event({ id: "tibo-bsky-mirror-b" })],
      [first.id, announcedKey(first)],
      now,
    );
    expect(selected).toEqual([]);
  });
});

describe("selectLandedResetNotifications", () => {
  it("includes a scheduled reset once its occursAt passes", () => {
    const selected = selectLandedResetNotifications(
      [
        event({ id: "landed", occursAt: "2026-08-13T11:45:00Z" }),
        event({ id: "still-upcoming", occursAt: "2026-08-13T12:30:00Z" }),
        event({ id: "no-schedule" }),
      ],
      [],
      now,
    );
    expect(selected.map((entry) => entry.id)).toEqual(["landed"]);
  });

  it("excludes already-landed events, samples, and windows long past", () => {
    const selected = selectLandedResetNotifications(
      [
        event({ id: "delivered", occursAt: "2026-08-13T11:45:00Z" }),
        event({ id: "sample", sample: true, occursAt: "2026-08-13T11:45:00Z" }),
        event({ id: "stale", occursAt: "2026-08-13T08:00:00Z" }),
      ],
      [landedKey(event({ id: "delivered" }))],
      now,
    );
    expect(selected).toEqual([]);
  });
});

describe("notification copy", () => {
  it("distinguishes a local detection from a published announcement", () => {
    const local = event({ id: "local", source: "detected" });
    expect(resetNotificationTitle(local)).toBe("Possible quota reset detected");
    expect(resetNotificationBody(local, now)).toContain("available quota jumped");
    expect(resetNotificationTitle(event({ id: "public" }))).toBe("Codex quota reset announced");
  });

  it("frames an upcoming reset as a burn window with lead time", () => {
    const upcoming = event({ id: "upcoming", occursAt: "2026-08-13T13:00:00Z" });
    expect(resetNotificationTitle(upcoming, now)).toBe("⚡ Codex reset incoming");
    const body = resetNotificationBody(upcoming, now);
    expect(body).toContain("1h");
    expect(body).toContain("spend what's left");
  });
});

describe("plans and gating", () => {
  it("leads the incoming body with the plans", async () => {
    const { resetNotificationBody, planSummary } = await import("./notifications");
    const nowMs = Date.parse("2026-10-03T12:00:00Z");
    const event = {
      id: "p",
      announcedAt: new Date(nowMs - 60_000).toISOString(),
      occursAt: new Date(nowMs + 3_600_000).toISOString(),
      source: "tibo" as const,
      plansAffected: ["Plus", "Pro"],
    };
    expect(planSummary(event)).toBe("Plus, Pro");
    expect(resetNotificationBody(event, nowMs)).toMatch(/^Plus, Pro: lands in ~1h/);
  });
});

describe("native delivery", () => {
  const stored = () => JSON.parse(localStorage.getItem("codex-meter.notified-events.v1") ?? "[]") as string[];

  beforeEach(() => {
    delivery.permission = "granted";
    delivery.getNotificationStatus.mockReset().mockImplementation(async () => ({
      permission: delivery.permission,
      lastError: null,
    }));
    delivery.requestNotificationPermission.mockReset().mockImplementation(async () => ({
      permission: delivery.permission,
      lastError: null,
    }));
    delivery.postUsageNotification.mockReset().mockResolvedValue(undefined);
    Object.defineProperty(window, "__TAURI_INTERNALS__", { value: {}, configurable: true });
    localStorage.clear();
  });

  afterEach(() => {
    Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
    localStorage.clear();
  });

  it.each(["denied", "notDetermined", "unavailable"] as const)(
    "never requests, posts, or records anything in the background when %s",
    async (permission) => {
      delivery.permission = permission;
      await notifyFreshResets([event({ id: "fresh" })], now);
      expect(delivery.requestNotificationPermission).not.toHaveBeenCalled();
      expect(delivery.postUsageNotification).not.toHaveBeenCalled();
      expect(stored()).toEqual([]);
    },
  );

  it("retries a still-fresh event once permission is granted", async () => {
    delivery.permission = "denied";
    await notifyFreshResets([event({ id: "fresh" })], now);
    expect(delivery.postUsageNotification).not.toHaveBeenCalled();
    delivery.permission = "granted";
    await notifyFreshResets([event({ id: "fresh" })], now);
    expect(delivery.postUsageNotification).toHaveBeenCalledTimes(1);
    expect(delivery.postUsageNotification.mock.calls[0][0]).toBe("reset");
    expect(stored()).toContain("fresh");
  });

  it("posts when the platform reports an unknown permission", async () => {
    delivery.permission = "unknown";
    await notifyFreshResets([event({ id: "fresh" })], now);
    expect(delivery.postUsageNotification).toHaveBeenCalledTimes(1);
  });

  it("marks an event delivered only after a post succeeds", async () => {
    delivery.postUsageNotification.mockRejectedValueOnce("macOS did not accept the notification request.");
    await notifyFreshResets([event({ id: "fresh" })], now);
    expect(stored()).toEqual([]);
    await notifyFreshResets([event({ id: "fresh" })], now);
    expect(delivery.postUsageNotification).toHaveBeenCalledTimes(2);
    expect(stored()).toContain("fresh");
    await notifyFreshResets([event({ id: "fresh" })], now);
    expect(delivery.postUsageNotification).toHaveBeenCalledTimes(2);
  });

  it("does nothing when reset alerts are off", async () => {
    await notifyFreshResets([event({ id: "fresh" })], now, { enabled: false });
    expect(delivery.getNotificationStatus).not.toHaveBeenCalled();
    expect(delivery.postUsageNotification).not.toHaveBeenCalled();
  });

  it("caps a batch at two notifications", async () => {
    await notifyFreshResets(
      [
        event({ id: "a", announcedAt: "2026-08-13T11:30:00.000Z" }),
        event({ id: "b", announcedAt: "2026-08-13T11:31:00.000Z" }),
        event({ id: "c", announcedAt: "2026-08-13T11:32:00.000Z" }),
      ],
      now,
    );
    expect(delivery.postUsageNotification).toHaveBeenCalledTimes(2);
  });

  describe("test notification", () => {
    it("is denied without posting when permission is denied", async () => {
      delivery.permission = "denied";
      expect(await sendTestNotification()).toBe("denied");
      expect(delivery.requestNotificationPermission).not.toHaveBeenCalled();
      expect(delivery.postUsageNotification).not.toHaveBeenCalled();
    });

    it("asks once when permission is undetermined, then reports a refusal as denied", async () => {
      delivery.permission = "notDetermined";
      delivery.requestNotificationPermission.mockResolvedValueOnce({ permission: "denied", lastError: null });
      expect(await sendTestNotification()).toBe("denied");
      expect(delivery.requestNotificationPermission).toHaveBeenCalledTimes(1);
      expect(delivery.postUsageNotification).not.toHaveBeenCalled();
    });

    it("posts after a grant and reports acceptance", async () => {
      delivery.permission = "notDetermined";
      delivery.requestNotificationPermission.mockResolvedValueOnce({ permission: "granted", lastError: null });
      expect(await sendTestNotification()).toBe("sent");
      expect(delivery.postUsageNotification.mock.calls[0][0]).toBe("test");
    });

    it("reports a failed post and an unavailable build", async () => {
      delivery.postUsageNotification.mockRejectedValueOnce("nope");
      expect(await sendTestNotification()).toBe("failed");
      delivery.permission = "unavailable";
      expect(await sendTestNotification()).toBe("unavailable");
    });
  });
});

describe("local detections name their provider and account", () => {
  it("titles a detection with the provider and window, and stays generic for legacy ones", () => {
    const devin = event({ id: "d", source: "detected", provider: "devin", windowLabel: "Weekly", scopeId: "s" });
    expect(resetNotificationTitle(devin)).toBe("Devin: possible weekly quota reset detected");
    expect(resetNotificationBody(devin, now)).toBe("Your weekly quota jumped before its scheduled renewal.");
    const legacy = event({ id: "old", source: "detected" });
    expect(resetNotificationTitle(legacy)).toBe("Possible quota reset detected");
    expect(resetNotificationTitle(legacy)).not.toMatch(/Codex/);
  });

  it("keeps another provider's detection out of the Codex banner", () => {
    const devin = event({ id: "d", source: "detected", provider: "devin", scopeId: "s" });
    const codex = event({ id: "c", source: "detected", provider: "codex", scopeId: "s" });
    const legacy = event({ id: "l", source: "detected" });
    const announced = event({ id: "t" });
    expect(codexEvents([devin, codex, legacy, announced]).map((e) => e.id)).toEqual(["c", "t"]);
  });

  it("drops a detection from an account that is no longer on screen but keeps announcements", () => {
    const old = event({ id: "old", source: "detected", provider: "codex", scopeId: "scope-a" });
    const current = event({ id: "cur", source: "detected", provider: "codex", scopeId: "scope-b" });
    const unscoped = event({ id: "none", source: "detected", provider: "codex" });
    const announced = event({ id: "t" });
    expect(
      eventsForScopes([old, current, unscoped, announced], new Set(["scope-b"])).map((e) => e.id),
    ).toEqual(["cur", "t"]);
  });
});
