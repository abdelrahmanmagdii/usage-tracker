import { invoke } from "@tauri-apps/api/core";

/**
 * True only when the desktop app was started with `--demo` or
 * `USAGEBAR_DEMO=1`. A browser preview is never demo mode.
 */
export function demoMode(): Promise<boolean> {
  if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window)) {
    return Promise.resolve(false);
  }
  return invoke<boolean>("is_demo_mode").catch(() => false);
}
