import type { ResetEvent } from "../../types/codex";
import { readDetectedEvents } from "../../lib/history";
import { isRecord } from "../../lib/rateLimits";
import { BSKY_ACTORS, bskyFeedUrl, mergeEvents, parseBskyFeed, toResetEvent } from "../../../tools/tibo-watch/lib.mjs";

export interface ResetEventProvider {
  readonly id: string;
  listEvents(): Promise<ResetEvent[]>;
}

const DEFAULT_FEED_URL =
  "https://raw.githubusercontent.com/abdelrahmanmagdii/usage-tracker/main/data/resets.json";
const FEED_URL =
  (import.meta.env.VITE_TIBO_FEED_URL as string | undefined) || DEFAULT_FEED_URL;
const FEED_CACHE_KEY = "codex-meter.feed-cache.v1";
// Short TTL: the whole point of the feed is catching "reset landing in the
// next hour" announcements while the old quota is still worth burning.
const FEED_TTL_MS = 4 * 60 * 1000;

const sampleEvents: ResetEvent[] = [
  {
    id: "sample-tibo-2026-08-10",
    announcedAt: "2026-08-10T18:20:00Z",
    occurredAt: "2026-08-10T18:20:00Z",
    source: "tibo",
    text: "Sample surprise reset announcement",
    plansAffected: ["Plus", "Pro"],
    sample: true,
  },
  {
    id: "sample-openai-2026-07-25",
    announcedAt: "2026-07-25T16:00:00Z",
    occurredAt: "2026-07-25T16:00:00Z",
    source: "openai",
    text: "Sample public reset event",
    sample: true,
  },
];

export function normalizeFeedEvent(value: unknown): ResetEvent | null {
  if (!isRecord(value) || typeof value.id !== "string" || typeof value.announcedAt !== "string") {
    return null;
  }
  if (Number.isNaN(Date.parse(value.announcedAt))) return null;
  const source = value.source;
  return {
    id: value.id,
    announcedAt: value.announcedAt,
    occurredAt: typeof value.occurredAt === "string" ? value.occurredAt : undefined,
    occursAt: typeof value.occursAt === "string" ? value.occursAt : undefined,
    sourceUrl: typeof value.sourceUrl === "string" ? value.sourceUrl : undefined,
    source:
      source === "tibo" || source === "openai" || source === "manual" || source === "detected"
        ? source
        : "tibo",
    text: typeof value.text === "string" ? value.text : undefined,
    plansAffected: Array.isArray(value.plansAffected)
      ? value.plansAffected.filter((plan): plan is string => typeof plan === "string")
      : undefined,
  };
}

function readCachedFeed(): ResetEvent[] {
  try {
    const cached: unknown = JSON.parse(localStorage.getItem(FEED_CACHE_KEY) ?? "[]");
    if (!Array.isArray(cached)) return [];
    return cached
      .map(normalizeFeedEvent)
      .filter((event): event is ResetEvent => event !== null);
  } catch {
    return [];
  }
}

/** Fetches the community reset feed (GitHub-hosted resets.json) with a local cache fallback. */
export class FeedResetEventProvider implements ResetEventProvider {
  readonly id = "feed";
  private memoryCache: ResetEvent[] | null = null;
  private lastFetchMs = 0;

  async listEvents(): Promise<ResetEvent[]> {
    if (this.memoryCache && Date.now() - this.lastFetchMs < FEED_TTL_MS) {
      return this.memoryCache;
    }
    try {
      // The cache-busting query skips raw.githubusercontent's ~5-minute CDN
      // cache, so a freshly committed event is visible on the next poll.
      const separator = FEED_URL.includes("?") ? "&" : "?";
      const response = await fetch(`${FEED_URL}${separator}t=${Date.now()}`, { cache: "no-store" });
      if (!response.ok) throw new Error(`Feed returned HTTP ${response.status}`);
      const payload: unknown = await response.json();
      const raw = isRecord(payload) && Array.isArray(payload.events) ? payload.events : [];
      const events = raw
        .map(normalizeFeedEvent)
        .filter((event): event is ResetEvent => event !== null);
      this.memoryCache = events;
      this.lastFetchMs = Date.now();
      try {
        localStorage.setItem(FEED_CACHE_KEY, JSON.stringify(events));
      } catch {
        /* storage full or unavailable — the memory cache still applies */
      }
      return events;
    } catch {
      return this.memoryCache ?? readCachedFeed();
    }
  }
}

