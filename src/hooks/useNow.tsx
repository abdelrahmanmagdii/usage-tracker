import { createContext, useContext, useEffect, useState, type ReactNode } from "react";

const TICK_MS = 1_000;

/** The shared clock, or null when no NowProvider is mounted above. */
const NowContext = createContext<number | null>(null);

function useTicker(enabled: boolean): number {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (!enabled) return;
    setNow(Date.now());
    const interval = window.setInterval(() => setNow(Date.now()), TICK_MS);
    return () => window.clearInterval(interval);
  }, [enabled]);
  return now;
}

/**
 * One clock for the whole popover. Countdowns need a fresh `now` every
 * second, but the shell, the settings sheet and the setup guide do not: the
 * provider re-renders alone each tick and React skips its `children`, so
 * only components that call useNow (or sit inside <WithNow>) redraw.
 */
export function NowProvider({ children, value }: { children: ReactNode; value?: number }) {
  // A pinned `value` (tests, captures) stops the ticker instead of racing it.
  const ticking = useTicker(value === undefined);
  return <NowContext.Provider value={value ?? ticking}>{children}</NowContext.Provider>;
}

/** Wall-clock milliseconds, refreshed every second. Falls back to a private
 * ticker when rendered outside a NowProvider, e.g. in a component test. */
export function useNow(): number {
  const shared = useContext(NowContext);
  const local = useTicker(shared === null);
  return shared ?? local;
}

/** Hands `now` to a component that takes it as a prop, so the tick stops at
 * this boundary instead of re-rendering whatever renders the component. */
export function WithNow({ children }: { children: (now: number) => ReactNode }) {
  return <>{children(useNow())}</>;
}
