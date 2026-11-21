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
  permission: "granted" as string,
  accountActive: "default",
  accountReply: null as unknown,
  lastError: null as null | { source: string; message: string; at: number },
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
      case "get_account_sources":
        return {
          activeId: backend.accountActive,
          choices: [
            { id: "default", label: "Codex CLI (default)", custom: false },
            { id: "custom-1", label: "Work", custom: true },
          ],
          folderHint: "A Codex home folder that holds auth.json.",
          diagnostic: null,
          unavailable: null,
        };
      case "add_account_profile":
        return backend.accountReply;
      case "switch_account_profile":
        backend.accountActive = (_args as { id: string }).id;
        return undefined;
      case "get_notification_status":
        return { permission: backend.permission, lastError: backend.lastError };
      case "request_notification_permission":
        backend.permission = "granted";
        return { permission: backend.permission, lastError: null };
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
  backend.permission = "granted";
  backend.lastError = null;
  backend.accountActive = "default";
  backend.accountReply = null;
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

  it("saves the pace alerts toggle with its own command", async () => {
    await openSettings();
    const pace = screen.getByRole("switch", { name: /Pace alerts/ });
    expect(pace.getAttribute("aria-checked")).toBe("true");
    fireEvent.click(pace);
    expect(commands("set_pace_alerts")).toEqual([{ enabled: false }]);
    expect(pace.getAttribute("aria-checked")).toBe("false");
  });

  it("follows pace alerts changed elsewhere and hides them with usage alerts off", async () => {
    await openSettings();
    // The backend leaves paceAlerts out while it is on.
    broadcast({ ...DEFAULT_PREFS, providers: {}, paceAlerts: false });
    expect(screen.getByRole("switch", { name: /Pace alerts/ }).getAttribute("aria-checked")).toBe("false");
    const sparse = { ...DEFAULT_PREFS, providers: {} } as Partial<AppPrefs>;
    delete sparse.paceAlerts;
    broadcast(sparse as AppPrefs);
    expect(screen.getByRole("switch", { name: /Pace alerts/ }).getAttribute("aria-checked")).toBe("true");
    // Nested under usage alerts: no switch to flip while the master is off.
    broadcast({ ...DEFAULT_PREFS, providers: {}, usageAlerts: false });
    expect(screen.queryByRole("switch", { name: /Pace alerts/ })).toBeNull();
  });

  it("reverts the pace alerts toggle when saving it fails", async () => {
    backend.failing.add("set_pace_alerts");
    await openSettings();
    const pace = screen.getByRole("switch", { name: /Pace alerts/ });
    await act(async () => {
      fireEvent.click(pace);
    });
    expect(commands("set_pace_alerts")).toEqual([{ enabled: false }]);
    expect(pace.getAttribute("aria-checked")).toBe("true");
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

describe("SettingsModal notifications", () => {
  it("shows the real permission and offers Enable only before it is decided", async () => {
    backend.permission = "granted";
    const view = await openSettings();
    expect(screen.getByText("Notifications are allowed for UsageBar.")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Enable notifications/ })).toBeNull();
    view.unmount();

    backend.permission = "notDetermined";
    await openSettings();
    const enable = screen.getByRole("button", { name: /Enable notifications/ });
    await act(async () => {
      fireEvent.click(enable);
    });
    expect(commands("request_notification_permission")).toHaveLength(1);
    expect(screen.getByText("Notifications are allowed for UsageBar.")).toBeTruthy();
  });

  it("explains how to fix blocked permission and never requests it unprompted", async () => {
    backend.permission = "denied";
    await openSettings();
    expect(screen.getByText(/Notifications are blocked\. Turn them on in/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Enable notifications/ })).toBeNull();
    expect(commands("request_notification_permission")).toEqual([]);
  });

  it("re-reads the permission when the window regains focus", async () => {
    backend.permission = "denied";
    await openSettings();
    backend.permission = "granted";
    await act(async () => {
      window.dispatchEvent(new Event("focus"));
    });
    expect(screen.getByText("Notifications are allowed for UsageBar.")).toBeTruthy();
  });

  it("tests with reset alerts off, saying accepted rather than delivered", async () => {
    backend.prefs = { ...backend.prefs, resetAlerts: false };
    await openSettings();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /Send test notification/ }));
    });
    expect(commands("post_usage_notification")).toEqual([
      expect.objectContaining({ source: "test" }),
    ]);
    expect(screen.getByText("Accepted by macOS. Focus can still hide the banner.")).toBeTruthy();
  });

  it("shows the last delivery error", async () => {
    backend.lastError = { source: "pace", message: "Notifications are turned off for UsageBar.", at: 1 };
    await openSettings();
    expect(screen.getByText(/Last alert failed: Notifications are turned off/)).toBeTruthy();
  });
});

