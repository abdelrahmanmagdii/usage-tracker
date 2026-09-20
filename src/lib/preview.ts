import type { ResetEvent } from "../types/codex";
import { DEFAULT_PREFS, PROVIDER_IDS, type AppPrefs } from "./providers";

function params(search: string): URLSearchParams {
  return new URLSearchParams(search.startsWith("?") ? search.slice(1) : search);
}

/** True only in Vite dev when the page is opened with `?preview`. */
export function isDevPreview(search: string): boolean {
  return Boolean(import.meta.env.DEV) && params(search).has("preview");
}

/** Optional `?providers=codex,claude` filter so captures can show one tool. */
export function previewPrefs(search: string): AppPrefs | null {
  if (!isDevPreview(search)) return null;
  const raw = params(search).get("providers");
  const allowed = raw
    ? new Set(raw.split(",").map((id) => id.trim()).filter(Boolean))
    : null;
  return {
    ...DEFAULT_PREFS,
    onboardingComplete: true,
    providers: Object.fromEntries(
      PROVIDER_IDS.map((id) => [id, { visible: allowed ? allowed.has(id) : true }]),
    ),
  };
}

/** Upcoming reset used by the demo GIF. Opt in with `?alert`. */
export function previewResetEvent(nowMs: number, search: string): ResetEvent | null {
  if (!isDevPreview(search) || !params(search).has("alert")) return null;
  const parsed = Number(params(search).get("resetIn") ?? "42");
  const minutes = Number.isFinite(parsed) && parsed > 0 ? parsed : 42;
  return {
    id: "preview-reset",
    announcedAt: new Date(nowMs).toISOString(),
    occursAt: new Date(nowMs + minutes * 60_000).toISOString(),
    source: "tibo",
    text: "Preview reset announcement",
  };
}
