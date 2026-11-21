// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const ipc = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: ipc.invoke }));

import { getNotificationStatus } from "./notificationDelivery";

describe("notification status over IPC", () => {
  beforeEach(() => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", { value: {}, configurable: true });
  });
  afterEach(() => {
    Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
    ipc.invoke.mockReset();
  });

  it("treats a malformed permission as unavailable, never as postable", async () => {
    for (const reply of [{ permission: "maybe" }, {}, null, "granted", { permission: 3 }]) {
      ipc.invoke.mockResolvedValueOnce(reply);
      expect((await getNotificationStatus()).permission).toBe("unavailable");
    }
  });

  it("keeps the permissions the backend really reports", async () => {
    for (const permission of ["notDetermined", "denied", "granted", "unavailable", "unknown"]) {
      ipc.invoke.mockResolvedValueOnce({ permission, lastError: null });
      expect((await getNotificationStatus()).permission).toBe(permission);
    }
  });

  it("is unavailable when the call fails", async () => {
    ipc.invoke.mockRejectedValueOnce(new Error("no"));
    expect((await getNotificationStatus()).permission).toBe("unavailable");
  });
});