describe("SettingsModal accounts outside the desktop app", () => {
  it("explains instead of calling the backend", async () => {
    Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
    render(<SettingsModal onClose={() => undefined} onShowGuide={() => undefined} />);
    await act(async () => {});
    expect(screen.getByText("Open the desktop app to choose a local account.")).toBeTruthy();
    expect(commands("get_account_sources")).toEqual([]);
    expect(screen.queryByRole("alert")).toBeNull();
  });
});

describe("SettingsModal accounts", () => {
  const sourceSelect = () => screen.getByLabelText("Login source") as HTMLSelectElement;

  it("lists the saved sources and enables the switch only for a different one", async () => {
    await openSettings();
    expect(sourceSelect().value).toBe("default");
    const use = screen.getByRole("button", { name: "Use profile and restart UsageBar" }) as HTMLButtonElement;
    expect(use.disabled).toBe(true);
    fireEvent.change(sourceSelect(), { target: { value: "custom-1" } });
    expect(use.disabled).toBe(false);
    await act(async () => {
      fireEvent.click(use);
    });
    expect(commands("switch_account_profile")).toEqual([{ provider: "codex", id: "custom-1" }]);
    expect(screen.getByText(/Switching restarts UsageBar only/)).toBeTruthy();
    // A backend that does not restart is re-read, so "(active)" follows.
    expect(screen.getByRole("option", { name: "Work (active)" })).toBeTruthy();
    expect((screen.getByRole("button", { name: "Use profile and restart UsageBar" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("keeps the selection and explains when a switch fails", async () => {
    backend.failing.add("switch_account_profile");
    await openSettings();
    fireEvent.change(sourceSelect(), { target: { value: "custom-1" } });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Use profile and restart UsageBar" }));
    });
    expect(sourceSelect().value).toBe("custom-1");
    expect(screen.getByRole("alert").textContent).toMatch(/switch_account_profile failed/);
  });

  it("adds a local profile with an optional name and treats cancel as nothing", async () => {
    await openSettings();
    fireEvent.change(screen.getByLabelText("Profile name (optional)"), { target: { value: "Team" } });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /Add local profile/ }));
    });
    expect(commands("add_account_profile")).toEqual([{ provider: "codex", label: "Team" }]);
    expect(sourceSelect().value).toBe("default");
    backend.accountReply = {
      activeId: "default",
      choices: [
        { id: "default", label: "Codex CLI (default)", custom: false },
        { id: "custom-1", label: "Work", custom: true },
        { id: "custom-2", label: "Team", custom: true },
      ],
      folderHint: "x",
    };
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /Add local profile/ }));
    });
    expect(sourceSelect().value).toBe("custom-2");
  });

  it("forgets only an inactive custom profile", async () => {
    await openSettings();
    const forget = screen.getByRole("button", { name: /Forget profile/ }) as HTMLButtonElement;
    expect(forget.disabled).toBe(true);
    fireEvent.change(sourceSelect(), { target: { value: "custom-1" } });
    expect(forget.disabled).toBe(false);
    await act(async () => {
      fireEvent.click(forget);
    });
    expect(commands("forget_account_profile")).toEqual([{ provider: "codex", id: "custom-1" }]);
  });

  it("shows an inline error and re-reads the stored prefs when a save fails", async () => {
    backend.failing.add("set_dense_layout");
    await openSettings();
    await act(async () => {
      fireEvent.click(screen.getByRole("switch", { name: /Compact cards/ }));
    });
    expect(screen.getByText(/Could not save the change/)).toBeTruthy();
    expect(commands("get_app_prefs").length).toBeGreaterThan(1);
  });
});

describe("Onboarding notifications", () => {
  const providers = [{ label: "Codex", connected: true, detail: "Connected", onRetry: () => undefined }];

  async function resetsStep() {
    render(<Onboarding providers={providers} onClose={() => undefined} />);
    for (let index = 0; index < 4 && !screen.queryByRole("heading", { name: "Resets" }); index += 1) {
      fireEvent.click(screen.getByRole("button", { name: /Next/ }));
    }
    await act(async () => {});
  }

  it("offers Enable only while permission is undecided", async () => {
    backend.permission = "notDetermined";
    await resetsStep();
    expect(screen.getByRole("button", { name: "Enable notifications" })).toBeTruthy();
  });

  it.each(["denied", "unavailable", "unknown"])("shows a settings hint instead of Enable when %s", async (permission) => {
    backend.permission = permission;
    await resetsStep();
    expect(screen.queryByRole("button", { name: "Enable notifications" })).toBeNull();
    expect(screen.getByText(/Notifications are blocked|unavailable|cannot check/)).toBeTruthy();
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
