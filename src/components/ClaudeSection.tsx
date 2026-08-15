import { Sparkles } from "lucide-react";
import type { useClaudeMeter } from "../hooks/useClaudeMeter";
import { QuotaSection } from "./QuotaSection";
import { isRecord } from "../lib/rateLimits";

const PLAN_LABELS: Record<string, string> = {
  max: "Max",
  pro: "Pro",
  team: "Team",
  enterprise: "Enterprise",
};

function planLabel(account: unknown): string | null {
  if (!isRecord(account) || typeof account.planType !== "string") return null;
  return PLAN_LABELS[account.planType] ?? account.planType;
}

export function ClaudeSection({
  meter,
  now,
}: {
  meter: ReturnType<typeof useClaudeMeter>;
  now: number;
}) {
  const { state, buckets, refresh } = meter;
  // Machines without a Claude Code login skip the section entirely.
  if (state.connection === "cli_not_found" || state.connection === "starting") return null;

  const plan = planLabel(state.account);
  return (
    <section className="claude-block" aria-labelledby="claude-heading">
      <div className="section-label claude-label">
        <Sparkles size={14} aria-hidden="true" />
        <span id="claude-heading">Claude Code</span>
        {plan ? <span className="claude-plan">{plan}</span> : null}
      </div>
      {buckets.length && (state.connection === "connected" || state.connection === "not_authenticated") ? (
        <div className="quota-list claude-quota-list">
          {buckets.map((bucket) => (
            <QuotaSection key={bucket.id} bucket={bucket} now={now} />
          ))}
        </div>
      ) : null}
      {state.connection !== "connected" ? (
        <div className="claude-note glass-tile" role="status">
          <p>
            {state.connection === "not_authenticated"
              ? buckets.length
                ? "Showing the last known usage — the Claude Code session has expired. It recovers on its own after any Claude Code use."
                : state.diagnostic ?? "Claude Code is signed out."
              : state.diagnostic ?? "Claude usage is unavailable right now."}
          </p>
          <button className="secondary-button" onClick={() => void refresh()}>
            Retry
          </button>
        </div>
      ) : null}
    </section>
  );
}
