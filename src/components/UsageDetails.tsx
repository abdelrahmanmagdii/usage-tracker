import { Activity, ChevronDown } from "lucide-react";
import type { UsageSummary } from "../types/codex";
import { formatCompactNumber } from "../lib/rateLimits";

/** "12d", or a bare dash when the streak was not reported (not "—d"). */
function streak(days?: number): string {
  return days === undefined ? "—" : `${days}d`;
}

export function UsageDetails({ usage }: { usage: UsageSummary }) {
  const recent = usage.dailyUsage.slice(-7);
  // A negative count from a malformed payload would flip its bar upside down.
  const max = Math.max(...recent.map((day) => day.tokens), 1);
  const scale = (tokens: number) => Math.min(1, Math.max(0, tokens / max));
  const total = recent.reduce((sum, day) => sum + Math.max(0, day.tokens), 0);
  return (
    <details className="usage-details">
      <summary>
        <span><Activity size={15} aria-hidden="true" /> Usage details</span>
        <ChevronDown className="details-chevron" size={15} aria-hidden="true" />
      </summary>
      <div className="usage-content">
        <div className="usage-stats">
          <div><span>Lifetime</span><strong>{formatCompactNumber(usage.lifetimeTokens)}</strong></div>
          <div><span>Current streak</span><strong>{streak(usage.currentStreakDays)}</strong></div>
          <div><span>Longest streak</span><strong>{streak(usage.longestStreakDays)}</strong></div>
          <div><span>Peak day</span><strong>{formatCompactNumber(usage.peakDailyTokens)}</strong></div>
        </div>
        {recent.length ? (
          // A label on a plain div is not exposed to assistive tech; as an
          // image the chart is read with a one-line summary.
          <div
            className="spark-bars"
            role="img"
            aria-label={`Recent daily token usage: ${formatCompactNumber(total)} tokens over the last ${recent.length} ${recent.length === 1 ? "day" : "days"}`}
          >
            {recent.map((day, index) => (
              <span key={`${day.startDate}-${index}`} title={`${day.startDate}: ${day.tokens.toLocaleString()} tokens`}>
                <i style={{ transform: `scaleY(${scale(day.tokens)})` }} />
              </span>
            ))}
          </div>
        ) : null}
      </div>
    </details>
  );
}
