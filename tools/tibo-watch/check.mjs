#!/usr/bin/env node
/**
 * Tibo Watch checker — polls @thsottiaux's public timeline via free Nitter
 * RSS mirrors and merges surprise-reset announcements into data/resets.json.
 *
 * Usage:
 *   node tools/tibo-watch/check.mjs            # fetch + merge + write
 *   node tools/tibo-watch/check.mjs --dry-run  # fetch + print, write nothing
 *
 * Env overrides:
 *   TIBO_HANDLE     X handle to watch (default: thsottiaux)
 *   TIBO_INSTANCES  Comma-separated Nitter base URLs, tried in order
 *   TIBO_DATA_FILE  Path to resets.json (default: ../../data/resets.json)
 */
import { execFile } from "node:child_process";
import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";
import { mergeEvents, parseRssItems, toResetEvent } from "./lib.mjs";

const execFileAsync = promisify(execFile);

const HANDLE = process.env.TIBO_HANDLE || "thsottiaux";
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
      ["-sfL", "--compressed", "-A", USER_AGENT, "-H", "Accept: application/rss+xml, text/xml, */*", "-m", "20", url],
      { maxBuffer: 4 * 1024 * 1024 },
    );
    return stdout;
  } catch (curlError) {
    const response = await fetch(url, {
      headers: { "user-agent": USER_AGENT, accept: "application/rss+xml, text/xml, */*" },
      signal: AbortSignal.timeout(20_000),
    });
    if (!response.ok) throw new Error(`HTTP ${response.status} (curl also failed: ${curlError.message})`);
    return response.text();
  }
}

async function fetchTimeline() {
  let lastError = null;
  for (const base of INSTANCES) {
    const url = `${base}/${HANDLE}/rss`;
    for (let attempt = 1; attempt <= ATTEMPTS_PER_INSTANCE; attempt += 1) {
      try {
        // Nitter instances soft-fail with empty 200s or anti-bot HTML pages.
        const items = parseRssItems(await fetchBody(url), HANDLE);
        if (items.length === 0) throw new Error("no timeline items parsed");
        console.log(`tibo-watch: fetched ${items.length} tweets from ${url}`);
        return items;
      } catch (error) {
        lastError = error;
        console.warn(`tibo-watch: ${url} attempt ${attempt} failed (${error.message})`);
        if (attempt < ATTEMPTS_PER_INSTANCE) await sleep(RETRY_DELAY_MS);
      }
    }
  }
  console.warn(`tibo-watch: all instances failed (${lastError?.message ?? "unknown"}); keeping existing data`);
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
