// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { extractRateLimitBuckets } from "../lib/rateLimits";
import { CodexWidget } from "./CodexWidget";
import type { CodexBackendState, RateLimitBucket } from "../types/codex";
import { DEFAULT_PREFS } from "../lib/providers";

const meter = vi.hoisted(() => ({
  state: { connection: "connected" } as CodexBackendState,
  buckets: [] as RateLimitBucket[],
  refresh: vi.fn(),
  refreshing: false,
}));
vi.mock("../hooks/useCodexMeter", () => ({ useCodexMeter: () => meter }));

const otherMeters = vi.hoisted(() => ({
  antigravity: {
    state: { connection: "cli_not_found" } as CodexBackendState,
    buckets: [] as RateLimitBucket[],
    refresh: vi.fn(),
    refreshing: false,
  },
  claude: {
    state: { connection: "cli_not_found" } as CodexBackendState,
    buckets: [] as RateLimitBucket[],
    refresh: vi.fn(),
    refreshing: false,
  },
  cursor: {
    state: { connection: "cli_not_found" } as CodexBackendState,
    buckets: [] as RateLimitBucket[],
    refresh: vi.fn(),
    refreshing: false,
  },
  opencode: {
    state: { connection: "cli_not_found" } as CodexBackendState,
    buckets: [] as RateLimitBucket[],
    refresh: vi.fn(),
    refreshing: false,
  },
  devin: {
    state: { connection: "cli_not_found" } as CodexBackendState,
    buckets: [] as RateLimitBucket[],
    refresh: vi.fn(),
    refreshing: false,
  },
}));

vi.mock("../hooks/useClaudeMeter", () => ({
  useAntigravityMeter: () => otherMeters.antigravity,
  useClaudeMeter: () => otherMeters.claude,
  useCursorMeter: () => otherMeters.cursor,
  useOpenCodeMeter: () => otherMeters.opencode,
  useDevinMeter: () => otherMeters.devin,
}));

const ipc = vi.hoisted(() => vi.fn().mockResolvedValue(undefined));
vi.mock("@tauri-apps/api/core", () => ({ invoke: ipc }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn().mockResolvedValue(() => undefined) }));
const now = Date.parse("2026-10-15T17:00:00Z");

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(now);
  meter.state = { connection: "connected", updatedAt: now / 1000 };
  meter.buckets = extractRateLimitBuckets({
    rateLimitsByLimitId: {
      model: { primary: { usedPercent: 99, windowDurationMins: 300 } },
      codex: {
        primary: { usedPercent: 18, windowDurationMins: 300, resetsAt: now / 1000 + 7200 },
        secondary: { usedPercent: 36, windowDurationMins: 10080, resetsAt: now / 1000 + 3 * 86400 },
      },
    },
  });
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.clearAllMocks();
  Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
  for (const other of Object.values(otherMeters)) {
    other.buckets = [];
    other.state = { connection: "cli_not_found" };
  }
});

function renderExpanded() {
  render(<CodexWidget />);
  fireEvent.mouseEnter(screen.getByRole("main"));
  act(() => {
    vi.advanceTimersByTime(250);
  });
}

it("shows subscription quota remaining and separate reset countdowns", () => {
  renderExpanded();
  expect(screen.getByText("82%")).toBeTruthy();
  expect(screen.getByText("64%")).toBeTruthy();
  expect(screen.getByText("Resets in 2h 0m")).toBeTruthy();
  expect(screen.getByText("Resets in 3d 0h")).toBeTruthy();
  expect(screen.queryByText("1%")).toBeNull();
});

it("does not present missing quota as unused quota", () => {
  meter.state = { connection: "not_authenticated" };
  meter.buckets = [];
  renderExpanded();
  expect(screen.getByText("Sign in to Codex")).toBeTruthy();
  expect(screen.getAllByText("Quota unavailable")).toHaveLength(2);
  expect(screen.queryByText("100%")).toBeNull();
  expect(screen.queryByRole("meter")).toBeNull();
});

