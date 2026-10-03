import { describe, expect, it } from "vitest";
import { DEFAULT_PREFS, missingProviders, stateAfterRefreshFailure, withVisible } from "./providers";

describe("stateAfterRefreshFailure", () => {
  it("hides a meter that has never shown usage", () => {
    expect(stateAfterRefreshFailure({ connection: "starting" }, new Error("nope")).connection).toBe(
      "cli_not_found",
    );
  });

  it("keeps a Codex history that only has usage", () => {
    const next = stateAfterRefreshFailure(
      { connection: "connected", usage: { summary: { lifetimeTokens: 10 } } },
      new Error("offline"),
    );
    expect(next.connection).toBe("error");
    expect(next.usage).toEqual({ summary: { lifetimeTokens: 10 } });
    expect(next.diagnostic).toBe("offline");
  });

  it("stays hidden after a missing provider throws", () => {
    const next = stateAfterRefreshFailure(
      { connection: "cli_not_found", updatedAt: 10, rateLimits: { ok: true } },
      new Error("status -25293"),
    );
    expect(next).toEqual({ connection: "cli_not_found" });
  });
});

describe("missingProviders", () => {
  it("lists a tool with no login and a tool turned off in Settings", () => {
    const prefs = withVisible(DEFAULT_PREFS, "devin", false);
    const missing = missingProviders(prefs, {
      codex: { connection: "connected" },
      claude: { connection: "cli_not_found" },
      cursor: { connection: "starting" },
      opencode: { connection: "connected" },
      devin: { connection: "connected" },
      antigravity: { connection: "error" },
    });
    expect(missing.map((item) => item.id)).toEqual(["claude", "devin"]);
    expect(missing[0]?.detail).toMatch(/claude/);
    expect(missing[1]?.detail).toMatch(/Settings/);
  });
});
