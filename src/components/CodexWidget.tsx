import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { ArrowUpRight, CornerDownRight, GripVertical, Minus, RefreshCw, X } from "lucide-react";
import { useCodexMeter } from "../hooks/useCodexMeter";
import {
  useAntigravityMeter,
  useClaudeMeter,
  useCursorMeter,
  useDevinMeter,
  useGeminiMeter,
  useOpenCodeMeter,
} from "../hooks/useClaudeMeter";
import { formatCountdown, formatLocalReset, windowDurationLabel } from "../lib/rateLimits";
import { freshness } from "../lib/time";
import { previewPrefs } from "../lib/preview";
import { DEFAULT_PREFS, type AppPrefs, type ProviderId, isVisible, normalizePrefs } from "../lib/providers";
import type { CodexBackendState, RateLimitBucket } from "../types/codex";
import "./codex-widget.css";

function quotaTone(bucket?: RateLimitBucket) {
  if (!bucket) return "normal";
  const remaining = Math.round(bucket.remainingPercent);
  if (bucket.reached || remaining < 20) return "danger";
  if (remaining < 40) return "warning";
  return "normal";
}

function worstTone(a?: RateLimitBucket, b?: RateLimitBucket) {
  const toneA = quotaTone(a);
  const toneB = quotaTone(b);
  if (toneA === "danger" || toneB === "danger") return "danger";
  if (toneA === "warning" || toneB === "warning") return "warning";
  return "normal";
}

/**
 * The two windows a card shows: the 5-hour one (else the first that is not
 * the weekly one) and the weekly one. Falling back to "the first bucket" for
 * the 5-hour slot showed a weekly-only plan's weekly window twice, once
 * under a 5h label.
 */
export function pickWindows(buckets: RateLimitBucket[]): {
  fiveHour?: RateLimitBucket;
  weekly?: RateLimitBucket;
} {
  const weekly = buckets.find((b) => b.windowDurationMins === 10_080);
  const fiveHour = buckets.find((b) => b.windowDurationMins === 300) ?? buckets.find((b) => b !== weekly);
  return { fiveHour, weekly };
}

/**
 * Compact tag from the window's own length ("5h", "1d", "30d"), so a
 * Cursor month or a Devin day is not labelled 5h. `fallback` covers a
 * window that did not report its length.
 */
export function windowTag(bucket: RateLimitBucket | undefined, fallback: string): string {
  const mins = bucket?.windowDurationMins;
  if (!mins || !Number.isFinite(mins) || mins <= 0) return fallback;
  if (mins % 1_440 === 0) return `${mins / 1_440}d`;
  if (mins % 60 === 0) return `${mins / 60}h`;
  return `${mins}m`;
}

/** Spoken window name ("5-hour", "Weekly", "Daily") for meter labels. */
function windowName(bucket: RateLimitBucket | undefined, fallback: string): string {
  const mins = bucket?.windowDurationMins;
  return mins && Number.isFinite(mins) && mins > 0 ? windowDurationLabel(mins) : fallback;
}

/** One status vocabulary for every provider, including stale readings. */
function connectionStatus(
  state: CodexBackendState,
  readingStale: boolean,
  signIn: string,
  missing: string,
): string {
  switch (state.connection) {
    case "connected":
      return readingStale ? "Stale reading" : "Live quota";
    case "not_authenticated":
      return signIn;
    case "cli_not_found":
      return missing;
    case "starting":
      return "Connecting…";
    default:
      return "Reconnecting…";
  }
}

function planLabel(account: unknown): string | null {
  if (!account || typeof account !== "object" || !("planType" in account)) {
    return null;
  }
  return typeof account.planType === "string" ? account.planType : null;
}

