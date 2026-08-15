export function MeterMark({ compact = false }: { compact?: boolean }) {
  return (
    <span className={compact ? "meter-mark compact" : "meter-mark"} aria-hidden="true">
      <svg viewBox="0 0 32 32" focusable="false">
        <path
          className="meter-mark__cloud"
          d="M9.2 24.7a6.6 6.6 0 0 1-5.4-10.4 6.8 6.8 0 0 1 6.4-8.6 8.3 8.3 0 0 1 13.7 2.8 6.6 6.6 0 0 1 2.2 12.8 7.7 7.7 0 0 1-11.3 4.4 7.3 7.3 0 0 1-5.6-1Z"
        />
        <path className="meter-mark__terminal" d="m10.2 12.2 3.1 3.7-3.1 3.9M16.7 19.6h5" />
      </svg>
    </span>
  );
}
