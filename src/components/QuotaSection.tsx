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
  // Percentages are shown as USED, matching Codex and Claude Code themselves.
  const style = { "--progress": bucket.usedPercent / 100 } as CSSProperties;
  const heading = bucket.windowLabel ?? windowDurationLabel(bucket.windowDurationMins);

  return (
    <section className={`quota-section glass-tile tone-${tone}`} aria-label={`${heading} quota`}>
      <div className="quota-heading">
        <div>
          <h2>{heading}</h2>
          {bucket.limitName ? <p>{bucket.limitName}</p> : null}
        </div>
        <span className="quota-number"><strong>{used}</strong>% used</span>
      </div>
      <div
        className="progress-track"
        role="progressbar"
        aria-label={`${used} percent used`}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={used}
      >
        <span className="progress-fill" style={style} />
      </div>
      <div className="quota-caption">
        <span>{bucket.reached ? "Limit reached" : `${used}% used`}</span>
        <span>{remaining}% left</span>
      </div>
      <div className="reset-row">
        <span className="reset-countdown">{formatCountdown(bucket.resetsAt, now)}</span>
        <span className="reset-local">{formatLocalReset(bucket.resetsAt, new Date(now))}</span>
      </div>
    </section>
  );
}
