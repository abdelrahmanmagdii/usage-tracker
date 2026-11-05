/**
 * Simulated wall clock. The app reads time only through `Date` (`Date.now()`,
 * `new Date()`), so swapping the global `Date` for a subclass whose "now"
 * comes from this clock lets the unmodified app run at any speed, paused, or
 * jumped forward. Timers (`setInterval`) stay real: the app's one-second tick
 * simply observes `speed` simulated seconds per tick.
 */

const RealDate = Date;

export class SimClock {
  private baseSim: number; // ms
  private baseReal: number; // performance.now() at the last rebase
  private rate: number;
  private listeners = new Set<() => void>();

  constructor(startMs: number, speed: number) {
    this.baseSim = startMs;
    this.baseReal = performance.now();
    this.rate = Math.max(0, speed);
  }

  /** Simulated epoch milliseconds. */
  now(): number {
    return Math.floor(this.baseSim + (performance.now() - this.baseReal) * this.rate);
  }

  get speed(): number {
    return this.rate;
  }

  setSpeed(speed: number): void {
    this.rebase();
    this.rate = Math.max(0, Number.isFinite(speed) ? speed : 1);
    this.changed();
  }

  /** Jump forward (or back) by `ms` simulated milliseconds. */
  advance(ms: number): void {
    this.rebase();
    this.baseSim += ms;
    this.changed();
  }

  set(ms: number): void {
    this.rebase();
    this.baseSim = ms;
    this.changed();
  }

  onChange(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private rebase(): void {
    const real = performance.now();
    this.baseSim += (real - this.baseReal) * this.rate;
    this.baseReal = real;
  }

  private changed(): void {
    for (const listener of this.listeners) listener();
  }
}

/** Replace the global `Date` so `Date.now()` / `new Date()` follow `clock`. */
export function installClock(clock: SimClock): void {
  class SimDate extends RealDate {
    constructor(...args: unknown[]) {
      if (args.length === 0) super(clock.now());
      else super(...(args as [string | number | Date]));
    }

    static now(): number {
      return clock.now();
    }
  }
  Object.defineProperty(SimDate, "name", { value: "Date" });
  (globalThis as { Date: DateConstructor }).Date = SimDate as unknown as DateConstructor;
}

/** The real wall clock, for the harness's own bookkeeping. */
export const realNow = (): number => RealDate.now();

/**
 * Parses durations like `90`, `90s`, `15m`, `2h`, `3d`, `1h30m` into
 * milliseconds. Bare numbers are seconds.
 */
export function parseDuration(value: string | number): number {
  if (typeof value === "number") return value * 1000;
  const text = value.trim();
  if (/^-?\d+(\.\d+)?$/.test(text)) return Number(text) * 1000;
  const units: Record<string, number> = { ms: 1, s: 1_000, m: 60_000, h: 3_600_000, d: 86_400_000, w: 604_800_000 };
  let total = 0;
  let matched = false;
  const sign = text.startsWith("-") ? -1 : 1;
  for (const match of text.replace(/^[+-]/, "").matchAll(/(\d+(?:\.\d+)?)(ms|s|m|h|d|w)/g)) {
    total += Number(match[1]) * units[match[2]];
    matched = true;
  }
  if (!matched) throw new Error(`Unrecognised duration: ${value}`);
  return sign * total;
}

/** Parses `start=` values: ISO strings or epoch milliseconds. */
export function parseStart(value: string | null): number | null {
  if (!value) return null;
  if (/^\d+$/.test(value)) return Number(value);
  const parsed = RealDate.parse(value);
  return Number.isFinite(parsed) ? parsed : null;
}