function QuotaRow({
  label,
  bucket,
  now,
  stale,
}: {
  label: string;
  bucket?: RateLimitBucket;
  now: number;
  stale: boolean;
}) {
  const remaining = bucket ? Math.round(bucket.remainingPercent) : undefined;
  const countdown = bucket ? formatCountdown(bucket.resetsAt, now) : "Quota unavailable";
  const resetClock = bucket?.resetsAt ? formatLocalReset(bucket.resetsAt, new Date(now)) : "";
  const tone = quotaTone(bucket);
  return (
    <div className={`widget-quota quota-${tone}`}>
      <div className="widget-quota-header">
        <div className="widget-quota-label-group">
          <span className="widget-window">{label}</span>
          <strong className="widget-percent">
            {remaining === undefined ? "—" : `${stale ? "~" : ""}${remaining}%`}
          </strong>
          <span className="widget-subtext">left</span>
        </div>
        <div className="widget-reset">
          <span className="widget-reset-countdown">{countdown}</span>
          {resetClock && <small className="widget-reset-time">{resetClock}</small>}
        </div>
      </div>
      {bucket ? (
        <span
          className="widget-quota-track"
          role="meter"
          aria-label={`${label} quota remaining`}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={bucket.remainingPercent}
          aria-valuetext={`${remaining}% remaining${stale ? ", last known reading" : ""}`}
        >
          <i
            style={{
              width: `${Math.min(100, Math.max(0, bucket.remainingPercent))}%`,
            }}
          />
        </span>
      ) : (
        <span className="widget-quota-track" aria-hidden="true" />
      )}
    </div>
  );
}

export interface WidgetModelItem {
  id: string;
  provider: ProviderId;
  name: string;
  shortLabel: string;
  accent?: string;
  fiveHourBucket?: RateLimitBucket;
  weeklyBucket?: RateLimitBucket;
  stale: boolean;
  status: string;
  plan: string | null;
  state: CodexBackendState;
  refresh: () => Promise<void>;
}

