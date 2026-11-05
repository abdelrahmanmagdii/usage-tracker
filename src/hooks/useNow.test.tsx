// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, renderHook, screen } from "@testing-library/react";
import { NowProvider, WithNow, useNow } from "./useNow";

afterEach(() => {
  cleanup();
});

describe("NowProvider", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(Date.parse("2026-11-05T12:00:00Z"));
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("re-renders clock readers each second and leaves the rest alone", () => {
    let shellRenders = 0;
    function Shell() {
      shellRenders += 1;
      return <WithNow>{(now) => <output>{new Date(now).toISOString()}</output>}</WithNow>;
    }
    function Clock() {
      return <span data-testid="clock">{useNow()}</span>;
    }
    render(
      <NowProvider>
        <Shell />
        <Clock />
      </NowProvider>,
    );
    expect(screen.getByRole("status").textContent).toBe("2026-11-05T12:00:00.000Z");
    const rendersBefore = shellRenders;
    act(() => {
      vi.advanceTimersByTime(3_000);
    });
    expect(screen.getByRole("status").textContent).toBe("2026-11-05T12:00:03.000Z");
    expect(screen.getByTestId("clock").textContent).toBe(String(Date.parse("2026-11-05T12:00:03Z")));
    expect(shellRenders).toBe(rendersBefore);
  });

  it("holds a pinned time", () => {
    render(
      <NowProvider value={42}>
        <WithNow>{(now) => <output>{now}</output>}</WithNow>
      </NowProvider>,
    );
    act(() => {
      vi.advanceTimersByTime(5_000);
    });
    expect(screen.getByRole("status").textContent).toBe("42");
  });

  it("ticks on its own without a provider", () => {
    const { result } = renderHook(() => useNow());
    const start = result.current;
    act(() => {
      vi.advanceTimersByTime(2_000);
    });
    expect(result.current).toBe(start + 2_000);
  });
});
