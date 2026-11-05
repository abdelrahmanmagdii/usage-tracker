// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import App from "./App";
import type { AppPrefs } from "./lib/providers";
import type { CodexBackendState } from "./types/codex";

const ipc = vi.hoisted(() => ({
  states: {} as Record<string, unknown>,
  invoke: vi.fn(),
  handlers: new Map<string, Array<(event: { payload: unknown }) => void>>(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: ipc.invoke }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (name: string, handler: (event: { payload: unknown }) => void) => {
    ipc.handlers.set(name, [...(ipc.handlers.get(name) ?? []), handler]);
    return () => {
      ipc.handlers.set(name, (ipc.handlers.get(name) ?? []).filter((entry) => entry !== handler));
    };
  }),
}));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ startDragging: vi.fn() }) }));
vi.mock("@tauri-apps/plugin-notification", () => ({
  isPermissionGranted: async () => false,
  requestPermission: async () => "denied",
  sendNotification: vi.fn(),
}));
// The reset feed polls the network; the popover only needs its list.
vi.mock("./features/tibo-watch/useResetEvents", () => ({ useResetEvents: () => [] }));
// The popover calls useProviderMeters once per render, so counting the calls
// counts how often the whole shell re-renders.
const renders = vi.hoisted(() => ({ popover: 0 }));
vi.mock("./hooks/useProviderMeters", async (importOriginal) => {
  const original = await importOriginal<typeof import("./hooks/useProviderMeters")>();
  return {
    useProviderMeters: () => {
      renders.popover += 1;
      return original.useProviderMeters();
    },
  };
});

const now = Date.parse("2026-11-05T12:00:00Z");
const prefs: AppPrefs = {
  usageAlerts: true,
  resetAlerts: true,
  denseLayout: false,
  combinedTray: true,
  onboardingComplete: true,
  providers: {},
};

function reading(usedPercent: number, resetInSecs = 3_600): CodexBackendState {
  return {
    connection: "connected",
    updatedAt: now / 1000,
    rateLimits: {
      rateLimitsByLimitId: {
        main: { limitId: "main", primary: { usedPercent, windowDurationMins: 300, resetsAt: now / 1000 + resetInSecs } },
      },
    },
  };
}

const absent: CodexBackendState = { connection: "cli_not_found" };
const PROVIDERS = ["codex", "claude", "cursor", "opencode", "devin", "antigravity", "gemini"];

function emit(event: string, payload: unknown) {
  act(() => {
    for (const handler of ipc.handlers.get(event) ?? []) handler({ payload });
  });
}

async function renderApp() {
  const view = render(<App />);
  // Let every mocked IPC promise, and the state it sets, settle.
  await act(async () => {
    for (let tick = 0; tick < 10; tick += 1) await Promise.resolve();
    // React schedules work set from a resolved promise on a 0ms timer.
    await vi.advanceTimersByTimeAsync(0);
  });
  return view;
}

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "setTimeout", "clearTimeout", "Date"] });
  vi.setSystemTime(now);
  Object.defineProperty(window, "__TAURI_INTERNALS__", { value: {}, configurable: true });
  renders.popover = 0;
  ipc.states = {
    get_app_prefs: prefs,
    get_tray_windows: Object.fromEntries(PROVIDERS.map((id) => [id, []])),
    get_autostart: false,
    is_demo_mode: false,
    get_codex_state: reading(30, 2 * 3_600),
    get_claude_state: reading(60),
    get_cursor_state: absent,
    get_opencode_state: absent,
    get_devin_state: absent,
    get_antigravity_state: reading(10),
    get_gemini_state: reading(20),
  };
  ipc.invoke.mockImplementation(async (command: string) => {
    if (command in ipc.states) return ipc.states[command];
    const refreshed = /^refresh_(\w+)$/.exec(command)?.[1];
    if (refreshed) return ipc.states[`get_${refreshed}_state`];
    return undefined;
  });
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.clearAllMocks();
  ipc.handlers.clear();
  localStorage.clear();
  Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
});

