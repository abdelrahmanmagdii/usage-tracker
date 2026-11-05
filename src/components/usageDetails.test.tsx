// @vitest-environment jsdom
import { afterEach, expect, it } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import { UsageDetails } from "./UsageDetails";

afterEach(cleanup);

it("exposes the daily chart to assistive tech with a summary", () => {
  render(
    <UsageDetails
      usage={{
        lifetimeTokens: 1_200_000,
        dailyUsage: [
          { startDate: "2026-10-01", tokens: 1_000 },
          { startDate: "2026-10-02", tokens: 2_000 },
        ],
      }}
    />,
  );
  expect(screen.getByRole("img", { name: /Recent daily token usage: 3K tokens over the last 2 days/ })).toBeTruthy();
});

it("shows a bare dash for an unreported streak", () => {
  const { container } = render(<UsageDetails usage={{ currentStreakDays: 4, dailyUsage: [] }} />);
  expect(screen.getByText("4d")).toBeTruthy();
  expect(container.textContent).not.toContain("—d");
});

it("keeps malformed bars inside the chart", () => {
  const { container } = render(
    <UsageDetails
      usage={{
        dailyUsage: [
          { startDate: "2026-10-01", tokens: -500 },
          { startDate: "2026-10-01", tokens: 500 },
        ],
      }}
    />,
  );
  const bars = Array.from(container.querySelectorAll<HTMLElement>(".spark-bars i")).map(
    (bar) => bar.style.transform,
  );
  expect(bars).toEqual(["scaleY(0)", "scaleY(1)"]);
});
