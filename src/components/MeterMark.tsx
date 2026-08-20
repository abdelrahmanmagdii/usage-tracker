export function MeterMark({ compact = false }: { compact?: boolean }) {
  return (
    <span className={compact ? "meter-mark compact" : "meter-mark"} aria-hidden="true">
      <svg viewBox="0 0 32 32" focusable="false">
        {/* The UsageBar mark: two usage bars — the tall one Codex (white),
            the short one Claude (coral) — matching the menu-bar glyph so the
            popover badge and the tray read as one brand. */}
        <rect className="meter-mark__bar" x="9.75" y="9" width="5" height="14" rx="2.5" />
        <rect className="meter-mark__bar meter-mark__bar--accent" x="17.25" y="15" width="5" height="8" rx="2.5" />
      </svg>
    </span>
  );
}