describe("App", () => {
  it("renders a section per signed-in tool from the catalog", async () => {
    const { container } = await renderApp();
    const sections = [...container.querySelectorAll("section.provider-block")];
    expect(sections.map((section) => section.getAttribute("data-provider"))).toEqual(["claude", "antigravity", "gemini"]);
    expect(screen.getByRole("region", { name: "Gemini CLI" })).toBeTruthy();
    const glyph = (id: string) => container.querySelector(`[data-provider="${id}"] .provider-label svg`)?.getAttribute("class");
    expect(glyph("gemini")).toContain("lucide-gem");
    expect(glyph("antigravity")).toContain("lucide-sparkle");
    // The header shows the tool closest to running out: Claude, 40% left.
    expect(container.querySelector(".available-value strong")?.textContent).toBe("40");
  });

  it("ticks the countdowns without re-rendering the rest of the popover", async () => {
    const { container } = await renderApp();
    const header = () => container.querySelector(".status-copy strong")?.textContent;
    expect(header()).toBe("Renews in 1h 0m");
    const before = renders.popover;
    act(() => {
      vi.advanceTimersByTime(61_000);
    });
    expect(header()).toBe("Renews in 58m 59s");
    expect(renders.popover).toBe(before);
  });

  it("refreshes only the tools that are shown", async () => {
    ipc.states.get_app_prefs = { ...prefs, providers: { claude: { visible: false } } };
    await renderApp();
    ipc.invoke.mockClear();
    fireEvent.click(screen.getByRole("button", { name: "Refresh usage data" }));
    const commands = ipc.invoke.mock.calls.map(([command]) => command);
    expect(commands).toContain("refresh_codex");
    expect(commands).toContain("refresh_gemini");
    expect(commands).not.toContain("refresh_claude");
  });

  it("keeps a prefs change that lands before the initial prefs read", async () => {
    let resolvePrefs!: (value: AppPrefs) => void;
    ipc.invoke.mockImplementation(async (command: string) => {
      if (command === "get_app_prefs") return new Promise((done) => { resolvePrefs = done; });
      return ipc.states[command];
    });
    const { container } = await renderApp();
    emit("usagebar://prefs", { ...prefs, providers: { gemini: { visible: false } } });
    await act(async () => resolvePrefs(prefs));
    expect(container.querySelector('[data-provider="gemini"]')).toBeNull();
  });

  it("says Connecting while a shown tool is still starting, even with Codex hidden", async () => {
    ipc.states.get_app_prefs = { ...prefs, providers: { codex: { visible: false } } };
    ipc.states.get_claude_state = { connection: "starting" };
    ipc.states.get_antigravity_state = absent;
    ipc.states.get_gemini_state = absent;
    await renderApp();
    expect(screen.getByText("Connecting…")).toBeTruthy();
    emit("claude://state", absent);
    expect(screen.getByText("Offline")).toBeTruthy();
  });

  it("lists the setup guide entries from the catalog", async () => {
    ipc.states.get_app_prefs = { ...prefs, onboardingComplete: false };
    await renderApp();
    const guide = screen.getByRole("dialog", { name: "UsageBar setup" });
    fireEvent.click(within(guide).getByRole("button", { name: /Next/ }));
    expect(within(guide).getByText("Codex")).toBeTruthy();
    expect(within(guide).getByText("Claude Code")).toBeTruthy();
    expect(within(guide).getByText("Gemini CLI")).toBeTruthy();
    expect(within(guide).queryByText("Cursor")).toBeNull();
  });

  describe("Escape", () => {
    it("closes settings first, then hides the window", async () => {
      await renderApp();
      fireEvent.click(screen.getByRole("button", { name: /Settings/ }));
      expect(screen.getByRole("dialog")).toBeTruthy();
      ipc.invoke.mockClear();
      fireEvent.keyDown(window, { key: "Escape" });
      expect(screen.queryByRole("dialog")).toBeNull();
      expect(ipc.invoke).not.toHaveBeenCalledWith("hide_window");
      fireEvent.keyDown(window, { key: "Escape" });
      expect(ipc.invoke).toHaveBeenCalledWith("hide_window");
    });

    it("closes one layer per press, not every layer while the key is held", async () => {
      await renderApp();
      fireEvent.click(screen.getByRole("button", { name: /Settings/ }));
      ipc.invoke.mockClear();
      fireEvent.keyDown(window, { key: "Escape" });
      fireEvent.keyDown(window, { key: "Escape", repeat: true });
      expect(screen.queryByRole("dialog")).toBeNull();
      expect(ipc.invoke).not.toHaveBeenCalledWith("hide_window");
    });

    it("leaves a press a control already handled", async () => {
      await renderApp();
      ipc.invoke.mockClear();
      const event = new KeyboardEvent("keydown", { key: "Escape", cancelable: true });
      event.preventDefault();
      act(() => {
        window.dispatchEvent(event);
      });
      expect(ipc.invoke).not.toHaveBeenCalledWith("hide_window");
    });

    it("finishes onboarding when it dismisses the guide", async () => {
      ipc.states.get_app_prefs = { ...prefs, onboardingComplete: false };
      await renderApp();
      expect(screen.getByRole("dialog", { name: "UsageBar setup" })).toBeTruthy();
      fireEvent.keyDown(window, { key: "Escape" });
      expect(screen.queryByRole("dialog")).toBeNull();
      expect(ipc.invoke).toHaveBeenCalledWith("complete_onboarding");
      expect(ipc.invoke).not.toHaveBeenCalledWith("hide_window");
    });
  });
});
