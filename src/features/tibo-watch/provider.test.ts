import { describe, expect, it } from "vitest";
import type { ResetEvent } from "../../types/codex";
import {
  BlueskyResetEventProvider,
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
    expect(tiboStatus([], now).label).toBe("Watching for resets");
  });
});

function bskyResponse(posts: { rkey: string; createdAt: string; text: string }[]): Response {
  const feed = posts.map(({ rkey, createdAt, text }) => ({
    post: { uri: `at://did:plc:x/app.bsky.feed.post/${rkey}`, record: { createdAt, text } },
  }));
  return new Response(JSON.stringify({ feed }), { status: 200 });
}

describe("BlueskyResetEventProvider", () => {
  it("detects resets across mirrors and stores each tweet once", async () => {
    const tweet = { createdAt: "2026-10-02T02:14:51.000Z", text: "Global reset landing tomorrow 10am PST for all paid ChatGPT accounts." };
    const chatter = { createdAt: "2026-10-02T05:56:13.000Z", text: "Down to 6110 unread emails" };
    const fetcher = (async (url: string) =>
      bskyResponse(
        url.includes("mirror-a")
          ? [{ rkey: "a1", ...tweet }, { rkey: "a2", ...chatter }]
          : [{ rkey: "b1", ...tweet }],
      )) as unknown as typeof fetch;
    const events = await new BlueskyResetEventProvider(["mirror-a", "mirror-b"], fetcher).listEvents();
    expect(events).toHaveLength(1);
    expect(events[0].occursAt).toBe("2026-10-02T17:00:00.000Z");
  });

  it("survives one mirror failing", async () => {
    const fetcher = (async (url: string) =>
      url.includes("down")
        ? new Response("", { status: 502 })
        : bskyResponse([{ rkey: "a1", createdAt: "2026-10-02T21:18:48.000Z", text: "Reset all propagated. Enjoy." }])) as unknown as typeof fetch;
    const events = await new BlueskyResetEventProvider(["down", "up"], fetcher).listEvents();
    expect(events.map((event) => event.id)).toEqual(["tibo-bsky-a1"]);
  });
});

describe("CombinedResetEventProvider mirror dedupe", () => {
  it("keeps one copy of a tweet seen through two mirrors", async () => {
    const at = "2026-10-02T21:18:48.000Z";
    const feed: ResetEventProvider = { id: "feed", listEvents: async () => [event({ id: "tibo-bsky-a", announcedAt: at })] };
    const bsky: ResetEventProvider = { id: "bsky", listEvents: async () => [event({ id: "tibo-bsky-b", announcedAt: at })] };
    const events = await new CombinedResetEventProvider([feed, bsky]).listEvents();
    expect(events.map((entry) => entry.id)).toEqual(["tibo-bsky-a"]);
  });
});
