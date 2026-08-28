import { describe, expect, it } from "vitest";
import type { RateLimitBucket } from "../../types/codex";
import {
  SITE_URL,
  linkedInShareText,
  linkedInShareUrl,
  xIntentUrl,
  xShareText,
} from "./shareCopy";

const bucket: RateLimitBucket = {
  id: "codex:primary:300",
  limitId: "codex",
  limitName: "Codex",
  windowKind: "primary",
  usedPercent: 37,
  remainingPercent: 63,
  windowDurationMins: 300,
  resetsAt: Date.UTC(2026, 7, 12, 22, 12, 0) / 1000,
  reached: false,
};

const now = Date.UTC(2026, 7, 12, 20, 0, 0);

describe("share copy", () => {
  it("keeps the X caption under 280 characters and names the window", () => {
    const text = xShareText(bucket, now);
    expect(text.length).toBeLessThanOrEqual(280);
    expect(text).toContain("63% left");
    expect(text).toContain("5-hour Codex");
    expect(text).toContain(SITE_URL);
    expect(xIntentUrl(text)).toContain("https://x.com/intent/tweet?text=");
  });

  it("uses a named window label on LinkedIn and points at the public site", () => {
    const named: RateLimitBucket = { ...bucket, windowLabel: "Fable", limitName: "Weekly limit" };
    const text = linkedInShareText(named, now);
    expect(text).toContain("Fable Weekly limit");
    expect(text).toContain("on-device");
    expect(text).toContain(SITE_URL);
    expect(linkedInShareUrl()).toContain(encodeURIComponent(SITE_URL));
  });

  it("still produces a post when reset time is missing", () => {
    const text = xShareText({ ...bucket, resetsAt: undefined }, now);
    expect(text).toContain("reset time not reported");
    expect(text.length).toBeLessThanOrEqual(280);
  });
});
