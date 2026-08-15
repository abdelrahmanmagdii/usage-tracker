import { Activity, ChevronDown } from "lucide-react";
import type { UsageSummary } from "../types/codex";
import { formatCompactNumber } from "../lib/rateLimits";

export function UsageDetails({ usage }: { usage: UsageSummary }) {
  const recent = usage.dailyUsage.slice(-7);
  const max = Math.max(...recent.map((day) => day.tokens), 1);
  return (
    <details className="usage-details">
      <summary>
        <span><Activity size={15} aria-hidden="true" /> Usage details</span>
        <ChevronDown className="details-chevron" size={15} aria-hidden="true" />
      </summary>
      <div className="usage-content">
        <div className="usage-stats">
          <div><span>Lifetime</span><strong>{formatCompactNumber(usage.lifetimeTokens)}</strong></div>
          <div><span>Current streak</span><strong>{usage.currentStreakDays ?? "—"}d</strong></div>
          <div><span>Longest streak</span><strong>{usage.longestStreakDays ?? "—"}d</strong></div>
          <div><span>Peak day</span><strong>{formatCompactNumber(usage.peakDailyTokens)}</strong></div>
        </div>
        {recent.length ? (
          <div className="spark-bars" aria-label="Recent daily token usage">
            {recent.map((day) => (
              <span key={day.startDate} title={`${day.startDate}: ${day.tokens.toLocaleString()} tokens`}>
                <i style={{ transform: `scaleY(${day.tokens / max})` }} />
              </span>
            ))}
          </div>
        ) : null}
      </div>
    </details>
  );
}
