import { describe, expect, it } from "vitest";
import type { ResetEvent } from "../../types/codex";
import {
  CombinedResetEventProvider,
  normalizeFeedEvent,
  tiboStatus,
  upcomingReset,
  type ResetEventProvider,
} from "./provider";

function event(partial: Partial<ResetEvent> & { id: string; announcedAt: string }): ResetEvent {
  return { source: "tibo", ...partial };
}

describe("normalizeFeedEvent", () => {
  it("accepts a well-formed feed entry", () => {
    expect(
      normalizeFeedEvent({
        id: "tibo-1",
        announcedAt: "2026-08-13T01:01:37Z",
        occursAt: "2026-08-13T02:01:37Z",
        source: "tibo",
        text: "Enjoy a nice reset everyone",
        sourceUrl: "https://x.com/thsottiaux/status/1",
        plansAffected: ["Pro", 42],
      }),
    ).toEqual({
      id: "tibo-1",
      announcedAt: "2026-08-13T01:01:37Z",
      occurredAt: undefined,
      occursAt: "2026-08-13T02:01:37Z",
      source: "tibo",
      text: "Enjoy a nice reset everyone",
      sourceUrl: "https://x.com/thsottiaux/status/1",
      plansAffected: ["Pro"],
    });
  });

  it("rejects malformed entries", () => {
    expect(normalizeFeedEvent(null)).toBeNull();
    expect(normalizeFeedEvent({ announcedAt: "2026-08-13T01:01:37Z" })).toBeNull();
    expect(normalizeFeedEvent({ id: "x", announcedAt: "not a date" })).toBeNull();
    expect(normalizeFeedEvent("tibo-1")).toBeNull();
  });

  it("defaults unknown sources to tibo", () => {
    expect(
      normalizeFeedEvent({ id: "tibo-1", announcedAt: "2026-08-13T01:01:37Z", source: "???" })?.source,
    ).toBe("tibo");
  });
});

function stubProvider(id: string, events: ResetEvent[]): ResetEventProvider {
  return { id, listEvents: () => Promise.resolve(events) };
}

describe("CombinedResetEventProvider", () => {
  it("merges providers, dedupes by id, and sorts newest first", async () => {
    const older = event({ id: "a", announcedAt: "2026-08-01T00:00:00Z" });
    const newer = event({ id: "b", announcedAt: "2026-08-10T00:00:00Z" });
    const dupe = event({ id: "b", announcedAt: "2026-08-10T00:00:00Z", text: "duplicate copy" });
    const combined = new CombinedResetEventProvider([
      stubProvider("one", [older]),
      stubProvider("two", [newer, dupe]),
    ]);
    const events = await combined.listEvents();
    expect(events.map((entry) => entry.id)).toEqual(["b", "a"]);
    expect(events[0].text).toBeUndefined();
  });

  it("survives a failing provider", async () => {
    const failing: ResetEventProvider = {
      id: "failing",
      listEvents: () => Promise.reject(new Error("network down")),
    };
    const combined = new CombinedResetEventProvider([
      failing,
      stubProvider("ok", [event({ id: "a", announcedAt: "2026-08-01T00:00:00Z" })]),
    ]);
    expect(await combined.listEvents()).toHaveLength(1);
  });
});

describe("upcomingReset", () => {
  const now = Date.parse("2026-08-13T00:00:00Z");
  it("finds the nearest future effective time", () => {
    const events = [
      event({ id: "past", announcedAt: "2026-08-10T00:00:00Z", occursAt: "2026-08-10T01:00:00Z" }),
      event({ id: "later", announcedAt: "2026-08-12T00:00:00Z", occursAt: "2026-08-14T00:00:00Z" }),
      event({ id: "sooner", announcedAt: "2026-08-12T06:00:00Z", occursAt: "2026-08-13T06:00:00Z" }),
    ];
    expect(upcomingReset(events, now)?.id).toBe("sooner");
  });

  it("returns undefined when nothing is pending", () => {
    expect(upcomingReset([event({ id: "a", announcedAt: "2026-08-01T00:00:00Z" })], now)).toBeUndefined();
  });
});

describe("tiboStatus", () => {
  const now = Date.parse("2026-08-13T00:00:00Z");
  it("flags an incoming reset before anything else", () => {
    const events = [
      event({ id: "fresh", announcedAt: "2026-08-12T22:00:00Z" }),
      event({ id: "incoming", announcedAt: "2026-08-12T20:00:00Z", occursAt: "2026-08-13T02:00:00Z" }),
    ];
    expect(tiboStatus(events, now).label).toBe("Reset incoming");
  });

  it("falls back to recency tones", () => {
    expect(
      tiboStatus([event({ id: "fresh", announcedAt: "2026-08-12T22:00:00Z" })], now).label,
    ).toBe("Recently reset");
    expect(
      tiboStatus([event({ id: "stale", announcedAt: "2026-08-05T00:00:00Z" })], now).label,
    ).toBe("No recent resets");
    expect(
      tiboStatus([event({ id: "ancient", announcedAt: "2026-07-01T00:00:00Z" })], now).label,
    ).toBe("No recent resets");
    expect(tiboStatus([], now).label).toBe("No resets recorded");
  });
});
