// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

const ipc = vi.hoisted(() => vi.fn().mockResolvedValue(undefined));
vi.mock("@tauri-apps/api/core", () => ({ invoke: ipc }));

import { TiboWatch, isOpenablePostUrl } from "./TiboWatch";
import { ResetAlert, pickAlertEvent } from "./ResetAlert";
import type { ResetEvent } from "../types/codex";

afterEach(() => {
  cleanup();
  localStorage.clear();
  vi.clearAllMocks();
  vi.restoreAllMocks();
  Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
});

const now = Date.parse("2026-10-03T12:00:00Z");
const minutes = (n: number) => new Date(now + n * 60_000).toISOString();
const post = (partial: Partial<ResetEvent> & { id: string }): ResetEvent => ({
  announcedAt: minutes(-30),
  source: "tibo",
  text: "Global reset landing soon",
  ...partial,
});

describe("reset post links", () => {
  it("accepts only the post hosts the backend will open", () => {
    expect(isOpenablePostUrl("https://bsky.app/profile/a/post/1")).toBe(true);
    expect(isOpenablePostUrl("https://x.com/thsottiaux/status/1")).toBe(true);
    expect(isOpenablePostUrl("javascript:alert(1)")).toBe(false);
    expect(isOpenablePostUrl("https://evil.example/x.com/")).toBe(false);
    expect(isOpenablePostUrl("https://x.com.evil.example/")).toBe(false);
    expect(isOpenablePostUrl("https://x.com/a b")).toBe(false);
    expect(isOpenablePostUrl("https://x.com/a\nb")).toBe(false);
    expect(isOpenablePostUrl(undefined)).toBe(false);
  });

  it("offers no button for a link the backend would refuse", () => {
    const open = vi.spyOn(window, "open").mockReturnValue(null);
    render(<TiboWatch now={now} events={[post({ id: "a", sourceUrl: "javascript:alert(document.cookie)" })]} />);
    expect(screen.queryByRole("button", { name: /View post/ })).toBeNull();
    expect(open).not.toHaveBeenCalled();
  });

  it("opens allowed links through the backend allowlist in the app", () => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", { value: {}, configurable: true });
    const url = "https://bsky.app/profile/a/post/1";
    render(<TiboWatch now={now} events={[post({ id: "a", sourceUrl: url })]} />);
    fireEvent.click(screen.getByRole("button", { name: /View post/ }));
    expect(ipc).toHaveBeenCalledWith("open_post_url", { url });
  });

  it("renders post text as text, never as markup", () => {
    const { container } = render(
      <TiboWatch now={now} events={[post({ id: "a", text: '<img src=x onerror="alert(1)"> reset' })]} />,
    );
    expect(container.querySelector("img")).toBeNull();
    expect(screen.getByText(/<img src=x/)).toBeTruthy();
  });
});

describe("TiboWatch last reset", () => {
  it("does not report a reset still on its way as the last one", () => {
    const events = [
      post({ id: "incoming", announcedAt: minutes(-10), occursAt: minutes(120) }),
      post({ id: "older", announcedAt: minutes(-3 * 60) }),
    ];
    render(<TiboWatch now={now} events={events} />);
    const last = screen.getByText("Last reset").nextElementSibling;
    expect(last?.textContent).toBe("3h ago");
  });
});

describe("ResetAlert", () => {
  it("shows the landed banner when a scheduled reset takes effect, however early it was announced", () => {
    const scheduled = post({ id: "scheduled", announcedAt: minutes(-6 * 60), occursAt: minutes(-5) });
    expect(pickAlertEvent([scheduled], now)?.id).toBe("scheduled");
    render(<ResetAlert now={now} events={[scheduled]} />);
    expect(screen.getByRole("status").textContent).toBe("Quota was just reset");
  });

  it("lets a landed banner expire and ignores samples", () => {
    expect(pickAlertEvent([post({ id: "old", announcedAt: minutes(-120) })], now)).toBeUndefined();
    expect(pickAlertEvent([post({ id: "s", announcedAt: minutes(-5), sample: true })], now)).toBeUndefined();
  });

  it("keeps the ticking countdown out of the live region", () => {
    const incoming = post({ id: "incoming", occursAt: minutes(45), plansAffected: ["Plus"] });
    const { rerender } = render(<ResetAlert now={now} events={[incoming]} />);
    const region = screen.getByRole("status");
    expect(region.textContent).toBe("Reset announced for Plus");
    expect(screen.getByText(/takes effect in 45m/)).toBeTruthy();
    rerender(<ResetAlert now={now + 60_000} events={[incoming]} />);
    expect(screen.getByRole("status").textContent).toBe("Reset announced for Plus");
    expect(screen.getByText(/takes effect in 44m/)).toBeTruthy();
  });
});
