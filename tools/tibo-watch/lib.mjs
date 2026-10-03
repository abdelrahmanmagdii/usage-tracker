/**
 * Pure helpers for the Tibo Watch scraper.
 * No I/O in this module so it stays trivially testable with node:test.
 */

/** X→Bluesky relays of @thsottiaux, read via the public AppView (no auth). */
export const BSKY_ACTORS = ["thsottiaux-bot.eurosky.social", "thsottiaux-mirr.selfhosted.social"];

export function bskyFeedUrl(actor, limit = 100) {
  return `https://public.api.bsky.app/xrpc/app.bsky.feed.getAuthorFeed?actor=${encodeURIComponent(actor)}&limit=${limit}&filter=posts_no_replies`;
}

/**
 * "reset" alone is too loose — Tibo jokes about resets ("Don't say reset",
 * "I previously promised a reset…"). These patterns target actual
 * announcements: completed resets or imminent ones with a timeframe.
 */
const ANNOUNCEMENT_PATTERNS = [
  /limits? (?:have|has) been reset/i,
  /(?:i|we)(?:'ve|’ve| have)(?: now| just)? reset/i,
  /\ball reset\b/i,
  /^[^.!?]*[.!?]\s*a reset and\b/i,
  /\b(?:full|banked) reset\b/i,
  /\breset (?:will |is going to )?land/i,
  /(?:^|[^\w'’])resets?(?:ting)? (?:all|every|usage|the limits|your)/i,
  /(?:nice|surprise|fresh|free|global) resets?\b/i,
  /resets? (?:(?:is|are) )?(?:now )?(?:live|done|out|landing|landed|incoming|rolling|propagat)/i,
  /enjoy (?:a|the|this|that)? ?\w* resets?\b/i,
];

function extractTag(chunk, tag) {
  const match = chunk.match(new RegExp(`<${tag}[^>]*>([\\s\\S]*?)<\\/${tag}>`));
  return match ? match[1].trim() : null;
}

function stripCdata(text) {
  return text.replace(/^<!\[CDATA\[/, "").replace(/\]\]>$/, "");
}

export function decodeEntities(text) {
  return text
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&#0?39;|&apos;/g, "'")
    .replace(/&nbsp;/g, " ")
    .replace(/&amp;/g, "&");
}

/**
 * Parses a Nitter-style RSS timeline into normalized tweet items.
 * Only the <title> (the author's own words) is used for text, so quoted
 * tweets embedded in <description> never leak into keyword matching.
 */
export function parseRssItems(xml, handle) {
  if (typeof xml !== "string") return [];
  const items = [];
  const itemPattern = /<item>([\s\S]*?)<\/item>/g;
  let match;
  while ((match = itemPattern.exec(xml)) !== null) {
    const chunk = match[1];
    const title = extractTag(chunk, "title");
    const pubDate = extractTag(chunk, "pubDate");
    const guid = extractTag(chunk, "guid");
    if (!title || !pubDate || !guid || !/^\d+$/.test(guid)) continue;
    const announced = new Date(pubDate);
    if (Number.isNaN(announced.getTime())) continue;
    items.push({
      id: `tibo-${guid}`,
      text: decodeEntities(stripCdata(title)),
      announcedAt: announced.toISOString(),
      sourceUrl: `https://x.com/${handle}/status/${guid}`,
    });
  }
  return items;
}

/**
 * The mirror relays append decoration the author's followers never see:
 * "QT: <tweet url>" tails (the link is to the *quoted* tweet, not this
 * post) and "📊 This post has a poll …" footers. Strip both so keyword
 * matching and notification text see only Tibo's words.
 */
function stripMirrorDecoration(text) {
  return text
    .replace(/\s*QT: https?:\/\/\S+/g, "")
    .replace(/\s*📊 This post has a poll[^\n]*/g, "")
    .trim();
}

/**
 * Parses a Bluesky getAuthorFeed response into the same normalized item
 * shape as parseRssItems. All Nitter mirrors have gone dark, so an
 * unofficial X→Bluesky relay of @thsottiaux is now the primary source —
 * the public AppView needs no auth.
 *
 * Ids use the post rkey (`tibo-bsky-…`), not a tweet id: Bluesky posts
 * carry no link to the original status, so cross-source dedup with
 * Nitter ids is impossible. Reposts are skipped (same as the RT rule).
 */
export function parseBskyFeed(jsonText, actor) {
  let data;
  try {
    data = JSON.parse(jsonText);
  } catch {
    return [];
  }
  const feed = Array.isArray(data?.feed) ? data.feed : [];
  const items = [];
  for (const entry of feed) {
    if (typeof entry?.reason?.$type === "string" && entry.reason.$type.includes("reasonRepost")) {
      continue;
    }
    const post = entry?.post;
    const text = typeof post?.record?.text === "string" ? post.record.text : null;
    const createdAt = post?.record?.createdAt;
    const rkey = typeof post?.uri === "string" ? post.uri.split("/").pop() : null;
    if (!text || typeof createdAt !== "string" || !rkey) continue;
    const announced = new Date(createdAt);
    if (Number.isNaN(announced.getTime())) continue;
    items.push({
      id: `tibo-bsky-${rkey}`,
      text: stripMirrorDecoration(text),
      announcedAt: announced.toISOString(),
      sourceUrl: `https://bsky.app/profile/${actor}/post/${rkey}`,
    });
  }
  return items;
}

/** True when a tweet looks like an actual reset announcement. Retweets of others are excluded. */
export function isResetTweet(text) {
  if (/^RT @/i.test(text)) return false;
  return ANNOUNCEMENT_PATTERNS.some((pattern) => pattern.test(text));
}

/**
 * Best-effort lead time ("in 2 hours", "in the next hour", "in ~30 min")
 * so the app can show and notify a time-to-effect. Returns minutes or null.
 */
export function parseLeadTimeMinutes(text) {
  if (/in (?:the )?next half (?:an )?hour\b/i.test(text)) return 30;
  if (/in (?:the )?next hour\b/i.test(text)) return 60;
  const hours = text.match(
    /in (?:about |around |roughly |approximately |~ ?|less than |under )?(\d+(?:\.\d+)?)\s*(?:h|hrs?|hours?)\b/i,
  );
  if (hours) return Math.round(Number.parseFloat(hours[1]) * 60);
  const minutes = text.match(
    /in (?:about |around |roughly |approximately |~ ?|less than |under )?(\d+)\s*(?:m|mins?|minutes?)\b/i,
  );
  if (minutes) return Number.parseInt(minutes[1], 10);
  return null;
}

const ZONES = {
  pt: "America/Los_Angeles",
  pst: "America/Los_Angeles",
  pdt: "America/Los_Angeles",
  et: "America/New_York",
  est: "America/New_York",
  edt: "America/New_York",
  utc: "UTC",
  gmt: "UTC",
};

/** Offset (ms) of `timeZone` from UTC at the given instant. */
function zoneOffsetMs(timeZone, atMs) {
  const parts = Object.fromEntries(
    new Intl.DateTimeFormat("en-US", {
      timeZone,
      hourCycle: "h23",
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    })
      .formatToParts(new Date(atMs))
      .map((part) => [part.type, part.value]),
  );
  const asUtc = Date.UTC(+parts.year, +parts.month - 1, +parts.day, +parts.hour, +parts.minute, +parts.second);
  return asUtc - Math.floor(atMs / 1000) * 1000;
}

/**
 * Wall-clock schedules ("landing tomorrow 10am PST", "today at 3:30pm ET").
 * The day is resolved in the named zone relative to the post time; "PST" is
 * treated as Pacific time year-round, as people write it. Returns ms or null.
 */
export function parseScheduledTime(text, announcedAt) {
  const match = text.match(
    /\b(?:land(?:s|ing)?|drops?|dropping|goes live|resets?|resetting)\b[^.!?]{0,30}?\b(today|tonight|tomorrow)?\s*(?:at\s+|@\s*)?(\d{1,2})(?::(\d{2}))?\s*(am|pm)\s*(pt|pst|pdt|et|est|edt|utc|gmt)\b/i,
  );
  const announcedMs = Date.parse(announcedAt);
  if (!match || !Number.isFinite(announcedMs)) return null;
  const [, day, hourText, minuteText, meridiem, zoneText] = match;
  const timeZone = ZONES[zoneText.toLowerCase()];
  let hour = Number.parseInt(hourText, 10) % 12;
  if (meridiem.toLowerCase() === "pm") hour += 12;
  const minute = minuteText ? Number.parseInt(minuteText, 10) : 0;
  if (hour > 23 || minute > 59) return null;
  const local = new Date(announcedMs + zoneOffsetMs(timeZone, announcedMs));
  const dayOffset = day?.toLowerCase() === "tomorrow" ? 1 : 0;
  const wallAsUtc = Date.UTC(
    local.getUTCFullYear(),
    local.getUTCMonth(),
    local.getUTCDate() + dayOffset,
    hour,
    minute,
  );
  let occurs = wallAsUtc - zoneOffsetMs(timeZone, wallAsUtc);
  // A bare "10am PT" already past today means the next one.
  if (!day && occurs <= announcedMs) occurs += 86_400_000;
  return occurs;
}

/** When an announced reset should take effect, as an ISO string, if stated. */
export function parseOccursAt(text, announcedAt) {
  const scheduled = parseScheduledTime(text, announcedAt);
  if (scheduled !== null) return new Date(scheduled).toISOString();
  const leadMinutes = parseLeadTimeMinutes(text);
  return leadMinutes
    ? new Date(Date.parse(announcedAt) + leadMinutes * 60_000).toISOString()
    : null;
}

const PLAN_NAMES = ["Free", "Go", "Plus", "Pro", "Business", "Team", "Enterprise", "Edu"];

/**
 * Plans a reset covers, when the post names them ("for all paid ChatGPT
 * accounts", "Plus and Pro"). Plan names are matched case-sensitively so
 * ordinary words ("go /fast", "pro tip") don't count. Returns null if unstated.
 */
export function parsePlans(text) {
  if (/\ball paid\b/i.test(text)) return ["All paid plans"];
  const named = PLAN_NAMES.filter((plan) =>
    new RegExp(`\\b${plan}\\b(?!\\s*/)`).test(text),
  ).filter((plan) => plan !== "Go" || /\bGo\b(?=\s*(?:,|and|&|plans?|users|accounts|subscribers))/.test(text));
  return named.length ? named : null;
}

/** Converts a parsed RSS item into a ResetEvent, or null when unrelated. */
export function toResetEvent(item) {
  if (!isResetTweet(item.text)) return null;
  const occursAt = parseOccursAt(item.text, item.announcedAt);
  const plansAffected = parsePlans(item.text);
  return {
    id: item.id,
    announcedAt: item.announcedAt,
    ...(occursAt ? { occursAt } : {}),
    ...(plansAffected ? { plansAffected } : {}),
    source: "tibo",
    text: item.text.slice(0, 280),
    sourceUrl: item.sourceUrl,
  };
}

const MAX_EVENTS = 500;

/**
 * Merges scraped events into the stored feed. Existing entries always win,
 * so hand-written (manual) backfill entries are never modified or removed.
 */
export function mergeEvents(existing, incoming) {
  const byId = new Map();
  for (const event of Array.isArray(existing) ? existing : []) {
    if (event && typeof event.id === "string") byId.set(event.id, event);
  }
  // Each relay mirrors the same tweet under its own post id, but keeps the
  // tweet's timestamp, so the timestamp identifies a tweet across mirrors.
  const announced = new Set([...byId.values()].map((event) => event.announcedAt));
  let added = 0;
  for (const event of incoming) {
    if (!byId.has(event.id) && !announced.has(event.announcedAt)) {
      byId.set(event.id, event);
      announced.add(event.announcedAt);
      added += 1;
    }
  }
  const events = [...byId.values()]
    .sort(
      (a, b) =>
        Date.parse(b.occurredAt ?? b.announcedAt) - Date.parse(a.occurredAt ?? a.announcedAt),
    )
    .slice(0, MAX_EVENTS);
  return { events, added };
}
