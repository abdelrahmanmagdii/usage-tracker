export function relativeTime(isoDate?: string, nowMs = Date.now()): string {
  if (!isoDate) return "No reset sightings yet";
  const elapsed = Math.max(0, nowMs - Date.parse(isoDate));
  const hours = Math.floor(elapsed / 3_600_000);
  const days = Math.floor(hours / 24);
  if (days > 0) return `${days}d ${hours % 24}h ago`;
  if (hours > 0) return `${hours}h ago`;
  const minutes = Math.max(1, Math.floor(elapsed / 60_000));
  return `${minutes}m ago`;
}

/**
 * How long ago a meter last got real numbers, from a unix timestamp in seconds
 * (what the backend stores in `updatedAt`). Used to say plainly when a tile is
 * showing last-known usage rather than current usage.
 */
export function describeAge(updatedAtSeconds?: number | null, nowMs = Date.now()): string | null {
  if (typeof updatedAtSeconds !== "number" || !Number.isFinite(updatedAtSeconds)) return null;
  const elapsed = Math.max(0, nowMs / 1_000 - updatedAtSeconds);
  const minutes = Math.floor(elapsed / 60);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.floor(hours / 24)}d ago`;
}

/** Compact human lead time, e.g. "45m", "2h 15m", "2 days". */
export function formatLeadTime(ms: number): string {
  const minutes = Math.max(1, Math.round(ms / 60_000));
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  if (hours < 24) return rest > 0 ? `${hours}h ${rest}m` : `${hours}h`;
  const days = Math.floor(hours / 24);
  return days === 1 ? "1 day" : `${days} days`;
}
