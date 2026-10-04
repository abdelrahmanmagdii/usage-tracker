import { describe, expect, it } from "vitest";
import type { ResetEvent } from "../../types/codex";
import {
  announcedKey,
  landedKey,
  resetNotificationBody,
  resetNotificationTitle,
  selectFreshResetNotifications,
  selectLandedResetNotifications,
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
    expect(resetNotificationTitle(local)).toBe("Possible Codex quota reset detected");
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