it("marks cached percentages as stale when the connection drops", () => {
  meter.state = { connection: "disconnected", updatedAt: now / 1000 - 1200 };
  renderExpanded();
  expect(screen.getByText("~82%")).toBeTruthy();
  expect(screen.getByText("Reconnecting…")).toBeTruthy();
});

it("refreshes from its own button", () => {
  renderExpanded();
  fireEvent.click(screen.getByRole("button", { name: "Refresh Codex quota" }));
  expect(meter.refresh).toHaveBeenCalledOnce();
});

it("keeps 5-hour percentage and countdown visible while collapsed without chunkiness", () => {
  render(<CodexWidget />);
  expect(screen.getByText("82%")).toBeTruthy();
  expect(screen.getByText("2h 0m")).toBeTruthy();
  // Weekly limit is omitted from collapsed view to save space
  expect(screen.queryByText("64%")).toBeNull();
  expect(screen.queryByRole("button", { name: "Refresh Codex quota" })).toBeNull();
});

it("expands on hover, stays open on re-entry, and collapses after leaving", () => {
  render(<CodexWidget />);
  const widget = screen.getByRole("main");
  fireEvent.mouseEnter(widget);
  expect(screen.queryByText("Resets in 2h 0m")).toBeNull();
  act(() => {
    vi.advanceTimersByTime(250);
  });
  expect(screen.getByText("Resets in 2h 0m")).toBeTruthy();
  fireEvent.mouseLeave(widget);
  act(() => {
    vi.advanceTimersByTime(100);
  });
  fireEvent.mouseEnter(widget);
  act(() => {
    vi.advanceTimersByTime(300);
  });
  expect(screen.getByText("Resets in 2h 0m")).toBeTruthy();
  fireEvent.mouseLeave(widget);
  act(() => {
    vi.advanceTimersByTime(250);
  });
  expect(screen.getByText("2h 0m")).toBeTruthy();
  expect(screen.queryByText("Resets in 2h 0m")).toBeNull();
});

it("offers separate minimize and quit actions on hover", () => {
  Object.defineProperty(window, "__TAURI_INTERNALS__", { value: {}, configurable: true });
  renderExpanded();
  fireEvent.click(screen.getByRole("button", { name: "Minimize Codex meter" }));
  expect(ipc).toHaveBeenCalledWith("hide_codex_widget");
  fireEvent.mouseEnter(screen.getByRole("main"));
  act(() => {
    vi.advanceTimersByTime(250);
  });
  fireEvent.click(screen.getByRole("button", { name: "Quit UsageBar" }));
  expect(ipc).toHaveBeenCalledWith("quit_app");
});

it("starts native dragging from the grip with the left mouse button", () => {
  Object.defineProperty(window, "__TAURI_INTERNALS__", { value: {}, configurable: true });
  renderExpanded();
  fireEvent.mouseDown(screen.getByLabelText("Drag to move meter"), { button: 2 });
  expect(ipc).not.toHaveBeenCalledWith("drag_codex_widget");
  fireEvent.mouseDown(screen.getByLabelText("Drag to move meter"), { button: 0 });
  expect(ipc).toHaveBeenCalledWith("drag_codex_widget");
});

it("offers a position reset in the expanded controls", () => {
  Object.defineProperty(window, "__TAURI_INTERNALS__", { value: {}, configurable: true });
  renderExpanded();
  fireEvent.click(screen.getByRole("button", { name: "Restore meter to bottom-right" }));
  expect(ipc).toHaveBeenCalledWith("reset_widget_position");
});

it("ignores a brief hover without expanding", () => {
  render(<CodexWidget />);
  fireEvent.mouseEnter(screen.getByRole("main"));
  act(() => {
    vi.advanceTimersByTime(200);
  });
  fireEvent.mouseLeave(screen.getByRole("main"));
  act(() => {
    vi.advanceTimersByTime(1000);
  });
  expect(screen.getByText("1h 59m")).toBeTruthy();
  expect(screen.queryByText("Resets in 2h 0m")).toBeNull();
});

