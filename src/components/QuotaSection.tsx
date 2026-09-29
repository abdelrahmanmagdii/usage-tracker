import { useMemo, type CSSProperties } from "react";
import type { RateLimitBucket } from "../types/codex";
import { historySeries } from "../lib/history";
import {
  formatCountdown,
  formatLocalReset,
  windowDurationLabel,
} from "../lib/rateLimits";

const SPARK_WIDTH = 96;
const SPARK_HEIGHT = 22;
const SPARK_PAD = 2;
const SPARK_POINTS = 40;

/** A quiet trend line of the percent remaining, drawn from local history. */
function Sparkline({ values }: { values: number[] }) {
  const step = (SPARK_WIDTH - SPARK_PAD * 2) / (values.length - 1);
  const y = (value: number) =>
    SPARK_HEIGHT - SPARK_PAD - (Math.min(100, Math.max(0, value)) / 100) * (SPARK_HEIGHT - SPARK_PAD * 2);
  const points = values
    .map((value, index) => `${(SPARK_PAD + index * step).toFixed(2)},${y(value).toFixed(2)}`)
    .join(" ");
  const lastX = SPARK_PAD + (values.length - 1) * step;
  const lastY = y(values[values.length - 1]);
  return (
    <svg
      className="quota-sparkline"
      viewBox={`0 0 ${SPARK_WIDTH} ${SPARK_HEIGHT}`}
      preserveAspectRatio="none"
      aria-hidden="true"
    >
      <polyline points={points} />
      <circle cx={lastX.toFixed(2)} cy={lastY.toFixed(2)} r="1.7" />
    </svg>
  );
}

export function QuotaSection({
  provider,
  bucket,
  now,
}: {
  provider: string;
  bucket: RateLimitBucket;
  now: number;
}) {
  const used = Math.round(bucket.usedPercent);
  const remaining = Math.round(bucket.remainingPercent);
  const tone = bucket.reached || remaining <= 5 ? "danger" : remaining <= 30 ? "warning" : "healthy";
  // Percentages read as REMAINING, matching what the Codex and Claude apps
  // show, so the meter never disagrees with the app it mirrors. The bar fills
  // with what is left, so a full bar means plenty of quota.
  const style = { "--progress": bucket.remainingPercent / 100 } as CSSProperties;
  const heading = bucket.windowLabel ?? windowDurationLabel(bucket.windowDurationMins);
  // Re-read history only when the observation changes, not on every `now` tick.
  const series = useMemo(
    () => historySeries(provider, bucket.id).slice(-SPARK_POINTS).map((s) => s.remainingPercent),
    [provider, bucket.id, bucket.usedPercent, bucket.resetsAt],
  );

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
      {series.length >= 2 ? <Sparkline values={series} /> : null}
      <div className="reset-row">
        <span className="reset-countdown">{formatCountdown(bucket.resetsAt, now)}</span>
        <span className="reset-local">{formatLocalReset(bucket.resetsAt, new Date(now))}</span>
      </div>
    </section>
  );
}
