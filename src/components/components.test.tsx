// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { TiboWatch } from "./TiboWatch";
import { ResetAlert } from "./ResetAlert";
import { QuotaSection } from "./QuotaSection";
import { ProviderSection } from "./ProviderSection";
import { ConnectionStateView } from "./ConnectionState";
import { SettingsModal } from "./SettingsModal";
import type { RateLimitBucket, ResetEvent } from "../types/codex";

afterEach(() => {
  cleanup();
  localStorage.clear();
});

const now = Date.parse("2026-10-03T12:00:00Z");
const announcement: ResetEvent = {
  id: "tibo-1",
  announcedAt: new Date(now - 30 * 60_000).toISOString(),
  occursAt: new Date(now + 90 * 60_000).toISOString(),
  source: "tibo",
  text: "Global reset landing at 1:30pm PT for all paid ChatGPT accounts.",
  plansAffected: ["All paid plans"],
  sourceUrl: "https://bsky.app/profile/a/post/1",
};
const bucket: RateLimitBucket = {
  id: "codex:primary",
  limitId: "codex",
  windowKind: "primary",
  usedPercent: 40,
  remainingPercent: 60,
  windowDurationMins: 300,
  resetsAt: now / 1000 + 3 * 3600,
  reached: false,
};

describe("TiboWatch", () => {
  it("shows the upcoming post, its plans, and a link", () => {
    const open = vi.spyOn(window, "open").mockReturnValue(null);
    render(<TiboWatch now={now} events={[announcement]} />);
    expect(screen.getByText("Reset incoming")).toBeTruthy();
    expect(screen.getByText(/Global reset landing/)).toBeTruthy();
    expect(screen.getByText("All paid plans")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /View post/ }));
    expect(open).toHaveBeenCalledWith(announcement.sourceUrl, "_blank", "noopener");
  });

  it("says it is watching when nothing is recorded", () => {
    render(<TiboWatch now={now} events={[]} />);
    expect(screen.getAllByText(/Watching for/).length).toBeGreaterThan(0);
  });
});

describe("ResetAlert", () => {
  it("names the plans and can be dismissed", () => {
    render(<ResetAlert now={now} events={[announcement]} />);
    expect(screen.getByRole("status").textContent).toContain("for All paid plans");
    fireEvent.click(screen.getByRole("button", { name: /Dismiss/ }));
    expect(screen.queryByRole("status")).toBeNull();
  });
});

describe("QuotaSection", () => {
  it("renders one compact row in dense mode", () => {
    const { container } = render(<QuotaSection provider="codex" bucket={bucket} now={now} dense />);
    expect(container.querySelector(".quota-section--dense")).toBeTruthy();
    expect(screen.getByRole("progressbar").getAttribute("aria-valuenow")).toBe("60");
  });
});

describe("ProviderSection", () => {
  const meter = (updatedAt: number) => ({
    state: { connection: "connected" as const, updatedAt, rateLimits: { rateLimitsByLimitId: {} } },
    buckets: [bucket],
    refreshing: false,
    refresh: async () => undefined,
  });

  it("shows how fresh the reading is", () => {
    render(<ProviderSection id="claude" label="Claude Code" icon={null} meter={meter(now / 1000 - 120)} now={now} signedOutHint="" />);
    expect(screen.getByText("Updated 2m ago")).toBeTruthy();
  });

  it("warns once a reading is stale", () => {
    render(<ProviderSection id="claude" label="Claude Code" icon={null} meter={meter(now / 1000 - 30 * 60)} now={now} signedOutHint="" />);
    expect(screen.getByText(/Not refreshed · 30m ago/)).toBeTruthy();
  });
});

describe("ConnectionStateView", () => {
  it("collapses to a note when other tools are on screen", () => {
    render(<ConnectionStateView state={{ connection: "error" }} onRetry={() => undefined} compact />);
    expect(screen.getByText("Codex isn't reachable.")).toBeTruthy();
    expect(screen.queryByText("App Server unavailable")).toBeNull();
  });
});

describe("SettingsModal", () => {
  it("toggles reset alerts and compact cards, and reports test notifications", async () => {
    render(<SettingsModal onClose={() => undefined} onShowGuide={() => undefined} />);
    const resetToggle = screen.getByRole("switch", { name: /Reset alerts/ });
    expect(resetToggle.getAttribute("aria-checked")).toBe("true");
    fireEvent.click(resetToggle);
    expect(resetToggle.getAttribute("aria-checked")).toBe("false");
    const dense = screen.getByRole("switch", { name: /Compact cards/ });
    fireEvent.click(dense);
    expect(dense.getAttribute("aria-checked")).toBe("true");
    fireEvent.click(screen.getByRole("button", { name: /Send test notification/ }));
    expect(await screen.findByText(/Couldn't send a notification/)).toBeTruthy();
  });
});
