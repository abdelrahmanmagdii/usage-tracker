import { describe, expect, it } from "vitest";
import { isDevPreview, previewPrefs, previewResetEvent } from "./preview";

describe("preview query helpers", () => {
  it("stays off without the preview flag", () => {
    expect(isDevPreview("")).toBe(false);
    expect(previewPrefs("")).toBeNull();
    expect(previewResetEvent(0, "")).toBeNull();
  });

  it("filters visible tools from the query string", () => {
    const prefs = previewPrefs("?preview&providers=codex,claude");
    expect(prefs?.providers.codex?.visible).toBe(true);
    expect(prefs?.providers.claude?.visible).toBe(true);
    expect(prefs?.providers.cursor?.visible).toBe(false);
    expect(prefs?.providers.devin?.visible).toBe(false);
    expect(prefs?.providers.antigravity?.visible).toBe(false);
  });

  it("builds an upcoming reset only when alert is set", () => {
    const now = Date.parse("2026-09-20T18:00:00Z");
    expect(previewResetEvent(now, "?preview")).toBeNull();
    const event = previewResetEvent(now, "?preview&alert&resetIn=42");
    expect(event?.id).toBe("preview-reset");
    expect(event?.occursAt).toBe("2026-09-20T18:42:00.000Z");
  });
});
