// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ResetEvent } from "../../types/codex";

const plugin = vi.hoisted(() => ({
  isPermissionGranted: vi.fn(),
  requestPermission: vi.fn(),
  sendNotification: vi.fn(),
}));
vi.mock("@tauri-apps/plugin-notification", () => plugin);

import {
  announcedKey,
  notifyFreshResets,
  selectFreshResetNotifications,
} from "./notifications";
import {
  CombinedResetEventProvider,
  latestLandedReset,
  tiboStatus,
  type ResetEventProvider,
} from "./provider";

const now = Date.parse("2026-08-13T12:00:00Z");

function event(partial: Partial<ResetEvent> & { id: string }): ResetEvent {
  return { announcedAt: "2026-08-13T11:30:00.000Z", source: "tibo", ...partial };
}

const stub = (id: string, events: ResetEvent[]): ResetEventProvider => ({
  id,
  listEvents: () => Promise.resolve(events),
});

describe("one tweet, two timestamp spellings", () => {
  const relay = event({ id: "tibo-bsky-abc", announcedAt: "2026-08-13T11:30:00.000Z" });
  const manual = event({ id: "manual-2026-08-13", announcedAt: "2026-08-13T11:30:00Z", source: "manual" });

  it("is listed once by the combined provider", async () => {
    const combined = new CombinedResetEventProvider([stub("feed", [manual]), stub("bluesky", [relay])]);
    expect((await combined.listEvents()).map((entry) => entry.id)).toEqual(["manual-2026-08-13"]);
  });

  it("keeps a locally detected jump at the same instant as a post", async () => {
    const detected = event({ id: "detected-1", source: "detected" });
    const combined = new CombinedResetEventProvider([stub("feed", [relay]), stub("local", [detected])]);
    expect(await combined.listEvents()).toHaveLength(2);
  });

  it("shares a notification key, so the feed copy is not announced after the relay copy", () => {
    expect(announcedKey(manual)).toBe(announcedKey(relay));
    expect(selectFreshResetNotifications([manual], [announcedKey(relay)], now)).toEqual([]);
  });

  it("keeps keys stored by earlier versions for relay timestamps", () => {
    expect(announcedKey(relay)).toBe("announced:2026-08-13T11:30:00.000Z");
  });

  it("selects only one of two mirrors handed over in the same list", () => {
    expect(selectFreshResetNotifications([relay, manual], [], now)).toHaveLength(1);
  });
});

describe("latestLandedReset", () => {
  it("skips a reset still on its way and dates a scheduled one by when it landed", () => {
    const events = [
      event({ id: "incoming", announcedAt: "2026-08-13T11:00:00Z", occursAt: "2026-08-13T15:00:00Z" }),
      event({ id: "scheduled", announcedAt: "2026-08-09T09:00:00Z", occursAt: "2026-08-12T17:00:00Z" }),
      event({ id: "surprise", announcedAt: "2026-08-11T10:00:00Z" }),
    ];
    const landed = latestLandedReset(events, now);
    expect(landed?.event.id).toBe("scheduled");
    expect(landed?.at).toBe(Date.parse("2026-08-12T17:00:00Z"));
  });

  it("counts a scheduled reset that landed recently as recent, not by its old announcement", () => {
    const events = [
      event({ id: "scheduled", announcedAt: "2026-08-08T09:00:00Z", occursAt: "2026-08-12T17:00:00Z" }),
    ];
    expect(tiboStatus(events, now).label).toBe("Recently reset");
  });
});

describe("notifyFreshResets", () => {
  beforeEach(() => {
    localStorage.clear();
    Object.defineProperty(window, "__TAURI_INTERNALS__", { value: {}, configurable: true });
    plugin.sendNotification.mockResolvedValue(undefined);
    plugin.requestPermission.mockResolvedValue("granted");
  });

  afterEach(() => {
    vi.clearAllMocks();
    Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
  });

  it("does not send one announcement twice when called again mid-delivery", async () => {
    // The first call waits on the permission check (in the app, often a
    // system prompt) while a feed poll or pref change calls again.
    let grant: (value: boolean) => void = () => undefined;
    plugin.isPermissionGranted
      .mockImplementationOnce(() => new Promise<boolean>((resolve) => (grant = resolve)))
      .mockResolvedValue(true);
    const events = [event({ id: "tibo-1" })];

    const first = notifyFreshResets(events, now);
    const second = notifyFreshResets([...events], now);
    await vi.waitFor(() => expect(plugin.isPermissionGranted).toHaveBeenCalled());
    grant(true);
    await Promise.all([first, second]);

    expect(plugin.sendNotification).toHaveBeenCalledOnce();
  });

  it("still delivers a later batch after an earlier one failed", async () => {
    plugin.isPermissionGranted.mockRejectedValueOnce(new Error("bridge down")).mockResolvedValue(true);
    await notifyFreshResets([event({ id: "tibo-1" })], now);
    expect(plugin.sendNotification).not.toHaveBeenCalled();
    await notifyFreshResets([event({ id: "tibo-1" })], now);
    expect(plugin.sendNotification).toHaveBeenCalledOnce();
  });

  it("remembers deliveries across restarts", async () => {
    plugin.isPermissionGranted.mockResolvedValue(true);
    await notifyFreshResets([event({ id: "tibo-1" })], now);
    // A relaunch reads the delivered ids back from storage.
    await notifyFreshResets([event({ id: "tibo-1" })], now + 60_000);
    expect(plugin.sendNotification).toHaveBeenCalledOnce();
  });
});
