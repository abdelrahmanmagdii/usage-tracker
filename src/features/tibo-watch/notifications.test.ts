import { describe, expect, it } from "vitest";
import type { ResetEvent } from "../../types/codex";
import {
  resetNotificationBody,
  resetNotificationTitle,
  selectFreshResetNotifications,
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
        event({ id: "old", announcedAt: "2026-08-13T08:00:00Z" }),
        event({ id: "future", announcedAt: "2026-08-13T13:00:00Z" }),
        event({ id: "delivered" }),
      ],
      ["delivered"],
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

  it("shows the lead time for an upcoming announced reset", () => {
    expect(
      resetNotificationBody(event({ id: "upcoming", occursAt: "2026-08-13T13:00:00Z" }), now),
    ).toContain("1h");
  });
});
