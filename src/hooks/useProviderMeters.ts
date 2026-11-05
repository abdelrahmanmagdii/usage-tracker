import { useMemo } from "react";
import { useCodexMeter, type CodexMeter } from "./useCodexMeter";
import {
  useAntigravityMeter,
  useClaudeMeter,
  useCursorMeter,
  useDevinMeter,
  useGeminiMeter,
  useOpenCodeMeter,
  type MeterId,
} from "./useClaudeMeter";
import type { ProviderMeter } from "./useProviderMeter";

export type ProviderMeters = { codex: CodexMeter } & Record<MeterId, ProviderMeter>;

/**
 * Every provider's meter keyed by id, for views that walk PROVIDER_CATALOG.
 * The calls are spelled out rather than looped so hook order is fixed by the
 * code, not by data.
 */
export function useProviderMeters(): ProviderMeters {
  const codex = useCodexMeter();
  const claude = useClaudeMeter();
  const cursor = useCursorMeter();
  const opencode = useOpenCodeMeter();
  const devin = useDevinMeter();
  const antigravity = useAntigravityMeter();
  const gemini = useGeminiMeter();
  return useMemo(
    () => ({ codex, claude, cursor, opencode, devin, antigravity, gemini }),
    [codex, claude, cursor, opencode, devin, antigravity, gemini],
  );
}
