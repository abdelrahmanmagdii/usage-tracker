import {
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
  type PointerEvent,
} from "react";
import {
  describeTimeline,
  evenPaceAt,
  formatChartTime,
  readoutAt,
  timelineStops,
  timeTicks,
  type QuotaTimeline,
  type Readout,
} from "../lib/quotaTimeline";
import "./quota-chart.css";

// Roomy tiles get a readable plot plus an axis row; dense rows get a slim
// strip that still carries the same marks and interaction.
const PLOT_HEIGHT = { roomy: 46, dense: 22 };
// Horizontal room for the end dot and its ring, so "now" near the reset or
// the first reading at the window start never clips.
const PAD_X = 6;
const PAD_Y = 5;
const FALLBACK_WIDTH = 320;
// The pointer snaps to a reading (or the run-out point) within this many
// pixels, so steps are easy to land on without pixel hunting.
const SNAP_PX = 8;

function readoutText(readout: Readout, spanMs: number): { value: string; detail: string } {
  const time = formatChartTime(readout.t, spanMs, true);
  const even = readout.evenPace === null ? null : `even pace ${Math.round(readout.evenPace)}%`;
  if (readout.remaining === null) {
    const label = readout.kind === "unrecorded" ? "Not recorded" : "Not yet";
    return { value: label, detail: [even, time].filter(Boolean).join(" · ") };
  }
  const value = readout.kind === "runout" ? "Runs out" : `${Math.round(readout.remaining)}% left`;
  const prefix = readout.kind === "now" ? "now" : readout.kind === "recorded" ? null : "projected";
  return { value, detail: [prefix, even, time].filter(Boolean).join(" · ") };
}

/**
 * Percent remaining across the current window, on a true time axis from the
 * window's start to its reset. A muted diagonal shows "even pace" (using the
 * quota evenly until the reset), so a line above it has room to spare and a
 * line below it is burning fast. When the recent pace runs out before the
 * reset, a dashed projection runs down to the run-out point, matching the
 * pace text under the chart.
 */
