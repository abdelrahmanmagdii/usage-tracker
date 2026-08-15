import { Radio } from "lucide-react";
import type { ResetEvent } from "../types/codex";
import { formatLeadTime, relativeTime } from "../lib/time";
import { tiboStatus, upcomingReset } from "../features/tibo-watch/provider";

export function TiboWatch({ now, events }: { now: number; events: ResetEvent[] }) {
  const status = tiboStatus(events, now);
  const upcoming = upcomingReset(events, now);
  const latest = events[0];
  const thisMonth = events.filter((event) => {
    const date = new Date(event.occurredAt ?? event.announcedAt);
    const current = new Date(now);
    return date.getMonth() === current.getMonth() && date.getFullYear() === current.getFullYear();
  }).length;

  return (
    <section className="tibo-watch" aria-labelledby="tibo-heading">
      <div className="section-label"><Radio size={14} aria-hidden="true" /><span id="tibo-heading">Reset radar</span></div>
      <div className="tibo-status">
        <span className={`status-dot ${status.tone}`} aria-hidden="true" />
        <strong>{status.label}</strong>
        {latest?.sample ? <span className="sample-badge">sample</span> : null}
      </div>
      {upcoming?.occursAt ? (
        <div className="tibo-meta tibo-incoming">
          <span>Takes effect in</span>
          <strong>{formatLeadTime(Date.parse(upcoming.occursAt) - now)}</strong>
        </div>
      ) : null}
      <div className="tibo-meta">
        <span>Last reset</span>
        <strong>{relativeTime(latest?.occurredAt ?? latest?.announcedAt, now)}</strong>
      </div>
      <div className="tibo-meta">
        <span>Recorded this month</span>
        <strong>{thisMonth}</strong>
      </div>
    </section>
  );
}
