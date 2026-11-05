/**
 * Browser-boundary stubs: `fetch` (the reset feed and the Bluesky relays) and
 * `Notification` (what @tauri-apps/plugin-notification uses on desktop).
 * Nothing leaves the page: known feeds get fixtures, any other cross-origin
 * request is refused and logged, so a simulation is offline and repeatable.
 */

export type FeedOptions = {
  /** Simulation start, epoch ms. */
  startMs: number;
  /** Announce a Codex reset this many minutes after the start. */
  announcedResetInMinutes?: number;
  /** `live` lets feed requests through to the network. */
  mode: "fixture" | "live" | "empty";
};

const FEED_HOST = "raw.githubusercontent.com";
const BSKY_HOST = "public.api.bsky.app";

export function feedFixture({ startMs, announcedResetInMinutes }: FeedOptions): unknown {
  const at = (hoursBefore: number) => new Date(startMs - hoursBefore * 3_600_000).toISOString();
  const events: Array<Record<string, unknown>> = [
    {
      id: "sim-tibo-landed",
      announcedAt: at(52),
      source: "tibo",
      text: "Reset all propagated. Enjoy.",
      sourceUrl: "https://bsky.app/profile/thsottiaux-bot.eurosky.social/post/simlanded",
    },
    {
      id: "sim-tibo-scheduled",
      announcedAt: at(70),
      occursAt: at(55),
      source: "tibo",
      text: "Global reset landing tomorrow 10am PST for all paid ChatGPT accounts.",
      plansAffected: ["Plus", "Pro"],
      sourceUrl: "https://bsky.app/profile/thsottiaux-bot.eurosky.social/post/simscheduled",
    },
  ];
  if (announcedResetInMinutes !== undefined) {
    events.unshift({
      id: "sim-tibo-incoming",
      announcedAt: at(0.25),
      occursAt: new Date(startMs + announcedResetInMinutes * 60_000).toISOString(),
      source: "tibo",
      text: `Loading a full reset for all paid accounts in about ${announcedResetInMinutes} minutes.`,
      plansAffected: ["Plus", "Pro"],
      sourceUrl: "https://bsky.app/profile/thsottiaux-bot.eurosky.social/post/simincoming",
    });
  }
  return { updatedAt: new Date(startMs).toISOString(), events };
}

export function installFetch(options: FeedOptions, log: (message: string) => void): void {
  const realFetch = window.fetch.bind(window);
  window.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = new URL(typeof input === "string" ? input : input instanceof URL ? input.href : input.url, location.href);
    if (url.origin === location.origin) return realFetch(input, init);
    if (options.mode === "live" && (url.host === FEED_HOST || url.host === BSKY_HOST)) return realFetch(input, init);
    const json = (body: unknown) =>
      new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
    if (url.host === FEED_HOST && url.pathname.endsWith("/resets.json")) {
      return json(options.mode === "empty" ? { events: [] } : feedFixture(options));
    }
    if (url.host === BSKY_HOST) return json({ feed: [] });
    log(`blocked network request: ${url.href}`);
    throw new TypeError(`UsageBar simulator blocked ${url.host}`);
  };
}

export type SimNotification = { title: string; body?: string };

/** A desktop `Notification` that records instead of showing. */
export function installNotifications(
  permission: NotificationPermission,
  record: (notification: SimNotification) => void,
): void {
  let current: NotificationPermission = permission;
  class SimNotificationImpl extends EventTarget {
    static get permission(): NotificationPermission {
      return current;
    }
    static async requestPermission(): Promise<NotificationPermission> {
      if (current === "default") current = "granted";
      return current;
    }
    readonly title: string;
    readonly body: string;
    constructor(title: string, options?: NotificationOptions) {
      super();
      this.title = title;
      this.body = options?.body ?? "";
      record({ title, body: this.body });
    }
    close(): void {}
  }
  Object.defineProperty(window, "Notification", { value: SimNotificationImpl, configurable: true, writable: true });
}