/**
 * Reads the X→Bluesky relays directly, using the same detection rules as the
 * GitHub watcher. GitHub runs scheduled workflows hours apart, so the hosted
 * feed alone delivers most announcements too late to act on.
 */
export class BlueskyResetEventProvider implements ResetEventProvider {
  readonly id = "bluesky";
  private memoryCache: ResetEvent[] | null = null;
  private lastFetchMs = 0;

  constructor(
    private readonly actors: string[] = BSKY_ACTORS,
    private readonly fetcher: typeof fetch = (...args) => fetch(...args),
  ) {}

  async listEvents(): Promise<ResetEvent[]> {
    if (this.memoryCache && Date.now() - this.lastFetchMs < FEED_TTL_MS) {
      return this.memoryCache;
    }
    const results = await Promise.allSettled(
      this.actors.map(async (actor) => {
        const response = await this.fetcher(bskyFeedUrl(actor, 50), { cache: "no-store" });
        if (!response.ok) throw new Error(`Bluesky returned HTTP ${response.status}`);
        return parseBskyFeed(await response.text(), actor)
          .map(toResetEvent)
          .filter((event): event is ResetEvent => event !== null);
      }),
    );
    const fetched = results.flatMap((result) => (result.status === "fulfilled" ? result.value : []));
    if (results.every((result) => result.status === "rejected")) return this.memoryCache ?? [];
    this.memoryCache = mergeEvents([], fetched).events;
    this.lastFetchMs = Date.now();
    return this.memoryCache;
  }
}

export class LocalResetEventProvider implements ResetEventProvider {
  readonly id = "local";

  async listEvents(): Promise<ResetEvent[]> {
    const local = readDetectedEvents();
    return [...(import.meta.env.DEV ? sampleEvents : []), ...local].sort(
      (a, b) => Date.parse(b.occurredAt ?? b.announcedAt) - Date.parse(a.occurredAt ?? a.announcedAt),
    );
  }
}

/**
 * Merges every provider, keeping the first occurrence of each event. Tibo
 * events are also keyed by tweet timestamp, since each relay mirrors a
 * tweet under its own post id.
 */
export class CombinedResetEventProvider implements ResetEventProvider {
  readonly id = "combined";

  constructor(
    private readonly providers: ResetEventProvider[] = [
      new FeedResetEventProvider(),
      new BlueskyResetEventProvider(),
      new LocalResetEventProvider(),
    ],
  ) {}

  async listEvents(): Promise<ResetEvent[]> {
    const lists = await Promise.all(
      this.providers.map((provider) =>
        provider.listEvents().catch(() => [] as ResetEvent[]),
      ),
    );
    const byId = new Map<string, ResetEvent>();
    const announced = new Set<string>();
    for (const event of lists.flat()) {
      if (byId.has(event.id)) continue;
      if (event.source === "tibo") {
        if (announced.has(event.announcedAt)) continue;
        announced.add(event.announcedAt);
      }
      byId.set(event.id, event);
    }
    return [...byId.values()].sort(
      (a, b) => Date.parse(b.occurredAt ?? b.announcedAt) - Date.parse(a.occurredAt ?? a.announcedAt),
    );
  }
}

/** The next announced reset that has not taken effect yet, if any. */
export function upcomingReset(events: ResetEvent[], nowMs = Date.now()): ResetEvent | undefined {
  return events
    .filter((event) => event.occursAt && Date.parse(event.occursAt) > nowMs)
    .sort((a, b) => Date.parse(a.occursAt as string) - Date.parse(b.occursAt as string))[0];
}

export function tiboStatus(events: ResetEvent[], nowMs = Date.now()) {
  if (upcomingReset(events, nowMs)) {
    return { tone: "danger", label: "Reset incoming" } as const;
  }
  const latest = events[0];
  if (!latest) return { tone: "warning", label: "Watching for resets" } as const;
  const ageDays = (nowMs - Date.parse(latest.occurredAt ?? latest.announcedAt)) / 86_400_000;
  if (ageDays <= 3) return { tone: "success", label: "Recently reset" } as const;
  return { tone: "warning", label: "No recent resets" } as const;
}
