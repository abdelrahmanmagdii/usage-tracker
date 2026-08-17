import type { CSSProperties } from "react";
import type { RateLimitBucket } from "../types/codex";
import {
  formatCountdown,
  formatLocalReset,
  windowDurationLabel,
} from "../lib/rateLimits";

export function QuotaSection({ bucket, now }: { bucket: RateLimitBucket; now: number }) {
  const used = Math.round(bucket.usedPercent);
  const remaining = Math.round(bucket.remainingPercent);
  const tone = bucket.reached || remaining <= 5 ? "danger" : remaining <= 30 ? "warning" : "healthy";
  // Percentages read as REMAINING, matching what the Codex and Claude apps
  // show, so the meter never disagrees with the app it mirrors. The bar fills
  // with what is left, so a full bar means plenty of quota.
  const style = { "--progress": bucket.remainingPercent / 100 } as CSSProperties;
  const heading = bucket.windowLabel ?? windowDurationLabel(bucket.windowDurationMins);

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
      <div className="reset-row">
        <span className="reset-countdown">{formatCountdown(bucket.resetsAt, now)}</span>
        <span className="reset-local">{formatLocalReset(bucket.resetsAt, new Date(now))}</span>
      </div>
    </section>
  );
}
