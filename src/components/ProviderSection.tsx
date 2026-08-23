import type { ReactNode } from "react";
import type { ProviderMeter } from "../hooks/useProviderMeter";
import { QuotaSection } from "./QuotaSection";
import { isRecord } from "../lib/rateLimits";
import { describeAge } from "../lib/time";
import type { CodexBackendState } from "../types/codex";

function planLabel(account: unknown): string | null {
  if (!isRecord(account) || typeof account.planType !== "string" || !account.planType) {
    return null;
  }
  return account.planType;
}

function unavailableNote(
  label: string,
  signedOutHint: string,
  state: CodexBackendState,
  showingUsage: boolean,
  nowMs: number,
): string {
  const expired = state.connection === "not_authenticated";
  if (!showingUsage) {
    return state.diagnostic ?? (expired ? `${label} is signed out.` : `${label} usage is unavailable right now.`);
  }
  const reason = expired ? signedOutHint : `${state.diagnostic ?? `${label} usage could not be refreshed`}.`;
  const age = describeAge(state.updatedAt, nowMs);
  return `Showing the last known usage — ${reason}${age ? ` Last updated ${age}.` : ""}`;
}

export function ProviderSection({
  id,
  label,
  icon,
  meter,
  now,
  signedOutHint,
}: {
  id: string;
  label: string;
  icon: ReactNode;
  meter: ProviderMeter;
  now: number;
  signedOutHint: string;
}) {
  const { state, buckets, refresh } = meter;
  if (state.connection === "cli_not_found" || state.connection === "starting") return null;

  const plan = planLabel(state.account);
  return (
    <section className="provider-block" data-provider={id} aria-labelledby={`${id}-heading`}>
      <div className="section-label provider-label">
        {icon}
        <span id={`${id}-heading`}>{label}</span>
        {plan ? <span className="provider-plan">{plan}</span> : null}
      </div>
      {buckets.length ? (
        <div className="quota-list provider-quota-list">
          {buckets.map((bucket) => (
            <QuotaSection key={bucket.id} bucket={bucket} now={now} />
          ))}
        </div>
      ) : null}
      {state.connection !== "connected" ? (
        <div className="provider-note glass-tile" role="status">
          <p>{unavailableNote(label, signedOutHint, state, buckets.length > 0, now)}</p>
          <button className="secondary-button" onClick={() => void refresh()}>
            Retry
          </button>
        </div>
      ) : null}
    </section>
  );
}
