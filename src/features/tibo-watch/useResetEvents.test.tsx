// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, renderHook } from "@testing-library/react";
import type { ResetEvent } from "../../types/codex";

const calls = vi.hoisted(() => [] as Array<(events: ResetEvent[]) => void>);
vi.mock("./provider", () => ({
  CombinedResetEventProvider: class {
    listEvents() {
      return new Promise<ResetEvent[]>((resolve) => calls.push(resolve));
    }
  },
}));
vi.mock("../../lib/demo", () => ({ demoMode: () => Promise.resolve(false) }));

import { readCachedEvents, useResetEvents } from "./useResetEvents";

const event = (id: string): ResetEvent => ({ id, announcedAt: "2026-08-13T11:30:00.000Z", source: "tibo" });

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(Date.parse("2026-08-13T12:00:00Z"));
  localStorage.clear();
  calls.length = 0;
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

it("never lets a slow older poll overwrite a newer one", async () => {
  const { result } = renderHook(() => useResetEvents());
  await act(async () => {});
  expect(calls).toHaveLength(1);

  // A focus nudge 30s later starts a second poll while the first is out.
  vi.setSystemTime(Date.now() + 31_000);
  act(() => {
    window.dispatchEvent(new Event("focus"));
  });
  expect(calls).toHaveLength(2);

  await act(async () => calls[1]([event("newer")]));
  await act(async () => calls[0]([event("older")]));
  expect(result.current.map((entry) => entry.id)).toEqual(["newer"]);
});

it("still applies an older poll when no newer one has landed", async () => {
  const { result } = renderHook(() => useResetEvents());
  await act(async () => {});
  vi.setSystemTime(Date.now() + 31_000);
  act(() => {
    window.dispatchEvent(new Event("focus"));
  });
  await act(async () => calls[0]([event("older")]));
  expect(result.current.map((entry) => entry.id)).toEqual(["older"]);
});

it("drops corrupt cached entries instead of crashing consumers on launch", () => {
  localStorage.setItem(
    "codex-meter.shared-reset-events.v1",
    JSON.stringify([null, 42, { id: "no-date" }, { ...event("ok"), sample: true }]),
  );
  expect(readCachedEvents()).toEqual([{ ...event("ok"), sample: true }]);
});

it("stops polling after unmount", async () => {
  const { unmount } = renderHook(() => useResetEvents());
  await act(async () => {});
  unmount();
  vi.setSystemTime(Date.now() + 10 * 60_000);
  act(() => {
    vi.advanceTimersByTime(10 * 60_000);
    window.dispatchEvent(new Event("focus"));
  });
  expect(calls).toHaveLength(1);
});
