#!/usr/bin/env node
/**
 * Tibo Watch checker — polls @thsottiaux's public timeline and merges
 * surprise-reset announcements into data/resets.json.
 *
 * Sources, tried in order:
 *   1. Bluesky mirrors of @thsottiaux via the public AppView (no auth) —
 *      the primary source; every public Nitter instance has gone dark.
 *   2. Free Nitter RSS mirrors, kept as a fallback in case one comes back.
 *
 * Usage:
 *   node tools/tibo-watch/check.mjs            # fetch + merge + write
 *   node tools/tibo-watch/check.mjs --dry-run  # fetch + print, write nothing
 *
 * Env overrides:
 *   TIBO_HANDLE       X handle to watch (default: thsottiaux)
 *   TIBO_BSKY_ACTORS  Comma-separated Bluesky relay handles, tried in order
 *   TIBO_INSTANCES    Comma-separated Nitter base URLs, tried in order
 *   TIBO_DATA_FILE    Path to resets.json (default: ../../data/resets.json)
 */
import { execFile } from "node:child_process";
import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";
import { mergeEvents, parseBskyFeed, parseRssItems, toResetEvent } from "./lib.mjs";

const execFileAsync = promisify(execFile);

const HANDLE = process.env.TIBO_HANDLE || "thsottiaux";
const BSKY_API = "https://public.api.bsky.app/xrpc/app.bsky.feed.getAuthorFeed";
const BSKY_ACTORS = (
  process.env.TIBO_BSKY_ACTORS ||
  "thsottiaux-bot.eurosky.social,thsottiaux-mirr.selfhosted.social"
)
  .split(",")
  .map((value) => value.trim())
  .filter(Boolean);
const INSTANCES = (
  process.env.TIBO_INSTANCES ||
  "https://nitter.net,https://nitter.privacyredirect.com,https://nitter.tiekoetter.com"
)
  .split(",")
  .map((value) => value.trim().replace(/\/$/, ""))
  .filter(Boolean);
const DATA_FILE =
  process.env.TIBO_DATA_FILE ||
  path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../data/resets.json");
const DRY_RUN = process.argv.includes("--dry-run");
const USER_AGENT =
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Safari/605.1.15";

const ATTEMPTS_PER_INSTANCE = 2;
const RETRY_DELAY_MS = 4_000;

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/**
 * curl first: several Nitter fronts answer HTTP/1.1 fetch clients with empty
 * 200s, while curl (HTTP/2) gets the real feed. Node's global fetch is the
 * fallback for environments without curl.
 */
async function fetchBody(url) {
  try {
    const { stdout } = await execFileAsync(
      "curl",
      ["-sfL", "--compressed", "-A", USER_AGENT, "-H", "Accept: application/json, application/rss+xml, text/xml, */*", "-m", "20", url],
      { maxBuffer: 4 * 1024 * 1024 },
    );
    return stdout;
  } catch (curlError) {
    const response = await fetch(url, {
      headers: { "user-agent": USER_AGENT, accept: "application/json, application/rss+xml, text/xml, */*" },
      signal: AbortSignal.timeout(20_000),
    });
    if (!response.ok) throw new Error(`HTTP ${response.status} (curl also failed: ${curlError.message})`);
    return response.text();
  }
}

function sources() {
  return [
    ...BSKY_ACTORS.map((actor) => ({
      url: `${BSKY_API}?actor=${encodeURIComponent(actor)}&limit=25&filter=posts_no_replies`,
      parse: (body) => parseBskyFeed(body, actor),
    })),
    ...INSTANCES.map((base) => ({
      url: `${base}/${HANDLE}/rss`,
      // Nitter instances soft-fail with empty 200s or anti-bot HTML pages.
      parse: (body) => parseRssItems(body, HANDLE),
    })),
  ];
}

async function fetchTimeline() {
  let lastError = null;
  for (const source of sources()) {
    for (let attempt = 1; attempt <= ATTEMPTS_PER_INSTANCE; attempt += 1) {
      try {
        const items = source.parse(await fetchBody(source.url));
        if (items.length === 0) throw new Error("no timeline items parsed");
        console.log(`tibo-watch: fetched ${items.length} posts from ${source.url}`);
        return items;
      } catch (error) {
        lastError = error;
        console.warn(`tibo-watch: ${source.url} attempt ${attempt} failed (${error.message})`);
        if (attempt < ATTEMPTS_PER_INSTANCE) await sleep(RETRY_DELAY_MS);
      }
    }
  }
  console.warn(`tibo-watch: all sources failed (${lastError?.message ?? "unknown"}); keeping existing data`);
  return null;
}

async function readFeed() {
  try {
    const parsed = JSON.parse(await readFile(DATA_FILE, "utf8"));
    return { updatedAt: parsed.updatedAt ?? null, events: Array.isArray(parsed.events) ? parsed.events : [] };
  } catch {
    return { updatedAt: null, events: [] };
  }
}

const items = await fetchTimeline();
if (!items) process.exit(0);

const resets = items.map(toResetEvent).filter(Boolean);
const feed = await readFeed();
const { events, added } = mergeEvents(feed.events, resets);
console.log(`tibo-watch: ${resets.length} reset tweets in timeline, ${added} new, ${events.length} total stored`);

if (DRY_RUN) {
  console.log(JSON.stringify({ updatedAt: feed.updatedAt, events: events.slice(0, 5) }, null, 2));
} else if (added > 0 || feed.updatedAt === null) {
  const next = { updatedAt: new Date().toISOString(), events };
  await writeFile(DATA_FILE, `${JSON.stringify(next, null, 2)}\n`, "utf8");
  console.log(`tibo-watch: wrote ${DATA_FILE}`);
} else {
  console.log("tibo-watch: nothing to write");
}
