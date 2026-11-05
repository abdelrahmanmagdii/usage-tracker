import { useMemo, type CSSProperties } from "react";
import type { RateLimitBucket } from "../types/codex";
import { historySeries, projectedRunOut } from "../lib/history";
import { buildTimeline } from "../lib/quotaTimeline";
import { formatLeadTime } from "../lib/time";
import {
  formatCountdown,
  formatLocalReset,
  windowDurationLabel,
} from "../lib/rateLimits";
import { QuotaChart } from "./QuotaChart";

/** The pace warning, keyed to the dashed projection it describes. */
function PaceNote({ text, as: Tag }: { text: string; as: "p" | "span" }) {
  return (
    <Tag className="quota-pace">
      <svg className="quota-pace-key" width="14" height="4" viewBox="0 0 14 4" aria-hidden="true">
        <line x1="1" y1="2" x2="13" y2="2" />
      </svg>
      {text}
    </Tag>
  );
}

export function QuotaSection({
  provider,
  bucket,
  now,
  dense = false,
  live = true,
}: {
  provider: string;
  bucket: RateLimitBucket;
  now: number;
  dense?: boolean;
  /** False when the provider is failing, signed out or stale: the recorded
   * history still shows, but no "runs out in" projection is made from it. */
  live?: boolean;
}) {
  const used = Math.round(bucket.usedPercent);
  const remaining = Math.round(bucket.remainingPercent);
  const tone = bucket.reached || remaining <= 5 ? "danger" : remaining <= 30 ? "warning" : "healthy";
  // Percentages read as REMAINING, matching what the Codex and Claude apps
  // show, so the meter never disagrees with the app it mirrors. The bar fills
  // with what is left, so a full bar means plenty of quota.
  const style = { "--progress": bucket.remainingPercent / 100 } as CSSProperties;
  const heading = bucket.windowLabel ?? windowDurationLabel(bucket.windowDurationMins);
  // Re-read history only when the observation changes, not on every `now`
  // tick. The read is cached in history.ts, but the memo also keeps the
  // array identity stable for the memos below.
  const history = useMemo(
    () => historySeries(provider, bucket.id),
    [provider, bucket.id, bucket.usedPercent, bucket.resetsAt],
  );
  // Minute resolution is plenty for the pace estimate and the chart's "now";
  // it keeps both from recomputing on every one-second tick.
  const minute = Math.floor(now / 60_000);
  const runOut = useMemo(
    () => (live ? projectedRunOut(history, bucket, minute * 60_000) : null),
    [live, history, bucket, minute],
  );
  const timeline = useMemo(
    () => buildTimeline(history, bucket, minute * 60_000, runOut),
    [history, bucket, minute, runOut],
  );
  const pace = runOut ? `At this pace, runs out in ${formatLeadTime(runOut - now)}` : null;

  if (dense) {
    return (
      <section className={`quota-section quota-section--dense glass-tile tone-${tone}`} aria-label={`${heading} quota`}>
        <div className="dense-row">
          <h2>{heading}</h2>
          <div
            className="progress-track"
            role="progressbar"
            aria-label={`${remaining} percent remaining`}
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={remaining}
          >
            <span className="progress-fill" style={style} />
          </div>
          <span className="quota-number"><strong>{remaining}</strong>%</span>
        </div>
        {timeline ? <QuotaChart timeline={timeline} dense /> : null}
        <div className="dense-meta">
          <span className="reset-countdown">{formatCountdown(bucket.resetsAt, now)}</span>
          {pace ? <PaceNote text={pace} as="span" /> : null}
        </div>
      </section>
    );
  }

  return (
    <section className={`quota-section glass-tile tone-${tone}`} aria-label={`${heading} quota`}>
      <div className="quota-heading">
        <div>
          <h2>{heading}</h2>
          {bucket.limitName ? <p>{bucket.limitName}</p> : null}
        </div>
        <span className="quota-number"><strong>{remaining}</strong>% left</span>
      </div>
      <div
        className="progress-track"
        role="progressbar"
        aria-label={`${remaining} percent remaining`}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={remaining}
      >
        <span className="progress-fill" style={style} />
      </div>
      <div className="quota-caption">
        <span>{bucket.reached ? "Limit reached" : `${remaining}% left`}</span>
        <span>{used}% used</span>
      </div>
      {timeline ? <QuotaChart timeline={timeline} /> : null}
      {pace ? <PaceNote text={pace} as="p" /> : null}
      <div className="reset-row">
        <span className="reset-countdown">{formatCountdown(bucket.resetsAt, now)}</span>
        <span className="reset-local">{formatLocalReset(bucket.resetsAt, new Date(now))}</span>
      </div>
    </section>
  );
}
