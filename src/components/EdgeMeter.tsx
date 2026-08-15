import type { CSSProperties } from "react";
import type { RateLimitBucket } from "../types/codex";
import { windowDurationLabel } from "../lib/rateLimits";

export function meterTone(bucket: RateLimitBucket): "healthy" | "warning" | "danger" {
  const remaining = Math.round(bucket.remainingPercent);
  return bucket.reached || remaining <= 5 ? "danger" : remaining <= 30 ? "warning" : "healthy";
}

/** Ultra-thin luminous quota strip flush to the window's top edge. */
export function EdgeMeter({ bucket }: { bucket: RateLimitBucket }) {
  const used = Math.round(bucket.usedPercent);
  return (
    <div
      className={`edge-meter tone-${meterTone(bucket)}`}
      role="progressbar"
      aria-label={`${used} percent of the ${windowDurationLabel(bucket.windowDurationMins)} window used`}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={used}
    >
      <span
        className="edge-meter-fill"
        style={{ "--progress": bucket.usedPercent / 100 } as CSSProperties}
      />
    </div>
  );
}
