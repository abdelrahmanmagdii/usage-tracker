/**
 * Pure helpers for the Tibo Watch scraper.
 * No I/O in this module so it stays trivially testable with node:test.
 */

/**
 * "reset" alone is too loose — Tibo jokes about resets ("Don't say reset",
 * "I previously promised a reset…"). These patterns target actual
 * announcements: completed resets or imminent ones with a timeframe.
 */
const ANNOUNCEMENT_PATTERNS = [
  /limits? (?:have|has) been reset/i,
  /(?:i|we)(?:'ve| have) reset/i,
  /reset(?:ting)? (?:all|every|usage|the limits|your)/i,
  /(?:nice|surprise|fresh|free) resets?\b/i,
  /resets? (?:is|are) (?:live|done|out|landing|incoming|rolling)/i,
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
    /in (?:about |around |roughly |approximately |~|less than |under )?(\d+(?:\.\d+)?)\s*(?:h|hrs?|hours?)\b/i,
  );
  if (hours) return Math.round(Number.parseFloat(hours[1]) * 60);
  const minutes = text.match(
    /in (?:about |around |roughly |approximately |~|less than |under )?(\d+)\s*(?:m|mins?|minutes?)\b/i,
  );
  if (minutes) return Number.parseInt(minutes[1], 10);
  return null;
}

/** Converts a parsed RSS item into a ResetEvent, or null when unrelated. */
export function toResetEvent(item) {
  if (!isResetTweet(item.text)) return null;
  const leadMinutes = parseLeadTimeMinutes(item.text);
  return {
    id: item.id,
    announcedAt: item.announcedAt,
    ...(leadMinutes
      ? { occursAt: new Date(Date.parse(item.announcedAt) + leadMinutes * 60_000).toISOString() }
      : {}),
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
  let added = 0;
  for (const event of incoming) {
    if (!byId.has(event.id)) {
      byId.set(event.id, event);
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
