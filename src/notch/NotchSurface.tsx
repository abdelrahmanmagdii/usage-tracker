import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { AlertTriangle, Clock3 } from "lucide-react";
import { MeterMark } from "../components/MeterMark";
import { useResetEvents } from "../features/tibo-watch/useResetEvents";
import { useCodexMeter } from "../hooks/useCodexMeter";
import { formatLeadTime } from "../lib/time";
import { windowDurationLabel } from "../lib/rateLimits";
import type { RateLimitBucket, ResetEvent } from "../types/codex";
import "./notch.css";

type Presentation = {
  kind: "normal" | "low" | "reached" | "incoming" | "detected" | "stale" | "offline" | "loading";
  compact: string;
  title: string;
  detail: string;
};

function presentationFor(
  bucket: RateLimitBucket | undefined,
  events: ResetEvent[],
  connection: string,
  updatedAt: number | null | undefined,
  now: number,
): Presentation {
  const incoming = events
    .filter((event) => event.occursAt && Date.parse(event.occursAt) > now)
    .sort((left, right) => Date.parse(left.occursAt!) - Date.parse(right.occursAt!))[0];
  if (incoming?.occursAt) {
    const lead = formatLeadTime(Date.parse(incoming.occursAt) - now);
    const source = incoming.source === "openai" ? "OpenAI report" : "community report";
    return { kind: "incoming", compact: `Bonus reset · ~${lead}`, title: "Bonus reset announced", detail: `Expected in about ${lead} · ${source}` };
  }
  const detected = events.find((event) => event.source === "detected" && now - Date.parse(event.announcedAt) < 2 * 3_600_000);
  if (detected) {
    return { kind: "detected", compact: "Quota refreshed", title: "Possible bonus reset detected", detail: "Available quota increased ahead of schedule." };
  }
  if (!bucket) {
    return connection === "starting"
      ? { kind: "loading", compact: "Connecting…", title: "Connecting to Codex", detail: "Usage will appear when the local app server responds." }
      : { kind: "offline", compact: "Usage unavailable", title: "Codex is offline", detail: "Open UsageBar to reconnect." };
  }
  const updatedMs = updatedAt ? (updatedAt < 1_000_000_000_000 ? updatedAt * 1000 : updatedAt) : 0;
  const ageMs = updatedMs ? Math.max(0, now - updatedMs) : Number.POSITIVE_INFINITY;
  const used = Math.round(bucket.usedPercent);
  if (connection !== "connected") {
    const age = Number.isFinite(ageMs) ? formatLeadTime(ageMs) : "unknown";
    return ageMs >= 30 * 60_000
      ? { kind: "offline", compact: "Usage unavailable", title: "Codex is offline", detail: `Last updated ${age} ago.` }
      : { kind: "offline", compact: `${used}% · offline`, title: "Last known Codex usage", detail: `Updated ${age} ago.` };
  }
  if (ageMs > 10 * 60_000) {
    const age = formatLeadTime(ageMs);
    return { kind: "stale", compact: `${used}% · updated ${age} ago`, title: "Codex usage may be stale", detail: `Last updated ${age} ago.` };
  }
  const resetMs = bucket.resetsAt ? bucket.resetsAt * 1000 : 0;
  const lead = resetMs > now ? formatLeadTime(resetMs - now) : "soon";
  if (bucket.reached || bucket.remainingPercent <= 0) {
    return { kind: "reached", compact: `Limit reached · ${lead}`, title: "Codex limit reached", detail: `Renews in ${lead}.` };
  }
  return {
    kind: bucket.remainingPercent <= 20 ? "low" : "normal",
    compact: `${used}% · ${lead}`,
    title: `${windowDurationLabel(bucket.windowDurationMins)} · ${used}% used`,
    detail: resetMs ? `Renews in ${lead}.` : "Renewal time unavailable.",
  };
}

export function NotchSurface() {
  const { state, buckets } = useCodexMeter({ observeHistory: false });
  const events = useResetEvents({ owner: false });
  const [now, setNow] = useState(Date.now());
  const [expanded, setExpanded] = useState(false);
  const collapseTimer = useRef<number | undefined>(undefined);

  useEffect(() => {
    let interval = 0;
    const delay = 60_000 - (Date.now() % 60_000);
    const timeout = window.setTimeout(() => {
      setNow(Date.now());
      interval = window.setInterval(() => setNow(Date.now()), 60_000);
    }, delay);
    return () => {
      window.clearTimeout(timeout);
      window.clearInterval(interval);
    };
  }, []);

  const bucket = useMemo(
    () => buckets.reduce<RateLimitBucket | undefined>((lowest, item) => !lowest || item.remainingPercent < lowest.remainingPercent ? item : lowest, undefined),
    [buckets],
  );
  const presentation = presentationFor(bucket, events, state.connection, state.updatedAt, now);
  const used = bucket?.usedPercent ?? 0;
  const label = bucket
    ? `Codex usage, ${Math.round(used)} percent of the ${windowDurationLabel(bucket.windowDurationMins)} window used. ${presentation.detail}`
    : presentation.title;

  const setNativeExpanded = (next: boolean) => {
    setExpanded(next);
    if ("__TAURI_INTERNALS__" in window) void invoke("set_notch_expanded", { expanded: next });
  };
  const open = () => {
    if (collapseTimer.current) window.clearTimeout(collapseTimer.current);
    setNativeExpanded(true);
  };
  const closeSoon = () => {
    collapseTimer.current = window.setTimeout(() => setNativeExpanded(false), 260);
  };

  return (
    <main className={`notch-surface is-${presentation.kind}${expanded ? " is-expanded" : ""}`}>
      <button
        className="notch-button"
        aria-label={label}
        aria-expanded={expanded}
        onMouseEnter={open}
        onMouseLeave={closeSoon}
        onFocus={open}
        onBlur={closeSoon}
        onClick={() => void invoke("show_main_window")}
      >
        <span className="notch-compact">
          <MeterMark />
          <span className="notch-copy">
            <strong>{presentation.compact}</strong>
            <span className="notch-track" aria-hidden="true"><i style={{ transform: `scaleX(${used / 100})` }} /></span>
          </span>
          {presentation.kind === "incoming" || presentation.kind === "reached" ? <AlertTriangle size={14} aria-hidden="true" /> : <Clock3 size={13} aria-hidden="true" />}
        </span>
        <span className="notch-expanded-copy" role={presentation.kind === "incoming" || presentation.kind === "detected" ? "status" : undefined}>
          <strong>{presentation.title}</strong>
          <span>{presentation.detail}</span>
        </span>
      </button>
    </main>
  );
}
