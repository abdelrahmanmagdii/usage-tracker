import type { ResetEvent } from "../../src/types/codex";

export interface TimelineItem {
  id: string;
  text: string;
  announcedAt: string;
  sourceUrl: string;
}

export const BSKY_ACTORS: string[];
export function bskyFeedUrl(actor: string, limit?: number): string;
export function parseBskyFeed(jsonText: unknown, actor: string): TimelineItem[];
export function toResetEvent(item: TimelineItem): ResetEvent | null;
export function mergeEvents(
  existing: ResetEvent[] | null | undefined,
  incoming: ResetEvent[],
): { events: ResetEvent[]; added: number };