export function QuotaChart({ timeline, dense = false }: { timeline: QuotaTimeline; dense?: boolean }) {
  const frameRef = useRef<HTMLDivElement>(null);
  const tipRef = useRef<HTMLDivElement>(null);
  // Set between a press and the focus it causes, so only keyboard focus
  // opens the readout on "now" (a click already shows it under the pointer).
  const pressing = useRef(false);
  const [width, setWidth] = useState(FALLBACK_WIDTH);
  const [active, setActive] = useState<{ t: number; via: "pointer" | "keyboard" } | null>(null);
  const hintId = useId();

  // The tile's width depends on the window (popover vs. dashboard), and SVG
  // text and dots must not stretch, so the plot is laid out in real pixels.
  useLayoutEffect(() => {
    const frame = frameRef.current;
    if (!frame) return;
    const measure = () => {
      if (frame.clientWidth > 0) setWidth(frame.clientWidth);
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(frame);
    return () => observer.disconnect();
  }, []);

  const height = dense ? PLOT_HEIGHT.dense : PLOT_HEIGHT.roomy;
  const { startMs, endMs, nowMs, points, runOutMs } = timeline;
  const spanMs = endMs - startMs;
  const x = (t: number) => PAD_X + ((t - startMs) / spanMs) * (width - PAD_X * 2);
  const y = (remaining: number) => PAD_Y + (1 - remaining / 100) * (height - PAD_Y * 2);
  const toTime = (px: number) =>
    Math.min(endMs, Math.max(startMs, startMs + ((px - PAD_X) / (width - PAD_X * 2)) * spanMs));

  const first = points[0];
  let line = `M${x(first.t).toFixed(1)} ${y(first.remaining).toFixed(1)}`;
  // Step-after: a reading holds until the next one, then changes straight
  // up or down. A sloped line would invent usage between samples.
  for (const point of points.slice(1)) {
    line += `H${x(point.t).toFixed(1)}V${y(point.remaining).toFixed(1)}`;
  }
  const area = points.length > 1 ? `${line}V${y(0).toFixed(1)}H${x(first.t).toFixed(1)}Z` : null;

  const ticks = useMemo(() => (dense ? [] : timeTicks(startMs, endMs)), [dense, startMs, endMs]);
  const summary = useMemo(() => describeTimeline(timeline), [timeline]);
  const stops = useMemo(() => timelineStops(timeline), [timeline]);
  const live = points[points.length - 1];
  const even0 = evenPaceAt(timeline, startMs);
  const readout = active ? readoutAt(timeline, active.t) : null;
  const text = readout ? readoutText(readout, spanMs) : null;

  // Keep the tooltip inside the chart: centred on the crosshair, but pinned
  // to an edge near either end of the window.
  useLayoutEffect(() => {
    const tip = tipRef.current;
    if (!tip || !readout) return;
    const half = tip.offsetWidth / 2;
    const left = Math.min(Math.max(x(readout.t), half), Math.max(half, width - half));
    tip.style.left = `${left}px`;
  });

  const onPointerMove = (event: PointerEvent<SVGSVGElement>) => {
    const rect = event.currentTarget.getBoundingClientRect();
    const px = event.clientX - rect.left;
    const nearest = stops.reduce<number | null>(
      (best, stop) => (Math.abs(x(stop) - px) <= SNAP_PX && (best === null || Math.abs(x(stop) - px) < Math.abs(x(best) - px)) ? stop : best),
      null,
    );
    setActive({ t: nearest ?? toTime(px), via: "pointer" });
  };

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    // Arrows move to the neighbouring stop from wherever the readout is, even
    // a spot the pointer left between two readings.
    const last = stops.length - 1;
    let index: number | null = null;
    if (event.key === "ArrowLeft") {
      const before = active ? stops.filter((stop) => stop < active.t).length - 1 : last;
      index = Math.max(0, before);
    } else if (event.key === "ArrowRight") {
      const after = active ? stops.findIndex((stop) => stop > active.t) : last;
      index = after < 0 ? last : after;
    } else if (event.key === "Home") index = 0;
    else if (event.key === "End") index = last;
    else if (event.key === "Escape" && active) {
      event.preventDefault();
      setActive(null);
      return;
    }
    if (index === null) return;
    event.preventDefault();
    setActive({ t: stops[index], via: "keyboard" });
  };

  const crossX = readout ? x(readout.t) : 0;
  return (
    <div
      ref={frameRef}
      className={`quota-chart${dense ? " quota-chart--dense" : ""}`}
      role="group"
      aria-roledescription="chart"
      aria-label={`Usage over this window. ${summary}`}
      aria-describedby={hintId}
      tabIndex={0}
      onKeyDown={onKeyDown}
      onPointerDown={() => {
        pressing.current = true;
      }}
      onPointerUp={() => {
        pressing.current = false;
      }}
      onFocus={() => {
        if (!pressing.current) setActive((current) => current ?? { t: nowMs, via: "keyboard" });
        pressing.current = false;
      }}
      onBlur={() => setActive(null)}
    >
      <svg
        className="quota-chart-plot"
        width={width}
        height={height}
        viewBox={`0 0 ${width} ${height}`}
        aria-hidden="true"
        onPointerMove={onPointerMove}
        onPointerLeave={() => setActive((current) => (current?.via === "pointer" ? null : current))}
      >
        {ticks.map((tick) => (
          <line key={tick} className="quota-chart-tick" x1={x(tick)} x2={x(tick)} y1={PAD_Y} y2={y(0)} />
        ))}
        <line className="quota-chart-baseline" x1={PAD_X} x2={width - PAD_X} y1={y(0)} y2={y(0)} />
        {even0 !== null ? (
          <line className="quota-chart-even" x1={x(startMs)} y1={y(even0)} x2={x(endMs)} y2={y(0)} />
        ) : null}
        <line className="quota-chart-now" x1={x(nowMs)} x2={x(nowMs)} y1={PAD_Y} y2={y(0)} />
        {area ? <path className="quota-chart-area" d={area} /> : null}
        {points.length > 1 ? <path className="quota-chart-line" d={line} /> : null}
        {runOutMs !== null ? (
          <>
            <line className="quota-chart-projection" x1={x(nowMs)} y1={y(live.remaining)} x2={x(runOutMs)} y2={y(0)} />
            <circle className="quota-chart-runout" cx={x(runOutMs)} cy={y(0)} r={dense ? 2.5 : 3} />
          </>
        ) : null}
        <circle className="quota-chart-dot" cx={x(nowMs)} cy={y(live.remaining)} r={dense ? 3 : 4} />
        {readout ? (
          <g className="quota-chart-cursor">
            <line x1={crossX} x2={crossX} y1={0} y2={height} />
            {readout.evenPace !== null ? (
              <circle className="quota-chart-cursor-even" cx={crossX} cy={y(readout.evenPace)} r={2.5} />
            ) : null}
            {readout.remaining !== null ? (
              <circle className="quota-chart-cursor-value" cx={crossX} cy={y(readout.remaining)} r={dense ? 3 : 3.5} />
            ) : null}
          </g>
        ) : null}
      </svg>
      {text ? (
        <div ref={tipRef} className="quota-chart-tip" aria-hidden="true">
          <strong>{text.value}</strong>
          <span>{text.detail}</span>
        </div>
      ) : null}
      {!dense ? (
        <div className="quota-chart-axis" aria-hidden="true">
          <span>{formatChartTime(startMs, spanMs)}</span>
          {even0 !== null ? (
            <span className="quota-chart-key">
              <svg width="12" height="8" viewBox="0 0 12 8"><line x1="1" y1="1" x2="11" y2="7" /></svg>
              even pace
            </span>
          ) : null}
          <span>Reset</span>
        </div>
      ) : null}
      <span id={hintId} className="quota-chart-sr">Use the arrow keys to step through readings.</span>
      <span className="quota-chart-sr" aria-live="polite">
        {readout && active?.via === "keyboard" && text ? `${text.value}, ${text.detail}` : ""}
      </span>
    </div>
  );
}
