import { Sparkles } from "lucide-react";
import type { useClaudeMeter } from "../hooks/useClaudeMeter";
import { QuotaSection } from "./QuotaSection";
import { isRecord } from "../lib/rateLimits";
import { describeAge } from "../lib/time";
import type { CodexBackendState } from "../types/codex";

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

/**
 * What the note says while the section is not connected. Tiles keep showing the
 * last known usage through expiries and read failures, so the note carries the
 * whole warning: why refreshing stopped, and how old the numbers on screen are.
 */
function unavailableNote(state: CodexBackendState, showingUsage: boolean, nowMs: number): string {
  const expired = state.connection === "not_authenticated";
  if (!showingUsage) {
    return state.diagnostic ?? (expired ? "Claude Code is signed out." : "Claude usage is unavailable right now.");
  }
  const reason = expired
    ? "the Claude Code session has expired. It recovers on its own after any Claude Code use."
    : `${state.diagnostic ?? "Claude usage could not be refreshed"}.`;
  const age = describeAge(state.updatedAt, nowMs);
  return `Showing the last known usage — ${reason}${age ? ` Last updated ${age}.` : ""}`;
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
      {/* Last-known usage stays on screen through expiries and read failures;
          the note below is what tells the user it is not current. */}
      {buckets.length ? (
        <div className="quota-list claude-quota-list">
          {buckets.map((bucket) => (
            <QuotaSection key={bucket.id} bucket={bucket} now={now} />
          ))}
        </div>
      ) : null}
      {state.connection !== "connected" ? (
        <div className="claude-note glass-tile" role="status">
          <p>{unavailableNote(state, buckets.length > 0, now)}</p>
          <button className="secondary-button" onClick={() => void refresh()}>
            Retry
          </button>
        </div>
      ) : null}
    </section>
  );
}
