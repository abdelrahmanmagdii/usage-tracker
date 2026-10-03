import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  decodeEntities,
  isResetTweet,
  mergeEvents,
  parseBskyFeed,
  parseLeadTimeMinutes,
  parseOccursAt,
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

const SAMPLE_BSKY_FEED = JSON.stringify({
  feed: [
    {
      post: {
        uri: "at://did:plc:abc/app.bsky.feed.post/3mweyqd7dl42r",
        record: {
          createdAt: "2026-09-26T00:07:13.000Z",
          text: "o yes… we’re back in action and we’ll reset usage limits for all paid users",
        },
      },
    },
    {
      post: {
        uri: "at://did:plc:abc/app.bsky.feed.post/3mw7bvf2cnm22",
        record: {
          createdAt: "2026-09-23T17:35:48.000Z",
          // The relay's QT tail links the *quoted* tweet — decoration, not text.
          text: "Voice! check it out\n\nQT: https://twitter.com/i/status/2102808325742322002",
        },
      },
    },
    {
      // A repost of someone else's post — must be skipped.
      reason: { $type: "app.bsky.feed.defs#reasonRepost" },
      post: {
        uri: "at://did:plc:xyz/app.bsky.feed.post/repost1",
        record: {
          createdAt: "2026-09-25T00:00:00.000Z",
          text: "usage limits have been reset for everyone",
        },
      },
    },
    {
      // A malformed entry is skipped, not fatal.
      post: { uri: "at://did:plc:abc/app.bsky.feed.post/broken" },
    },
  ],
});

describe("parseBskyFeed", () => {
  it("normalizes posts, strips relay decoration, and skips reposts", () => {
    const items = parseBskyFeed(SAMPLE_BSKY_FEED, "thsottiaux-bot.eurosky.social");
    assert.equal(items.length, 2);
    assert.equal(items[0].id, "tibo-bsky-3mweyqd7dl42r");
    assert.equal(items[0].announcedAt, "2026-09-26T00:07:13.000Z");
    assert.equal(
      items[0].sourceUrl,
      "https://bsky.app/profile/thsottiaux-bot.eurosky.social/post/3mweyqd7dl42r",
    );
    assert.equal(items[1].text, "Voice! check it out");
  });

  it("returns nothing for malformed input", () => {
    assert.deepEqual(parseBskyFeed("not json", "actor"), []);
    assert.deepEqual(parseBskyFeed("{}", "actor"), []);
    assert.deepEqual(parseBskyFeed(null, "actor"), []);
  });

  it("feeds real reset announcements through toResetEvent", () => {
    const [item] = parseBskyFeed(SAMPLE_BSKY_FEED, "actor");
    const event = toResetEvent(item);
    assert.equal(event.id, "tibo-bsky-3mweyqd7dl42r");
    assert.equal(event.source, "tibo");
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

  // Real @thsottiaux posts (via the Bluesky relays) that earlier patterns missed.
  it("matches real phrasings seen on the relays", () => {
    assert.ok(isResetTweet("Global reset landing tomorrow 10am PST for all paid ChatGPT accounts."));
    assert.ok(isResetTweet("Resets all propagated. That will be all. Have a fantastic weekend."));
    assert.ok(isResetTweet("Reset all propagated. Enjoy."));
    assert.ok(isResetTweet("All reset for everyone. Enjoy the week with Astra."));
    assert.ok(isResetTweet("Hi Astra users. A reset and a quick update on quality issues that have been posted around. (1/5)"));
    assert.ok(isResetTweet("Your Codex and ChatGPT Work reset will land at 6pm PST."));
    assert.ok(isResetTweet("we have now reset usage for all paid subscriptions for ChatGPT Work and Codex."));
    assert.ok(isResetTweet("Some Plus and Business users won't yet get access to Astra today, we've got you covered with a banked reset. Lands by end of day"));
  });

  it("ignores real posts that mention resets without announcing one", () => {
    assert.ok(!isResetTweet("Seeing some reports that the Pro 500 didn’t get the reset as expected earlier. Investigating and will make up for it"));
    assert.ok(!isResetTweet("Because usage on your primary dot is virtually unlimited at the moment, I can’t really give a reset."));
    assert.ok(!isResetTweet("We are almost Tuesday and I promised a reset for Tuesday. Among some other things."));
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

describe("parseOccursAt", () => {
  it("resolves wall-clock schedules in the named zone", () => {
    // 02:14Z on Oct 2 is still Oct 1 in Pacific time, so "tomorrow" is Oct 2.
    assert.equal(
      parseOccursAt("Global reset landing tomorrow 10am PST for all paid ChatGPT accounts.", "2026-10-02T02:14:51.000Z"),
      "2026-10-02T17:00:00.000Z",
    );
    assert.equal(
      parseOccursAt("Your Codex and ChatGPT Work reset will land at 6pm PST.", "2026-08-30T19:24:37.000Z"),
      "2026-08-31T01:00:00.000Z",
    );
    assert.equal(parseOccursAt("Reset lands today at 3:30pm ET", "2026-09-10T12:00:00.000Z"), "2026-09-10T19:30:00.000Z");
  });

  it("ignores times that aren't about the reset landing", () => {
    assert.equal(
      parseOccursAt("a banked reset. Lands by end of day and if you create your account by 8pm PT then you'll get it too.", "2026-09-04T20:57:17.000Z"),
      null,
    );
  });

  it("falls back to relative lead times", () => {
    assert.equal(parseOccursAt("First one will land in ~ 3 hours.", "2026-09-03T23:12:09.000Z"), "2026-09-04T02:12:09.000Z");
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

  it("stores a tweet seen through two mirrors once", () => {
    const at = "2026-10-02T21:18:48.000Z";
    const { events, added } = mergeEvents(
      [{ id: "tibo-bsky-a", announcedAt: at, source: "tibo" }],
      [{ id: "tibo-bsky-b", announcedAt: at, source: "tibo" }],
    );
    assert.equal(added, 0);
    assert.deepEqual(events.map((event) => event.id), ["tibo-bsky-a"]);
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
