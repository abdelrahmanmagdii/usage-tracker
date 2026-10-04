import { invoke } from "@tauri-apps/api/core";
import { ExternalLink, Radio } from "lucide-react";
import type { ResetEvent } from "../types/codex";
import { formatLeadTime, relativeTime } from "../lib/time";
import { tiboStatus, upcomingReset } from "../features/tibo-watch/provider";

const MAX_POST_CHARS = 140;

function openPost(url: string) {
  if ("__TAURI_INTERNALS__" in window) void invoke("open_post_url", { url }).catch(() => undefined);
  else window.open(url, "_blank", "noopener");
}

export function TiboWatch({ now, events }: { now: number; events: ResetEvent[] }) {
  const status = tiboStatus(events, now);
  const upcoming = upcomingReset(events, now);
  const latest = events[0];
  // The post worth reading: the one about to land, else the most recent.
  const post = upcoming ?? latest;
  const thisMonth = events.filter((event) => {
    const date = new Date(event.occurredAt ?? event.announcedAt);
    const current = new Date(now);
    return date.getMonth() === current.getMonth() && date.getFullYear() === current.getFullYear();
  }).length;
  const text = post?.text && post.source !== "detected" ? post.text : null;

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
      {text ? (
        <blockquote className="tibo-post">
          <p>{text.length > MAX_POST_CHARS ? `${text.slice(0, MAX_POST_CHARS - 1)}…` : text}</p>
          <footer>
            {post?.plansAffected?.length ? (
              <span className="tibo-plans">
                {post.plansAffected.map((plan) => <span key={plan} className="plan-chip">{plan}</span>)}
              </span>
            ) : <span />}
            {post?.sourceUrl ? (
              <button type="button" className="tibo-link" onClick={() => openPost(post.sourceUrl!)}>
                View post <ExternalLink size={11} aria-hidden="true" />
              </button>
            ) : null}
          </footer>
        </blockquote>
      ) : null}
      <div className="tibo-meta">
        <span>Last reset</span>
        <strong>{latest ? relativeTime(latest.occurredAt ?? latest.announcedAt, now) : "Watching for Tibo's posts"}</strong>
      </div>
      <div className="tibo-meta">
        <span>Recorded this month</span>
        <strong>{thisMonth}</strong>
      </div>
    </section>
  );
}
