import { describe, expect, it } from "vitest";
import tauriConfig from "../../src-tauri/tauri.conf.json";
import { DEFAULT_FEED_URL } from "../features/tibo-watch/provider";
import { bskyFeedUrl } from "../../tools/tibo-watch/lib.mjs";

// The webview's Content Security Policy only lets the page reach the hosts
// listed here. A feed moved to a new host would otherwise fail silently in
// release builds (dev runs without a policy so Vite's hot reload works).
const csp: Record<string, string> = tauriConfig.app.security.csp;
const connectSources = csp["connect-src"].split(/\s+/);

describe("content security policy", () => {
  it("allows every host the reset feeds fetch from", () => {
    for (const url of [DEFAULT_FEED_URL, bskyFeedUrl("someone.example")]) {
      expect(connectSources).toContain(new URL(url).origin);
    }
  });

  it("keeps the Tauri IPC channel reachable on every platform", () => {
    expect(connectSources).toContain("ipc:");
    expect(connectSources).toContain("http://ipc.localhost");
  });

  it("allows no inline or remote code", () => {
    expect(csp["script-src"]).toBe("'self'");
    expect(csp["default-src"]).toBe("'self'");
    expect(csp["object-src"]).toBe("'none'");
  });
});
