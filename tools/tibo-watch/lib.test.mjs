import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  decodeEntities,
  isResetTweet,
  mergeEvents,
  parseLeadTimeMinutes,
  parseRssItems,
  toResetEvent,
} from "./lib.mjs";

const SAMPLE_RSS = `<?xml version="1.0" encoding="UTF-8"?>
<rss xmlns:atom="http://www.w3.org/2005/Atom" xmlns:dc="http://purl.org/dc/elements/1.1/" version="2.0">
  <channel>
    <title>Tibo / @thsottiaux</title>
    <item>
      <title>Old news actually from a bunch of days ago, but crossed that 15M. Enjoy a nice reset everyone. Landing in the next hour or so, go /fast.</title>
      <dc:creator>@thsottiaux</dc:creator>
      <pubDate>Thu, 13 Aug 2026 01:01:37 GMT</pubDate>
      <guid isPermaLink="false">2087706104814023111</guid>
      <link>https://nitter.net/thsottiaux/status/2087706104814023111#m</link>
    </item>
    <item>
      <title>Typical conversation with @ajambrosino &amp; friends</title>
      <pubDate>Wed, 12 Aug 2026 22:03:35 GMT</pubDate>
      <guid isPermaLink="false">2087660979342512391</guid>
      <link>https://nitter.net/thsottiaux/status/2087660979342512391#m</link>
    </item>
    <item>
      <title>broken item without guid</title>
      <pubDate>Wed, 12 Aug 2026 21:00:00 GMT</pubDate>
    </item>
  </channel>
</rss>`;

describe("parseRssItems", () => {
  it("extracts well-formed items and skips broken ones", () => {
    const items = parseRssItems(SAMPLE_RSS, "thsottiaux");
    assert.equal(items.length, 2);
    assert.equal(items[0].id, "tibo-2087706104814023111");
    assert.equal(items[0].announcedAt, "2026-08-13T01:01:37.000Z");
    assert.equal(items[0].sourceUrl, "https://x.com/thsottiaux/status/2087706104814023111");
    assert.equal(items[1].text, "Typical conversation with @ajambrosino & friends");
  });

  it("returns nothing for malformed input", () => {
    assert.deepEqual(parseRssItems("not xml at all", "thsottiaux"), []);
    assert.deepEqual(parseRssItems(null, "thsottiaux"), []);
  });
});

describe("isResetTweet", () => {
  it("matches real reset announcements", () => {
    assert.ok(isResetTweet("Enjoy a nice reset everyone. Landing in the next hour or so"));
    assert.ok(isResetTweet("Usage limits have been reset for all paid ChatGPT Work and Codex users."));
    assert.ok(isResetTweet("I have reset usage limits for all paid users. Have fun out there!"));
    assert.ok(isResetTweet("Resetting all Codex limits in 30 minutes"));
    assert.ok(isResetTweet("surprise resets for everyone"));
    assert.ok(isResetTweet("R to @thsottiaux: Usage limits have been reset for all paid users"));
  });

  it("ignores meta commentary, jokes, unrelated tweets and retweets", () => {
    assert.ok(!isResetTweet("What could we improve? Don't say reset."));
    assert.ok(!isResetTweet("I previously promised a reset for every 1M in additional active users"));
    assert.ok(!isResetTweet("Typical conversation with @ajambrosino"));
    assert.ok(!isResetTweet("RT @someone: enjoy a nice reset everyone"));
    assert.ok(!isResetTweet("Mindset is everything"));
  });
});

describe("parseLeadTimeMinutes", () => {
  it("parses common phrasings", () => {
    assert.equal(parseLeadTimeMinutes("Landing in the next hour or so, go /fast."), 60);
    assert.equal(parseLeadTimeMinutes("Resetting limits in 30 minutes"), 30);
    assert.equal(parseLeadTimeMinutes("limits reset in 2 hours"), 120);
    assert.equal(parseLeadTimeMinutes("limits reset in ~45 min"), 45);
    assert.equal(parseLeadTimeMinutes("dropping in about 1.5 hours"), 90);
    assert.equal(parseLeadTimeMinutes("in the next half hour"), 30);
  });

  it("returns null when no lead time is present", () => {
    assert.equal(parseLeadTimeMinutes("Enjoy a nice reset everyone."), null);
    assert.equal(parseLeadTimeMinutes("we reset things yesterday"), null);
  });
});

describe("toResetEvent", () => {
  it("builds an event with a parsed occursAt", () => {
    const [item] = parseRssItems(SAMPLE_RSS, "thsottiaux");
    const event = toResetEvent(item);
    assert.equal(event.id, "tibo-2087706104814023111");
    assert.equal(event.source, "tibo");
    assert.equal(event.occursAt, "2026-08-13T02:01:37.000Z");
  });

  it("returns null for unrelated tweets", () => {
    const [, item] = parseRssItems(SAMPLE_RSS, "thsottiaux");
    assert.equal(toResetEvent(item), null);
  });
});

describe("mergeEvents", () => {
  it("adds new events, keeps existing entries untouched, and sorts newest first", () => {
    const manual = {
      id: "manual-2025-11-02",
      announcedAt: "2025-11-02T10:00:00.000Z",
      source: "manual",
      text: "Hand-recorded reset",
    };
    const existing = {
      id: "tibo-1",
      announcedAt: "2026-08-13T01:01:37.000Z",
      source: "tibo",
      text: "Original text I edited by hand",
    };
    const rescrape = { ...existing, text: "Scraped text that must not overwrite" };
    const fresh = {
      id: "tibo-2",
      announcedAt: "2026-08-13T05:00:00.000Z",
      source: "tibo",
    };
    const { events, added } = mergeEvents([manual, existing], [rescrape, fresh]);
    assert.equal(added, 1);
    assert.equal(events.length, 3);
    assert.equal(events[0].id, "tibo-2");
    assert.equal(events.find((event) => event.id === "tibo-1").text, existing.text);
    assert.ok(events.some((event) => event.id === "manual-2025-11-02"));
  });

  it("tolerates garbage in the existing store", () => {
    const { events, added } = mergeEvents(null, []);
    assert.deepEqual(events, []);
    assert.equal(added, 0);
  });
});

describe("decodeEntities", () => {
  it("decodes common entities", () => {
    assert.equal(decodeEntities("fish &amp; chips &lt;3 &#39;tis&quot;"), "fish & chips <3 'tis\"");
  });
});
