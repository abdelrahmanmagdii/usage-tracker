// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, renderHook } from "@testing-library/react";
import type { CodexBackendState } from "../types/codex";
import { isOlderSnapshot, useProviderMeter } from "./useProviderMeter";
import { useCodexMeter } from "./useCodexMeter";
import { useGeminiMeter } from "./useClaudeMeter";

const ipc = vi.hoisted(() => ({
  invoke: vi.fn(),
  handlers: new Map<string, (event: { payload: unknown }) => void>(),
  unlisten: vi.fn(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: ipc.invoke }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (name: string, handler: (event: { payload: unknown }) => void) => {
    ipc.handlers.set(name, handler);
    return ipc.unlisten;
  }),
}));
const history = vi.hoisted(() => ({ observeBuckets: vi.fn() }));
vi.mock("../lib/history", () => history);

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

function reading(updatedAt: number | undefined, usedPercent: number, connection: CodexBackendState["connection"] = "connected"): CodexBackendState {
  return {
    connection,
    updatedAt,
    rateLimits: {
      rateLimitsByLimitId: {
        session: { limitId: "session", primary: { usedPercent, windowDurationMins: 300 } },
      },
    },
  };
}

const preview = () => reading(1, 1);
const options = {
  provider: "claude",
  getCommand: "get_claude_state",
  refreshCommand: "refresh_claude",
  event: "claude://state",
  preview,
};

/** Lets the mocked `listen` register its handler and pending promises settle. */
async function flush() {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
}

function emit(event: string, payload: CodexBackendState) {
  act(() => ipc.handlers.get(event)?.({ payload }));
}

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  ipc.handlers.clear();
  Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
  window.history.replaceState({}, "", "/");
});

describe("isOlderSnapshot", () => {
  it("only rejects a timestamped reading older than a timestamped one on screen", () => {
    expect(isOlderSnapshot(reading(100, 0), 200)).toBe(true);
    expect(isOlderSnapshot(reading(200, 0), 200)).toBe(false);
    expect(isOlderSnapshot(reading(300, 0), 200)).toBe(false);
    expect(isOlderSnapshot({ connection: "cli_not_found" }, 200)).toBe(false);
    expect(isOlderSnapshot(reading(100, 0), undefined)).toBe(false);
    expect(isOlderSnapshot(reading(100, 0), null)).toBe(false);
  });
});

describe("useProviderMeter at startup", () => {
  beforeEach(() => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", { value: {}, configurable: true });
  });

  it("keeps a newer event when the initial read resolves late", async () => {
    const initial = deferred<CodexBackendState>();
    ipc.invoke.mockImplementation((command: string) =>
      command === "get_claude_state" ? initial.promise : Promise.reject(new Error(command)),
    );
    const { result } = renderHook(() => useProviderMeter(options));
    await flush();

    emit("claude://state", reading(200, 40));
    expect(result.current.state.updatedAt).toBe(200);

    await act(async () => initial.resolve(reading(100, 10)));
    expect(result.current.state.updatedAt).toBe(200);
    expect(result.current.buckets[0]?.usedPercent).toBe(40);
    // The stale read never reaches the history either.
    expect(history.observeBuckets).toHaveBeenCalledTimes(1);
  });

  it("still applies an equal or missing timestamp so connection changes go through", async () => {
    ipc.invoke.mockResolvedValue(reading(200, 40));
    const { result } = renderHook(() => useProviderMeter(options));
    await flush();
    expect(result.current.state.connection).toBe("connected");

    emit("claude://state", reading(200, 40, "not_authenticated"));
    expect(result.current.state.connection).toBe("not_authenticated");

    emit("claude://state", { connection: "cli_not_found" });
    expect(result.current.state.connection).toBe("cli_not_found");

    // After a timestamp-less state, the next reading applies whatever its age.
    emit("claude://state", reading(150, 20));
    expect(result.current.state.updatedAt).toBe(150);
  });

  it("applies the initial read when it is the newest reading", async () => {
    ipc.invoke.mockResolvedValue(reading(300, 55));
    const { result } = renderHook(() => useProviderMeter(options));
    await flush();
    expect(result.current.buckets[0]?.usedPercent).toBe(55);
    emit("claude://state", reading(250, 30));
    expect(result.current.buckets[0]?.usedPercent).toBe(55);
  });

  it("stops listening once unmounted", async () => {
    ipc.invoke.mockResolvedValue(reading(100, 10));
    const { unmount } = renderHook(() => useProviderMeter(options));
    await flush();
    unmount();
    await flush();
    expect(ipc.unlisten).toHaveBeenCalled();
  });
});

describe("useProviderMeter outside Tauri", () => {
  it("hides a tool that has no backend to read", async () => {
    const { result } = renderHook(() => useGeminiMeter());
    await flush();
    expect(result.current.state).toEqual({ connection: "cli_not_found" });
    expect(ipc.invoke).not.toHaveBeenCalled();
  });

  it("shows sample data with ?preview in dev", async () => {
    window.history.replaceState({}, "", "/?preview");
    const { result } = renderHook(() => useGeminiMeter());
    await flush();
    expect(result.current.state.connection).toBe("connected");
    expect(result.current.buckets.map((bucket) => bucket.limitId)).toEqual(["gemini-pro", "gemini-flash"]);
  });
});

describe("useCodexMeter", () => {
  it("explains itself outside Tauri instead of hiding", async () => {
    const { result } = renderHook(() => useCodexMeter());
    await flush();
    expect(result.current.state.connection).toBe("error");
    expect(result.current.state.diagnostic).toMatch(/through Tauri/);
    expect(result.current.resetCredits.availableCount).toBe(0);
    expect(result.current.usage).toBeNull();
  });

  it("adds reset credits and usage to the shared meter", async () => {
    window.history.replaceState({}, "", "/?preview");
    const { result } = renderHook(() => useCodexMeter());
    await flush();
    expect(result.current.buckets).toHaveLength(2);
    expect(result.current.resetCredits.availableCount).toBe(1);
    expect(result.current.usage?.lifetimeTokens).toBe(2_470_000);
    expect(history.observeBuckets).toHaveBeenCalledWith("codex", expect.any(Array));
  });

  it("leaves the history alone for the widget window", async () => {
    window.history.replaceState({}, "", "/?preview");
    renderHook(() => useCodexMeter({ observeHistory: false }));
    await flush();
    expect(history.observeBuckets).not.toHaveBeenCalled();
  });

  it("talks to the Codex commands and event", async () => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", { value: {}, configurable: true });
    ipc.invoke.mockResolvedValue(reading(100, 10));
    const { result } = renderHook(() => useCodexMeter());
    await flush();
    expect(ipc.invoke).toHaveBeenCalledWith("get_codex_state");
    expect(ipc.handlers.has("codex://state")).toBe(true);
    await act(() => result.current.refresh());
    expect(ipc.invoke).toHaveBeenCalledWith("refresh_codex");
  });
});