it("shows accessible progress meters for 5-hour quota while collapsed and both in expanded view", () => {
  const { unmount } = render(<CodexWidget />);
  expect(screen.getByText("5h")).toBeTruthy();
  expect(screen.getByRole("meter", { name: "5-hour quota remaining" }).getAttribute("aria-valuenow")).toBe("82");
  unmount();

  renderExpanded();
  expect(screen.getByRole("meter", { name: "5h quota remaining" }).getAttribute("aria-valuenow")).toBe("82");
  expect(screen.getByRole("meter", { name: "Weekly quota remaining" }).getAttribute("aria-valuenow")).toBe("64");
});

it("turns yellow at 35% remaining and red at 20% remaining", () => {
  meter.buckets = extractRateLimitBuckets({
    rateLimitsByLimitId: {
      codex: {
        primary: { usedPercent: 65, windowDurationMins: 300, resetsAt: now / 1000 + 7200 },
        secondary: { usedPercent: 80, windowDurationMins: 10080, resetsAt: now / 1000 + 86400 },
      },
    },
  });
  render(<CodexWidget />);
  expect(screen.getByText("35%").closest(".widget-compact-chip")?.classList.contains("quota-warning")).toBe(true);
});

it("supports multiple LLM models side-by-side with 5-hour and Argon 4 weekly usage", () => {
  otherMeters.antigravity.state = { connection: "connected", updatedAt: now / 1000 };
  otherMeters.antigravity.buckets = extractRateLimitBuckets({
    rateLimitsByLimitId: {
      "gemini-5h": { primary: { usedPercent: 2, windowDurationMins: 300, resetsAt: now / 1000 + 3600 } },
      "argon-weekly": {
        limitName: "Gemini 4 Argon",
        secondary: { usedPercent: 6, windowDurationMins: 10080, resetsAt: now / 1000 + 6 * 86400 },
      },
      "3p-5h": { primary: { usedPercent: 40, windowDurationMins: 300, resetsAt: now / 1000 + 5400 } },
    },
  });
  render(<CodexWidget />);
  expect(screen.getByText("Codex")).toBeTruthy();
  expect(screen.getByText("Gemini")).toBeTruthy();
  expect(screen.getByText("Argon 4")).toBeTruthy();
  expect(screen.getByText("Claude")).toBeTruthy();
  expect(screen.getByText("82%")).toBeTruthy();
  expect(screen.getByText("98%")).toBeTruthy();
  expect(screen.getByText("94%")).toBeTruthy();
  expect(screen.getByText("60%")).toBeTruthy();
  // Weekly limits remain hidden in collapsed mode when a 5h limit is present
  expect(screen.queryByText("64%")).toBeNull();
});

function connectAntigravityGroups() {
  otherMeters.antigravity.state = { connection: "connected", updatedAt: now / 1000 };
  otherMeters.antigravity.buckets = extractRateLimitBuckets({
    rateLimitsByLimitId: {
      "gemini-5h": { primary: { usedPercent: 2, windowDurationMins: 300 } },
      "argon-weekly": { limitName: "Gemini 4 Argon", secondary: { usedPercent: 6, windowDurationMins: 10080 } },
      "3p-5h": { primary: { usedPercent: 40, windowDurationMins: 300 } },
    },
  });
}

it("refreshes each meter once, even when one meter fills several cards", () => {
  connectAntigravityGroups();
  renderExpanded();
  fireEvent.click(screen.getByRole("button", { name: "Refresh quota" }));
  expect(otherMeters.antigravity.refresh).toHaveBeenCalledOnce();
  expect(meter.refresh).toHaveBeenCalledOnce();
});