export function CodexWidget() {
  const codex = useCodexMeter({ observeHistory: false });
  const antigravity = useAntigravityMeter();
  const claude = useClaudeMeter();
  const cursor = useCursorMeter();
  const opencode = useOpenCodeMeter();
  const devin = useDevinMeter();
  const gemini = useGeminiMeter();

  const [prefs, setPrefs] = useState<AppPrefs>(
    () => previewPrefs(window.location.search) ?? DEFAULT_PREFS,
  );
  const [now, setNow] = useState(Date.now());
  const [expanded, setExpanded] = useState(false);

  const collapseTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const expandTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const dragging = useRef(false);
  const expandedRef = useRef(false);

  useEffect(() => {
    if (
      !("__TAURI_INTERNALS__" in window) ||
      typeof (window as unknown as { __TAURI_INTERNALS__?: { transformCallback?: unknown } })
        .__TAURI_INTERNALS__?.transformCallback !== "function"
    ) {
      return;
    }
    let active = true;
    void invoke<AppPrefs>("get_app_prefs")
      .then((next) => active && setPrefs(normalizePrefs(next)))
      .catch(() => undefined);
    const unlisten = listen<AppPrefs>("usagebar://prefs", (event) => {
      if (active) setPrefs(normalizePrefs(event.payload));
    });
    return () => {
      active = false;
      void unlisten.then((off) => off());
    };
  }, []);

  const items = useMemo<WidgetModelItem[]>(() => {
    const list: WidgetModelItem[] = [];

    // 1. Antigravity: split into Gemini & 3P (Claude/GPT) if present
    const showAntigravity = isVisible(prefs, "antigravity");
    if (
      showAntigravity &&
      (antigravity.state.connection === "connected" || antigravity.buckets.length > 0)
    ) {
      const age = freshness(antigravity.state.updatedAt, now);
      const stale = antigravity.state.connection !== "connected" || age?.stale === true;
      const status =
        antigravity.state.connection === "connected"
          ? age?.stale
            ? "Stale reading"
            : "Live quota"
          : antigravity.state.connection === "not_authenticated"
            ? "Sign in to Antigravity"
            : antigravity.state.connection === "cli_not_found"
              ? "Antigravity missing"
              : antigravity.state.connection === "starting"
                ? "Connecting…"
                : "Reconnecting…";
      const plan = planLabel(antigravity.state.account);

      const argonBuckets = antigravity.buckets.filter(
        (b) => b.id.includes("argon") || b.limitName?.toLowerCase().includes("argon"),
      );
      const geminiBuckets = antigravity.buckets.filter(
        (b) =>
          !b.id.includes("argon") &&
          !b.limitName?.toLowerCase().includes("argon") &&
          (b.id.includes("gemini") || b.limitName?.toLowerCase().includes("gemini")),
      );
      const threePBuckets = antigravity.buckets.filter(
        (b) =>
          b.id.includes("3p") ||
          b.limitName?.toLowerCase().includes("claude") ||
          b.limitName?.toLowerCase().includes("gpt"),
      );

      if (geminiBuckets.length > 0) {
        const fiveHour = geminiBuckets.find(
          (b) => b.windowDurationMins === 300 || b.limitId.includes("5h"),
        );
        const weekly = geminiBuckets.find(
          (b) => b.windowDurationMins === 10_080 || b.limitId.includes("weekly"),
        );
        list.push({
          id: "antigravity-gemini",
          provider: "antigravity",
          name: "Gemini",
          shortLabel: "Gemini",
          fiveHourBucket: fiveHour ?? (weekly ? undefined : geminiBuckets[0]),
          weeklyBucket: weekly,
          stale,
          status,
          plan,
          state: antigravity.state,
          refresh: antigravity.refresh,
        });
      }

      if (argonBuckets.length > 0) {
        const fiveHour = argonBuckets.find(
          (b) => b.windowDurationMins === 300 || b.limitId.includes("5h"),
        );
        const weekly = argonBuckets.find(
          (b) => b.windowDurationMins === 10_080 || b.limitId.includes("weekly"),
        );
        list.push({
          id: "antigravity-argon",
          provider: "antigravity",
          name: "Gemini 4 Argon",
          shortLabel: "Argon 4",
          fiveHourBucket: fiveHour ?? (weekly ? undefined : argonBuckets[0]),
          weeklyBucket: weekly,
          stale,
          status,
          plan,
          state: antigravity.state,
          refresh: antigravity.refresh,
        });
      }

      if (threePBuckets.length > 0) {
        const fiveHour = threePBuckets.find(
          (b) => b.windowDurationMins === 300 || b.limitId.includes("5h"),
        );
        const weekly = threePBuckets.find(
          (b) => b.windowDurationMins === 10_080 || b.limitId.includes("weekly"),
        );
        list.push({
          id: "antigravity-3p",
          provider: "antigravity",
          name: "Claude & GPT",
          shortLabel: "Claude",
          fiveHourBucket: fiveHour ?? (weekly ? undefined : threePBuckets[0]),
          weeklyBucket: weekly,
          stale,
          status,
          plan,
          state: antigravity.state,
          refresh: antigravity.refresh,
        });
      }

      if (
        geminiBuckets.length === 0 &&
        argonBuckets.length === 0 &&
        threePBuckets.length === 0 &&
        antigravity.buckets.length > 0
      ) {
        list.push({
          id: "antigravity",
          provider: "antigravity",
          name: "Antigravity",
          shortLabel: "Antigravity",
          fiveHourBucket:
            antigravity.buckets.find((b) => b.windowDurationMins === 300) ?? antigravity.buckets[0],
          weeklyBucket: antigravity.buckets.find((b) => b.windowDurationMins === 10_080),
          stale,
          status,
          plan,
          state: antigravity.state,
          refresh: antigravity.refresh,
        });
      }
    }

    // 2-6. Single-card providers, in widget order.
    const simple: Array<{
      provider: Exclude<ProviderId, "antigravity">;
      name: string;
      shortLabel: string;
      signIn: string;
      missing: string;
      meter: { state: CodexBackendState; buckets: RateLimitBucket[]; refresh: () => Promise<void> };
      buckets: RateLimitBucket[];
    }> = [
      {
        provider: "codex",
        name: "Codex",
        shortLabel: "Codex",
        signIn: "Sign in to Codex",
        missing: "Codex CLI missing",
        meter: codex,
        // Model-scoped Codex limits ride along in the payload; the card
        // follows the subscription limit when there is one.
        buckets: codex.buckets.some((b) => b.limitId === "codex")
          ? codex.buckets.filter((b) => b.limitId === "codex")
          : codex.buckets,
      },
      {
        provider: "claude",
        name: "Claude Code",
        shortLabel: "Claude",
        signIn: "Sign in to Claude",
        missing: "Claude CLI missing",
        meter: claude,
        buckets: claude.buckets,
      },
      {
        provider: "opencode",
        name: "OpenCode Go",
        shortLabel: "OpenCode",
        signIn: "Sign in to OpenCode",
        missing: "OpenCode missing",
        meter: opencode,
        buckets: opencode.buckets,
      },
      {
        provider: "cursor",
        name: "Cursor",
        shortLabel: "Cursor",
        signIn: "Sign in to Cursor",
        missing: "Cursor missing",
        meter: cursor,
        buckets: cursor.buckets,
      },
      {
        provider: "devin",
        name: "Devin",
        shortLabel: "Devin",
        signIn: "Sign in to Devin",
        missing: "Devin missing",
        meter: devin,
        buckets: devin.buckets,
      },
      // Gemini CLI arrived after the widget was written and never got a card,
      // so a Windows user signed in only to Gemini saw a placeholder.
      {
        provider: "gemini",
        name: "Gemini CLI",
        shortLabel: "Gemini",
        signIn: "Sign in to Gemini CLI",
        missing: "Gemini CLI missing",
        meter: gemini,
        buckets: gemini.buckets,
      },
    ];

    const cardFor = (entry: (typeof simple)[number]): WidgetModelItem => {
      const { state } = entry.meter;
      const age = freshness(state.updatedAt, now);
      const { fiveHour, weekly } = pickWindows(entry.buckets);
      return {
        id: entry.provider,
        provider: entry.provider,
        name: entry.name,
        shortLabel: entry.shortLabel,
        fiveHourBucket: fiveHour,
        weeklyBucket: weekly,
        stale: state.connection !== "connected" || age?.stale === true,
        // OpenCode, Cursor and Devin used to say "Live quota" over a stale
        // reading and "Connecting…" when signed out.
        status: connectionStatus(state, age?.stale === true, entry.signIn, entry.missing),
        plan: planLabel(state.account),
        state,
        refresh: entry.meter.refresh,
      };
    };

    for (const entry of simple) {
      if (!isVisible(prefs, entry.provider)) continue;
      if (entry.meter.state.connection !== "connected" && entry.meter.buckets.length === 0) continue;
      list.push(cardFor(entry));
    }

    // Nothing to show yet: a placeholder card for the first tool the user
    // keeps visible, so it says what that tool is waiting on. It used to
    // always be Codex, even with Codex hidden.
    if (list.length === 0) {
      const placeholder = simple.find((entry) => isVisible(prefs, entry.provider)) ?? simple[0];
      list.push(cardFor(placeholder));
    }

    return list;
  }, [
    prefs,
    codex.state,
    codex.buckets,
    codex.refresh,
    antigravity.state,
    antigravity.buckets,
    antigravity.refresh,
    claude.state,
    claude.buckets,
    claude.refresh,
    cursor.state,
    cursor.buckets,
    cursor.refresh,
    opencode.state,
    opencode.buckets,
    opencode.refresh,
    devin.state,
    devin.buckets,
    devin.refresh,
    gemini.state,
    gemini.buckets,
    gemini.refresh,
    now,
  ]);

  const anyRefreshing =
    codex.refreshing ||
    antigravity.refreshing ||
    claude.refreshing ||
    cursor.refreshing ||
    opencode.refreshing ||
    devin.refreshing ||
    gemini.refreshing;

  const refreshAll = async () => {
    // Antigravity fills up to three cards from one meter; refresh it once,
    // not once per card.
    const refreshers = new Set(items.map((it) => it.refresh));
    await Promise.allSettled([...refreshers].map((refresh) => refresh()));
  };

function formatCompactTime(resetsAt?: number, nowMs = Date.now()): string {
  if (!resetsAt) return "";
  const seconds = Math.max(0, Math.floor(resetsAt - nowMs / 1000));
  if (seconds <= 0) return "due";
  const days = Math.floor(seconds / 86_400);
  const hours = Math.floor((seconds % 86_400) / 3_600);
  const minutes = Math.floor((seconds % 3_600) / 60);
  const secs = seconds % 60;
  if (days > 0) return `${days}d ${hours}h`;
  if (hours > 0) return `${hours}h ${minutes}m`;
  if (minutes > 0) return `${minutes}m ${String(secs).padStart(2, "0")}s`;
  return `${secs}s`;
}

  const brandRef = useRef<HTMLButtonElement>(null);
  const quotasRef = useRef<HTMLDivElement>(null);
  const [measuredCollapsedWidth, setMeasuredCollapsedWidth] = useState<number | null>(null);

  const estimatedCollapsedWidth = useMemo(() => {
    const count = items.length;
    if (count <= 1) return 188;
    return Math.min(640, 34 + count * 106 + 18);
  }, [items.length]);

  useEffect(() => {
    if (expanded) return;
    const measure = () => {
      if (!quotasRef.current) return;
      const brandW = brandRef.current ? brandRef.current.getBoundingClientRect().width : 32;
      const chipElements = Array.from(quotasRef.current.children) as HTMLElement[];
      const chipsW = chipElements.reduce((acc, el) => acc + el.getBoundingClientRect().width + 4, 0);
      const total = Math.ceil(brandW + chipsW + 14);
      if (total > 0 && Math.abs(total - (measuredCollapsedWidth ?? 0)) > 2) {
        setMeasuredCollapsedWidth(total);
      }
    };

    measure();
    if (typeof ResizeObserver !== "undefined" && quotasRef.current) {
      const ro = new ResizeObserver(() => measure());
      ro.observe(quotasRef.current);
      return () => ro.disconnect();
    }
  }, [expanded, items, now, measuredCollapsedWidth]);

  const collapsedWidth = measuredCollapsedWidth ?? estimatedCollapsedWidth;

  const expandedWidth = useMemo(() => {
    return Math.max(320, Math.min(380, collapsedWidth));
  }, [collapsedWidth]);

  const expandedHeight = useMemo(() => {
    const cardHeights = items.reduce((acc, it) => {
      const hasFiveHourRow = Boolean(it.fiveHourBucket || !it.weeklyBucket);
      const hasWeeklyRow = Boolean(it.weeklyBucket || items.length === 1);
      const rows = (hasFiveHourRow ? 1 : 0) + (hasWeeklyRow ? 1 : 0);
      return acc + (items.length > 1 ? 24 : 0) + rows * 42 + 10;
    }, 0);
    return Math.min(460, Math.max(130, 36 + cardHeights + 8));
  }, [items]);

  useEffect(() => {
    if (!expandedRef.current && "__TAURI_INTERNALS__" in window) {
      void invoke("set_widget_expanded", {
        expanded: false,
        width: collapsedWidth,
        height: 26.0,
      }).catch(() => undefined);
    }
  }, [collapsedWidth]);

  const changeExpandedRef = useRef<(next: boolean) => void>(() => {});
  const changeExpanded = (next: boolean) => {
    clearTimeout(collapseTimer.current);
    clearTimeout(expandTimer.current);
    if (expandedRef.current === next) return;
    expandedRef.current = next;
    setExpanded(next);

    if ("__TAURI_INTERNALS__" in window) {
      void invoke("set_widget_expanded", {
        expanded: next,
        width: next ? expandedWidth : collapsedWidth,
        height: next ? expandedHeight : 26.0,
      }).catch(() => undefined);
    }
  };
  changeExpandedRef.current = changeExpanded;

  // The expanded size was only sent at the moment of expanding. A tool that
  // connects (or drops out) while the panel is open changes how many cards
  // it holds, and the window kept its old height, clipping the new card.
  useEffect(() => {
    if (!expandedRef.current || !("__TAURI_INTERNALS__" in window)) return;
    void invoke("set_widget_expanded", {
      expanded: true,
      width: expandedWidth,
      height: expandedHeight,
    }).catch(() => undefined);
  }, [expandedWidth, expandedHeight]);

  useEffect(() => {
    if (
      !("__TAURI_INTERNALS__" in window) ||
      typeof (window as unknown as { __TAURI_INTERNALS__?: { transformCallback?: unknown } })
        .__TAURI_INTERNALS__?.transformCallback !== "function"
    ) {
      return;
    }
    const unlisten = listen<{ action: string }>("usagebar://demo", (event) => {
      if (event.payload.action === "widget-expand") changeExpandedRef.current(true);
      if (event.payload.action === "widget-collapse") changeExpandedRef.current(false);
    });
    return () => {
      void unlisten.then((off) => off());
    };
  }, []);

  useEffect(
    () => () => {
      clearTimeout(collapseTimer.current);
      clearTimeout(expandTimer.current);
    },
    [],
  );

  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, []);

  const hasAnyStale = items.some((it) => it.stale);
  const overallStatus =
    items.length === 1
      ? items[0].status
      : items.every((it) => it.state.connection === "connected")
        ? hasAnyStale
          ? "Stale reading"
          : "Live quota"
        : `${items.filter((it) => it.state.connection === "connected").length}/${items.length} live`;

  const openDashboard = () => {
    if ("__TAURI_INTERNALS__" in window) void invoke("show_dashboard");
  };

  return (
    <main
      className={`codex-widget${expanded ? "" : " widget-collapsed"}`}
      aria-label="Codex quota widget"
      onClick={() => {
        if (!expandedRef.current) {
          changeExpanded(true);
        }
      }}
      onMouseEnter={() => {
        clearTimeout(collapseTimer.current);
        clearTimeout(expandTimer.current);
        if (!expandedRef.current) {
          expandTimer.current = setTimeout(() => changeExpanded(true), 180);
        }
      }}
      onMouseLeave={(event) => {
        clearTimeout(expandTimer.current);
        if (dragging.current || (event.buttons & 1) !== 0) return;
        collapseTimer.current = setTimeout(() => changeExpanded(false), 220);
      }}
      onFocusCapture={() => changeExpanded(true)}
      onBlurCapture={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) {
          changeExpanded(false);
        }
      }}
    >
      {!expanded ? (
        <>
          <button
            ref={brandRef}
            className="widget-compact-brand"
            title={`${overallStatus} · Open UsageBar dashboard`}
            onClick={(e) => {
              e.stopPropagation();
              openDashboard();
            }}
          >
            <span className={`widget-dot${hasAnyStale ? " widget-dot-stale" : ""}`} />
            {items.length === 1 ? items[0].shortLabel : "AI"}
          </button>
          <div className="widget-compact-quotas" ref={quotasRef}>
            {items.map((item) => {
              const bucket = item.fiveHourBucket ?? item.weeklyBucket;
              const slotIsWeekly = !(item.fiveHourBucket || !item.weeklyBucket);
              const windowLabel = windowTag(bucket, slotIsWeekly ? "7d" : "5h");
              const spokenWindow = windowName(bucket, slotIsWeekly ? "Weekly" : "5-hour");
              const remaining = bucket ? Math.round(bucket.remainingPercent) : undefined;
              const tone = quotaTone(bucket);
              const countdown = bucket?.resetsAt ? formatCountdown(bucket.resetsAt, now) : "";
              const tooltip = bucket?.resetsAt
                ? `${item.name} · ${windowLabel}: ${remaining}% remaining · ${countdown} (${formatLocalReset(bucket.resetsAt, new Date(now))})`
                : `${item.name}: ${item.status}`;

              return (
                <div
                  key={item.id}
                  className={`widget-compact-chip widget-compact-row quota-${tone}`}
                  title={tooltip}
                >
                  {items.length > 1 && (
                    <span className="widget-model-name">
                      {item.shortLabel}
                    </span>
                  )}
                  {items.length === 1 && (
                    <span className="widget-window-tag">{windowLabel}</span>
                  )}
                  <strong className="widget-model-percent">
                    {remaining === undefined ? "—" : `${item.stale ? "~" : ""}${remaining}%`}
                  </strong>
                  {bucket?.resetsAt ? (
                    <time className="widget-compact-time">
                      {formatCompactTime(bucket.resetsAt, now)}
                    </time>
                  ) : null}
                  {bucket && (
                    <span
                      className="widget-chip-track"
                      role="meter"
                      aria-label={
                        items.length === 1
                          ? `${spokenWindow} quota remaining`
                          : `${item.shortLabel} ${spokenWindow.toLowerCase()} quota remaining`
                      }
                      aria-valuemin={0}
                      aria-valuemax={100}
                      aria-valuenow={bucket.remainingPercent}
                      aria-valuetext={`${remaining}% remaining${item.stale ? ", last known reading" : ""}`}
                    >
                      <i
                        style={{
                          width: `${bucket.remainingPercent}%`,
                        }}
                      />
                    </span>
                  )}
                </div>
              );
            })}
          </div>
        </>
      ) : (
        <>
          <header className="widget-header">
            <span
              className="widget-drag"
              title="Drag to move meter"
              aria-label="Drag to move meter"
              onMouseDown={(event) => {
                if (event.button !== 0 || !("__TAURI_INTERNALS__" in window)) return;
                clearTimeout(collapseTimer.current);
                dragging.current = true;
                void invoke("drag_codex_widget")
                  .finally(() => {
                    dragging.current = false;
                  })
                  .catch(() => undefined);
              }}
            >
              <GripVertical size={13} aria-hidden="true" />
            </span>
            <button className="widget-dashboard" title="Open UsageBar dashboard" onClick={openDashboard}>
              <span className={`widget-dot${hasAnyStale ? " widget-dot-stale" : ""}`} />
              <strong>{items.length === 1 ? items[0].name : "AI Usage"}</strong>
              <ArrowUpRight size={13} aria-hidden="true" />
            </button>
            <span
              className={`widget-status${hasAnyStale ? " widget-status-stale" : ""}`}
              title={overallStatus}
            >
              {overallStatus}
            </span>
            <span className="widget-actions">
              <button
                className={`widget-refresh${anyRefreshing ? " widget-refreshing" : ""}`}
                aria-label={
                  items.length === 1 && items[0].id === "codex"
                    ? "Refresh Codex quota"
                    : "Refresh quota"
                }
                title="Refresh quotas"
                disabled={anyRefreshing}
                onClick={() => void refreshAll()}
              >
                <RefreshCw size={12} aria-hidden="true" />
              </button>
              <button
                className="widget-refresh"
                aria-label="Restore meter to bottom-right"
                title="Restore to bottom-right"
                onClick={() => {
                  if ("__TAURI_INTERNALS__" in window) void invoke("reset_widget_position");
                }}
              >
                <CornerDownRight size={13} aria-hidden="true" />
              </button>
              <button
                className="widget-refresh"
                aria-label={
                  items.length === 1 && items[0].id === "codex"
                    ? "Minimize Codex meter"
                    : "Minimize meter"
                }
                title="Minimize meter — restore from tray menu"
                onClick={() => {
                  changeExpanded(false);
                  if ("__TAURI_INTERNALS__" in window) void invoke("hide_codex_widget");
                }}
              >
                <Minus size={13} aria-hidden="true" />
              </button>
              <button
                className="widget-refresh widget-close"
                aria-label="Quit UsageBar"
                title="Close UsageBar"
                onClick={() => {
                  if ("__TAURI_INTERNALS__" in window) void invoke("quit_app");
                }}
              >
                <X size={13} aria-hidden="true" />
              </button>
            </span>
          </header>

          <div className="widget-expanded-scroll">
            {items.map((item) => {
              const itemTone = worstTone(item.fiveHourBucket, item.weeklyBucket);
              return (
                <section
                  key={item.id}
                  className={`widget-model-card quota-${itemTone}`}
                  data-provider={item.provider}
                >
                  {items.length > 1 && (
                    <div className="widget-model-card-header">
                      <span className="widget-card-title">
                        {item.name}
                      </span>
                      {item.plan && <span className="widget-card-plan">{item.plan}</span>}
                      <span
                        className={`widget-card-status${item.stale ? " widget-card-status-stale" : ""}`}
                      >
                        {item.status}
                      </span>
                    </div>
                  )}
                  {(item.fiveHourBucket || !item.weeklyBucket) && (
                    <QuotaRow
                      label={windowTag(item.fiveHourBucket, "5h")}
                      bucket={item.fiveHourBucket}
                      now={now}
                      stale={item.stale}
                    />
                  )}
                  {(item.weeklyBucket || items.length === 1) && (
                    <QuotaRow
                      label="Weekly"
                      bucket={item.weeklyBucket}
                      now={now}
                      stale={item.stale}
                    />
                  )}
                </section>
              );
            })}
          </div>
        </>
      )}
    </main>
  );
}
