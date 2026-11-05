// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { AppPrefs } from "../lib/providers";

const backend = vi.hoisted(() => ({
  prefs: null as unknown as AppPrefs,
  failing: new Set<string>(),
  autostart: false,
  handlers: [] as Array<(event: { payload: AppPrefs }) => void>,
  prefsReads: [] as Array<(prefs: AppPrefs) => void>,
  deferPrefsRead: false,
}));

const invoke = vi.hoisted(() =>
  vi.fn(async (command: string, _args?: unknown): Promise<unknown> => {
    if (backend.failing.has(command)) throw new Error(`${command} failed`);
    switch (command) {
      case "get_app_prefs":
        if (backend.deferPrefsRead) {
          return new Promise((resolve) => backend.prefsReads.push(resolve));
        }
        return structuredClone(backend.prefs);
      case "get_tray_windows":
        return {
          codex: [{ id: "codex:primary", label: "5-hour", usedPercent: 18 }],
          claude: [],
          cursor: [],
          opencode: [],
          devin: [],
          antigravity: [],
          gemini: [],
        };
      case "get_autostart":
        return backend.autostart;
      default:
        return undefined;
    }
  }),
);
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (_name: string, handler: (event: { payload: AppPrefs }) => void) => {
    backend.handlers.push(handler);
    return () => {
      backend.handlers = backend.handlers.filter((entry) => entry !== handler);
    };
  }),
}));
vi.mock("@tauri-apps/plugin-notification", () => ({
  isPermissionGranted: vi.fn().mockResolvedValue(false),
  requestPermission: vi.fn().mockResolvedValue("granted"),
  sendNotification: vi.fn(),
}));

import { SettingsModal, parseThreshold, replaceThreshold } from "./SettingsModal";
import { Onboarding } from "./Onboarding";
import { DEFAULT_PREFS } from "../lib/providers";

function broadcast(prefs: AppPrefs) {
  act(() => {
    for (const handler of backend.handlers) handler({ payload: prefs });
  });
}

async function openSettings() {
  const view = render(<SettingsModal onClose={() => undefined} onShowGuide={() => undefined} />);
  await act(async () => {});
  return view;
}

const commands = (name: string) => invoke.mock.calls.filter(([command]) => command === name).map(([, args]) => args);

beforeEach(() => {
  backend.prefs = { ...DEFAULT_PREFS, providers: {} };
  backend.failing.clear();
  backend.autostart = false;
  backend.handlers = [];
  backend.prefsReads = [];
  backend.deferPrefsRead = false;
  Object.defineProperty(window, "__TAURI_INTERNALS__", { value: {}, configurable: true });
});

afterEach(() => {
  cleanup();
  invoke.mockClear();
  Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
});

describe("threshold parsing", () => {
  it("accepts only whole percents from 1 to 100, like the backend", () => {
    expect(parseThreshold("80")).toBe(80);
    expect(parseThreshold(" 100 ")).toBe(100);
    expect(parseThreshold("12.5")).toBeNull();
    expect(parseThreshold("1e2")).toBeNull();
    expect(parseThreshold("0")).toBeNull();
    expect(parseThreshold("101")).toBeNull();
    expect(parseThreshold("-5")).toBeNull();
    expect(parseThreshold("")).toBeNull();
  });

  it("replaces one level and keeps the rest, deduped and ascending", () => {
    expect(replaceThreshold([50, 80, 95], 0, 60)).toEqual([60, 80, 95]);
    expect(replaceThreshold([50, 80, 95], 2, 70)).toEqual([50, 70, 80]);
    expect(replaceThreshold([80, 95], 0, 95)).toEqual([95]);
  });
});

