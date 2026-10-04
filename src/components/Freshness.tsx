import { freshness } from "../lib/time";

/** "Updated 2m ago", turning into a warning once readings stop arriving. */
export function Freshness({ updatedAt, now }: { updatedAt?: number | null; now: number }) {
  const info = freshness(updatedAt, now);
  if (!info) return null;
  return (
    <span className={`freshness${info.stale ? " stale" : ""}`} title={info.stale ? "No new reading for a while — UsageBar keeps retrying." : undefined}>
      {info.stale ? `Not refreshed · ${info.label.replace(/^Updated /, "")}` : info.label}
    </span>
  );
}