it("shows a weekly-only plan's window once, labelled as weekly", () => {
  meter.buckets = extractRateLimitBuckets({
    rateLimitsByLimitId: {
      codex: { secondary: { usedPercent: 36, windowDurationMins: 10080, resetsAt: now / 1000 + 3 * 86400 } },
    },
  });
  const { unmount } = render(<CodexWidget />);
  expect(screen.getByText("7d")).toBeTruthy();
  expect(screen.queryByText("5h")).toBeNull();
  expect(screen.getByRole("meter", { name: "Weekly quota remaining" })).toBeTruthy();
  unmount();

  renderExpanded();
  expect(screen.getAllByText("64%")).toHaveLength(1);
});

it("labels other window lengths by their own duration, not as 5-hour", () => {
  meter.state = { connection: "cli_not_found" };
  meter.buckets = [];
  otherMeters.cursor.state = { connection: "connected", updatedAt: now / 1000 };
  otherMeters.cursor.buckets = extractRateLimitBuckets({
    rateLimitsByLimitId: {
      plan: { primary: { usedPercent: 41, windowDurationMins: 43_200, resetsAt: now / 1000 + 9 * 86400 } },
    },
  });
  render(<CodexWidget />);
  expect(screen.getByText("30d")).toBeTruthy();
  expect(screen.queryByText("5h")).toBeNull();
  expect(screen.getByRole("meter", { name: "30-day quota remaining" })).toBeTruthy();
});

it("says a stale Cursor reading is stale rather than live", () => {
  meter.state = { connection: "cli_not_found" };
  meter.buckets = [];
  otherMeters.cursor.state = { connection: "connected", updatedAt: now / 1000 - 3600 };
  otherMeters.cursor.buckets = extractRateLimitBuckets({
    rateLimitsByLimitId: { plan: { primary: { usedPercent: 41, windowDurationMins: 43_200 } } },
  });
  renderExpanded();
  expect(screen.getByText("Stale reading")).toBeTruthy();
  expect(screen.queryByText("Live quota")).toBeNull();
});

function withTauriPrefs(providers: Record<string, { visible: boolean }>) {
  Object.defineProperty(window, "__TAURI_INTERNALS__", {
    value: { transformCallback: () => 0 },
    configurable: true,
  });
  ipc.mockImplementation(async (command: string) =>
    command === "get_app_prefs" ? { ...DEFAULT_PREFS, providers } : undefined,
  );
}

it("waits on a tool the user keeps visible, not on a hidden Codex", async () => {
  withTauriPrefs({ codex: { visible: false } });
  meter.state = { connection: "cli_not_found" };
  meter.buckets = [];
  otherMeters.claude.state = { connection: "starting" };
  render(<CodexWidget />);
  await act(async () => {});
  expect(screen.getByText("Claude")).toBeTruthy();
  expect(screen.queryByText("Codex")).toBeNull();
  ipc.mockReset();
  ipc.mockResolvedValue(undefined);
});

it("grows the open panel when another tool connects while it is open", async () => {
  Object.defineProperty(window, "__TAURI_INTERNALS__", { value: {}, configurable: true });
  renderExpanded();
  const expandCalls = () =>
    ipc.mock.calls.filter(
      ([command, args]) => command === "set_widget_expanded" && (args as { expanded: boolean }).expanded,
    );
  const before = expandCalls();
  expect(before).toHaveLength(1);
  const firstHeight = (before[0][1] as { height: number }).height;

  otherMeters.claude.state = { connection: "connected", updatedAt: now / 1000 };
  otherMeters.claude.buckets = extractRateLimitBuckets({
    rateLimitsByLimitId: {
      session: { primary: { usedPercent: 8, windowDurationMins: 300 } },
      weekly: { secondary: { usedPercent: 57, windowDurationMins: 10080 } },
    },
  });
  act(() => {
    vi.advanceTimersByTime(1000);
  });
  const after = expandCalls();
  expect(after.length).toBeGreaterThan(1);
  expect((after[after.length - 1][1] as { height: number }).height).toBeGreaterThan(firstHeight);
});
