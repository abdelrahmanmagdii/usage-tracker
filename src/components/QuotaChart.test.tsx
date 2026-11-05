// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { QuotaSection } from "./QuotaSection";
import type { QuotaSnapshot, RateLimitBucket } from "../types/codex";

afterEach(() => {
  cleanup();
  localStorage.clear();
});

const now = Date.parse("2026-10-03T12:00:00Z");
const HOUR = 3_600_000;
const resetsAt = (now + 3 * HOUR) / 1000;
const bucket: RateLimitBucket = {
  id: "codex:primary:300",
  limitId: "codex",
  windowKind: "primary",
  usedPercent: 60,
  remainingPercent: 40,
  windowDurationMins: 300,
  resetsAt,
  reached: false,
};
const sample = (minsAgo: number, used: number, reset = resetsAt): QuotaSnapshot => ({
  timestamp: new Date(now - minsAgo * 60_000).toISOString(),
  limitId: "codex:codex:primary:300",
  usedPercent: used,
  remainingPercent: 100 - used,
  windowDurationMins: 300,
  resetsAt: reset,
});
const seed = (samples: QuotaSnapshot[]) =>
  localStorage.setItem("codex-meter.quota-history.v1", JSON.stringify(samples));

const chart = () => screen.getByRole("group", { name: /Usage over this window/ });

describe("QuotaChart in a quota tile", () => {
  it("draws the current window with even pace, a projection, and matching pace text", () => {
    // A previous window that nearly ran out, then 40 points in the last hour.
    seed([sample(400, 20, resetsAt - 18_000), sample(320, 90, resetsAt - 18_000), sample(110, 5), sample(60, 20), sample(30, 40), sample(1, 60)]);
    const { container } = render(<QuotaSection provider="codex" bucket={bucket} now={now} />);
    // Step line: a horizontal hold then a vertical change per reading. The
    // previous window's readings are not on it.
    const line = container.querySelector(".quota-chart-line")!.getAttribute("d")!;
    expect(line.match(/H/g)).toHaveLength(4);
    expect(container.querySelector(".quota-chart-even")).toBeTruthy();
    expect(container.querySelector(".quota-chart-projection")).toBeTruthy();
    expect(screen.getByText(/At this pace, runs out in/)).toBeTruthy();
    expect(chart().getAttribute("aria-label")).toContain("40% of this window has passed and 40% is left");
    expect(chart().getAttribute("aria-label")).toContain("before the reset");
    expect(container.querySelector(".quota-chart-axis")?.textContent).toContain("even pace");
  });

  it("shows the guide and the current point before anything is recorded", () => {
    const { container } = render(<QuotaSection provider="codex" bucket={bucket} now={now} />);
    expect(container.querySelector(".quota-chart-even")).toBeTruthy();
    expect(container.querySelector(".quota-chart-dot")).toBeTruthy();
    expect(container.querySelector(".quota-chart-line")).toBeNull();
    expect(container.querySelector(".quota-chart-projection")).toBeNull();
  });

  it("skips the chart when there is no reset time to anchor it", () => {
    const { container } = render(<QuotaSection provider="codex" bucket={{ ...bucket, resetsAt: undefined }} now={now} />);
    expect(container.querySelector(".quota-chart")).toBeNull();
  });

  it("steps through readings with the keyboard and announces them", () => {
    seed([sample(110, 5), sample(60, 20), sample(30, 40)]);
    render(<QuotaSection provider="codex" bucket={bucket} now={now} />);
    const group = chart();
    act(() => group.focus());
    const live = group.querySelector("[aria-live]")!;
    expect(live.textContent).toMatch(/^40% left, now/);
    fireEvent.keyDown(group, { key: "ArrowLeft" });
    expect(live.textContent).toMatch(/^60% left/);
    fireEvent.keyDown(group, { key: "Home" });
    expect(live.textContent).toMatch(/^95% left/);
    fireEvent.keyDown(group, { key: "ArrowRight" });
    expect(live.textContent).toMatch(/^80% left/);
    // The last stop is the projected run-out, matching the pace text.
    fireEvent.keyDown(group, { key: "End" });
    expect(live.textContent).toMatch(/^Runs out, projected/);
    fireEvent.keyDown(group, { key: "Escape" });
    expect(group.querySelector(".quota-chart-tip")).toBeNull();
  });

  it("shows a readout under the pointer and hides it on leave", () => {
    seed([sample(110, 5), sample(60, 20), sample(30, 40)]);
    const { container } = render(<QuotaSection provider="codex" bucket={bucket} now={now} />);
    const plot = container.querySelector(".quota-chart-plot")!;
    // jsdom has no layout: the plot falls back to 320px wide at x = 0, with
    // 6px of padding. The window starts 2h ago and lasts 5h.
    fireEvent.pointerMove(plot, { clientX: 6 });
    expect(container.querySelector(".quota-chart-tip")?.textContent).toContain("Not recorded");
    // 20% across is one hour in (the reading 60 minutes ago).
    fireEvent.pointerMove(plot, { clientX: 6 + 0.2 * (320 - 12) });
    expect(container.querySelector(".quota-chart-tip")?.textContent).toContain("80% left");
    // 55 points in under two hours runs out before the reset: the pointer
    // snaps to the run-out point, and past it the projection stays at zero.
    const runOutX = Number(container.querySelector(".quota-chart-runout")!.getAttribute("cx"));
    fireEvent.pointerMove(plot, { clientX: runOutX + 5 });
    expect(container.querySelector(".quota-chart-tip")?.textContent).toMatch(/^Runs outprojected/);
    fireEvent.pointerMove(plot, { clientX: 6 + 0.9 * (320 - 12) });
    expect(container.querySelector(".quota-chart-tip")?.textContent).toMatch(/^0% leftprojected/);
    fireEvent.pointerLeave(plot);
    expect(container.querySelector(".quota-chart-tip")).toBeNull();
  });

  it("only knows even pace ahead of now when no run-out is projected", () => {
    seed([sample(110, 30), sample(30, 40)]);
    const { container } = render(<QuotaSection provider="codex" bucket={{ ...bucket, usedPercent: 40, remainingPercent: 60 }} now={now} />);
    const plot = container.querySelector(".quota-chart-plot")!;
    fireEvent.pointerMove(plot, { clientX: 6 + 0.8 * (320 - 12) });
    expect(container.querySelector(".quota-chart-tip")?.textContent).toMatch(/^Not yeteven pace 20%/);
  });

  it("fits a slim chart into dense rows", () => {
    seed([sample(110, 5), sample(30, 40)]);
    const { container } = render(<QuotaSection provider="codex" bucket={bucket} now={now} dense />);
    expect(container.querySelector(".quota-chart--dense .quota-chart-line")).toBeTruthy();
    expect(container.querySelector(".quota-chart-axis")).toBeNull();
  });
});
