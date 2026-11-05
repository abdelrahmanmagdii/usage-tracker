import { useMemo, useState } from "react";
import { X, Zap } from "lucide-react";
import type { ResetEvent } from "../types/codex";
import { formatLeadTime } from "../lib/time";
import { latestLandedReset, upcomingReset } from "../features/tibo-watch/provider";
import { planSummary } from "../features/tibo-watch/notifications";

const DISMISS_KEY = "codex-meter.dismissed-alerts.v1";
const LANDED_WINDOW_MS = 90 * 60 * 1000;

function readDismissed(): Set<string> {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(DISMISS_KEY) ?? "[]");
    return new Set(Array.isArray(value) ? value.filter((id): id is string => typeof id === "string") : []);
  } catch {
    return new Set();
  }
}

/**
 * Picks the event worth surfacing: an upcoming reset first, otherwise one
 * that just landed. "Just landed" is measured from when the reset took
 * effect, so a reset announced hours ahead still gets its banner when its
 * scheduled time arrives.
 */
export function pickAlertEvent(events: ResetEvent[], nowMs: number): ResetEvent | undefined {
  const upcoming = upcomingReset(events, nowMs);
  if (upcoming) return upcoming;
  const landed = latestLandedReset(
    events.filter((event) => !event.sample),
    nowMs,
  );
  return landed && nowMs - landed.at <= LANDED_WINDOW_MS ? landed.event : undefined;
}

/** In-app banner for announced resets; system notifications remain the other channel. */
export function ResetAlert({ now, events }: { now: number; events: ResetEvent[] }) {
  const [dismissed, setDismissed] = useState(readDismissed);
  const candidate = useMemo(() => pickAlertEvent(events, now), [events, now]);
  if (!candidate || dismissed.has(candidate.id)) return null;

  const occursAt = candidate.occursAt ? Date.parse(candidate.occursAt) : Number.NaN;
  const incoming = Number.isFinite(occursAt) && occursAt > now;
  const plans = planSummary(candidate);

  const dismiss = () => {
    const next = new Set(dismissed).add(candidate.id);
    setDismissed(next);
    try {
      localStorage.setItem(DISMISS_KEY, JSON.stringify([...next].slice(-100)));
    } catch {
      /* storage unavailable — dismissal just won't persist */
    }
  };

  // Only the headline is a live region. The countdown re-renders every
  // minute, and inside the region a screen reader would re-announce the
  // whole banner each time; it stays readable as ordinary text beside it.
  return (
    <div className={`reset-alert ${incoming ? "incoming" : "landed"}`}>
      <Zap size={14} aria-hidden="true" />
      <span className="reset-alert-text">
        <span role="status">
          {incoming ? `Reset announced${plans ? ` for ${plans}` : ""}` : "Quota was just reset"}
        </span>
        {incoming ? (
          <>
            {" "}
            <strong>· takes effect in {formatLeadTime(occursAt - now)}</strong>
          </>
        ) : null}
      </span>
      <button onClick={dismiss} aria-label="Dismiss reset announcement"><X size={13} /></button>
    </div>
  );
}