describe("SettingsModal", () => {
  it("is a labelled modal dialog that takes focus and returns it on close", async () => {
    const opener = document.createElement("button");
    document.body.append(opener);
    opener.focus();
    const view = await openSettings();
    const dialog = screen.getByRole("dialog", { name: "Settings" });
    expect(dialog.getAttribute("aria-modal")).toBe("true");
    expect(dialog.contains(document.activeElement)).toBe(true);
    view.unmount();
    expect(document.activeElement).toBe(opener);
    opener.remove();
  });

  it("keeps Tab inside the dialog", async () => {
    await openSettings();
    const dialog = screen.getByRole("dialog");
    const buttons = Array.from(dialog.querySelectorAll<HTMLElement>("button:not([disabled]), input"));
    const last = buttons[buttons.length - 1];
    last.focus();
    fireEvent.keyDown(last, { key: "Tab" });
    expect(document.activeElement).toBe(buttons[0]);
    fireEvent.keyDown(buttons[0], { key: "Tab", shiftKey: true });
    expect(document.activeElement).toBe(last);
  });

  it("sends each control to its own command", async () => {
    await openSettings();
    fireEvent.click(screen.getByRole("switch", { name: /^Codex/ }));
    expect(commands("set_provider_visible")).toEqual([{ provider: "codex", visible: false }]);
    fireEvent.click(screen.getByRole("switch", { name: /^Claude Code/ }));
    expect(commands("set_provider_visible")).toContainEqual({ provider: "claude", visible: false });
    fireEvent.click(screen.getByRole("radio", { name: /Extended/ }));
    expect(commands("set_combined_tray")).toEqual([{ enabled: false }]);
    fireEvent.click(screen.getByRole("switch", { name: /Compact cards/ }));
    expect(commands("set_dense_layout")).toEqual([{ enabled: true }]);
    fireEvent.click(screen.getByRole("switch", { name: /Reset alerts/ }));
    expect(commands("set_reset_alerts")).toEqual([{ enabled: false }]);
    fireEvent.click(screen.getByRole("switch", { name: /Launch at login/ }));
    expect(commands("set_autostart")).toEqual([{ enabled: true }]);
    fireEvent.click(screen.getByRole("switch", { name: /Usage alerts/ }));
    expect(commands("set_usage_alerts")).toEqual([{ enabled: false }]);
  });

  it("picks a tray window", async () => {
    await openSettings();
    const radio = screen.getByRole("radio", { name: /5-hour/ });
    fireEvent.click(radio);
    expect(commands("set_tray_window")).toEqual([{ provider: "codex", window: "codex:primary" }]);
    expect(radio.getAttribute("aria-checked")).toBe("true");
  });

  it("reflects prefs changed elsewhere (tray menu) while open", async () => {
    await openSettings();
    expect(screen.getByRole("switch", { name: /^Codex/ }).getAttribute("aria-checked")).toBe("true");
    broadcast({ ...DEFAULT_PREFS, providers: { codex: { visible: false } }, denseLayout: true });
    expect(screen.getByRole("switch", { name: /^Codex/ }).getAttribute("aria-checked")).toBe("false");
    expect(screen.getByRole("switch", { name: /Compact cards/ }).getAttribute("aria-checked")).toBe("true");
  });

  it("follows Launch at Login toggled from the tray menu while open", async () => {
    await openSettings();
    const toggle = () => screen.getByRole("switch", { name: /Launch at login/i });
    expect(toggle().getAttribute("aria-checked")).toBe("false");
    // The tray toggle changes the OS setting, then broadcasts prefs.
    backend.autostart = true;
    broadcast({ ...DEFAULT_PREFS, providers: {} });
    await act(async () => {});
    expect(toggle().getAttribute("aria-checked")).toBe("true");
  });

  it("does not let a slow initial read roll back a newer broadcast", async () => {
    backend.deferPrefsRead = true;
    await openSettings();
    broadcast({ ...DEFAULT_PREFS, providers: {}, resetAlerts: false });
    await act(async () => backend.prefsReads[0]({ ...DEFAULT_PREFS, providers: {}, resetAlerts: true }));
    expect(screen.getByRole("switch", { name: /Reset alerts/ }).getAttribute("aria-checked")).toBe("false");
  });

  it("re-reads stored prefs when a save fails instead of showing an unsaved choice", async () => {
    backend.failing.add("set_provider_visible");
    await openSettings();
    const codex = screen.getByRole("switch", { name: /^Codex/ });
    await act(async () => {
      fireEvent.click(codex);
    });
    expect(codex.getAttribute("aria-checked")).toBe("true");
  });

  it("keeps a middle alert level when editing the outer ones", async () => {
    backend.prefs = { ...DEFAULT_PREFS, providers: {}, usageAlertThresholds: [50, 80, 95] };
    await openSettings();
    const warn = screen.getByRole("spinbutton", { name: "Warn at" });
    fireEvent.change(warn, { target: { value: "60" } });
    fireEvent.blur(warn);
    expect(commands("set_usage_alert_thresholds")).toEqual([{ thresholds: [60, 80, 95] }]);
    expect(screen.getByText(/60%, 80% and 95% used/)).toBeTruthy();
  });

  it("adds a level beside a single stored one instead of replacing it", async () => {
    backend.prefs = { ...DEFAULT_PREFS, providers: {}, usageAlertThresholds: [90] };
    await openSettings();
    const warn = screen.getByRole("spinbutton", { name: "Warn at" });
    fireEvent.change(warn, { target: { value: "75" } });
    fireEvent.keyDown(warn, { key: "Enter" });
    fireEvent.blur(warn);
    expect(commands("set_usage_alert_thresholds")).toEqual([{ thresholds: [75, 90] }]);
  });

  it("discards a threshold the backend would reject", async () => {
    await openSettings();
    const last = screen.getByRole("spinbutton", { name: "Last call at" }) as HTMLInputElement;
    for (const value of ["12.5", "0", "250", ""]) {
      fireEvent.change(last, { target: { value } });
      fireEvent.blur(last);
    }
    expect(commands("set_usage_alert_thresholds")).toEqual([]);
    expect(last.value).toBe("95");
  });
});

describe("Onboarding", () => {
  const providers = [{ label: "Codex", connected: true, detail: "Connected", onRetry: () => undefined }];

  it("is a modal dialog that starts on the primary action and reports progress", () => {
    render(<Onboarding providers={providers} onClose={() => undefined} />);
    const dialog = screen.getByRole("dialog", { name: "UsageBar setup" });
    expect(dialog.getAttribute("aria-modal")).toBe("true");
    expect(document.activeElement).toBe(screen.getByRole("button", { name: /Next/ }));
    expect(screen.getByRole("img", { name: "Step 1 of 4" })).toBeTruthy();
  });

  it("keeps focus on the primary action when Next becomes Done", () => {
    render(<Onboarding providers={providers} onClose={() => undefined} />);
    for (let index = 0; index < 3; index += 1) {
      const next = screen.getByRole("button", { name: /Next/ });
      next.focus();
      fireEvent.click(next);
    }
    expect(screen.getByRole("img", { name: "Step 4 of 4" })).toBeTruthy();
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Done" }));
  });

  it("returns focus to whatever opened it", () => {
    const opener = document.createElement("button");
    document.body.append(opener);
    opener.focus();
    const view = render(<Onboarding providers={providers} onClose={() => undefined} />);
    expect(document.activeElement).not.toBe(opener);
    view.unmount();
    expect(document.activeElement).toBe(opener);
    opener.remove();
  });
});
