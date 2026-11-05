import { describe, expect, it } from "vitest";
import {
  DEFAULT_PREFS,
  PROVIDER_CATALOG,
  PROVIDER_IDS,
  PROVIDER_SETUP,
  missingProviders,
  onboardingStatus,
  providerChannels,
  stateAfterRefreshFailure,
  withVisible,
  type ProviderId,
} from "./providers";
import { deviceName } from "./platform";

const providerById = (id: ProviderId) => PROVIDER_CATALOG.find((provider) => provider.id === id)!;

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

describe("PROVIDER_CATALOG", () => {
  it("lists every provider once, in display order", () => {
    expect(PROVIDER_CATALOG.map((provider) => provider.id)).toEqual([...PROVIDER_IDS]);
  });

  it("gives every section a glyph of its own", () => {
    const icons = PROVIDER_CATALOG.flatMap((provider) => (provider.icon ? [provider.icon] : []));
    expect(icons).toHaveLength(PROVIDER_IDS.length - 1);
    expect(new Set(icons).size).toBe(icons.length);
    expect(providerById("gemini").icon).not.toBe(providerById("antigravity").icon);
  });

  it("has a signed-out hint for every tool that renders through ProviderSection", () => {
    for (const provider of PROVIDER_CATALOG) {
      if (provider.id === "codex") expect(provider.signedOutHint).toBeNull();
      else expect(provider.signedOutHint).toBeTruthy();
    }
  });

  it("names the device instead of assuming a Mac", () => {
    expect(PROVIDER_SETUP.cursor).toBe(`Sign in through the Cursor app on this ${deviceName}.`);
    expect(PROVIDER_SETUP.devin).toContain(`on this ${deviceName}`);
  });
});

describe("providerChannels", () => {
  it("follows the backend's command and event names", () => {
    expect(providerChannels("opencode")).toEqual({
      getCommand: "get_opencode_state",
      refreshCommand: "refresh_opencode",
      event: "opencode://state",
    });
  });
});

describe("onboardingStatus", () => {
  it("leaves out a tool that is not installed, except Codex", () => {
    expect(onboardingStatus(providerById("claude"), { connection: "cli_not_found" })).toBeNull();
    expect(onboardingStatus(providerById("codex"), { connection: "cli_not_found" })).toEqual({
      connected: false,
      detail: "Sign in with the codex CLI, then retry",
    });
  });

  it("says what it reads once connected, else the diagnostic or the sign-in step", () => {
    const gemini = providerById("gemini");
    expect(onboardingStatus(gemini, { connection: "connected" })).toEqual({
      connected: true,
      detail: "Reading the login kept by the gemini CLI",
    });
    expect(onboardingStatus(gemini, { connection: "error", diagnostic: "Quota API returned 403" })?.detail).toBe(
      "Quota API returned 403",
    );
    expect(onboardingStatus(gemini, { connection: "not_authenticated" })?.detail).toBe("Run `gemini` and sign in");
  });
});
