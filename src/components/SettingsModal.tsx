import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Check, Settings2, X } from "lucide-react";

type TrayWindow = {
  id: string;
  label: string;
  usedPercent: number;
  durationMins?: number | null;
};

type AppPrefs = {
  compactTray: boolean;
  usageAlerts: boolean;
  codexTrayWindow: string;
  claudeTrayWindow: string;
};

const AUTO = "auto";
const DEFAULT_PREFS: AppPrefs = {
  compactTray: false,
  usageAlerts: true,
  codexTrayWindow: AUTO,
  claudeTrayWindow: AUTO,
};

const PREVIEW_WINDOWS: Record<"codex" | "claude", TrayWindow[]> = {
  codex: [
    { id: "codex:primary", label: "5-hour", usedPercent: 18 },
    { id: "codex:secondary", label: "Weekly", usedPercent: 36 },
  ],
  claude: [
    { id: "session:primary", label: "5-hour", usedPercent: 8 },
    { id: "weekly-all:secondary", label: "Weekly", usedPercent: 57 },
    { id: "weekly-scoped-fable:secondary", label: "Fable", usedPercent: 92 },
  ],
};

function inTauri(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

function WindowPicker({
  title,
  windows,
  selected,
  onSelect,
}: {
  title: string;
  windows: TrayWindow[];
  selected: string;
  onSelect: (id: string) => void;
}) {
  if (windows.length === 0) return null;
  const options = [{ id: AUTO, label: "Most used", usedPercent: -1 }, ...windows];
  return (
    <div className="setting-group">
      <span className="setting-label">{title}</span>
      <div className="window-picker" role="radiogroup" aria-label={`${title} menu-bar window`}>
        {options.map((option) => {
          const active = option.id === selected;
          return (
            <button
              key={option.id}
              className={`window-option${active ? " selected" : ""}`}
              role="radio"
              aria-checked={active}
              onClick={() => onSelect(option.id)}
            >
              <span className="window-option-name">{option.label}</span>
              {option.usedPercent >= 0 ? (
                <span className="window-option-value">{Math.round(option.usedPercent)}%</span>
              ) : (
                <span className="window-option-value muted">auto</span>
              )}
              {active ? <Check size={14} aria-hidden="true" /> : null}
            </button>
          );
        })}
      </div>
    </div>
  );
}

function Toggle({
  label,
  detail,
  checked,
  onChange,
}: {
  label: string;
  detail: string;
  checked: boolean;
  onChange: (next: boolean) => void;
}) {
  return (
    <button
      className={`setting-toggle${checked ? " is-on" : ""}`}
      role="switch"
      aria-checked={checked}
      onClick={() => onChange(!checked)}
    >
      <span>
        <strong>{label}</strong>
        <small>{detail}</small>
      </span>
      <span className="switch-track" aria-hidden="true">
        <i />
      </span>
    </button>
  );
}

export function SettingsModal({
  onClose,
  onShowGuide,
}: {
  onClose: () => void;
  onShowGuide: () => void;
}) {
  const [prefs, setPrefs] = useState<AppPrefs>(DEFAULT_PREFS);
  const [windows, setWindows] = useState<Record<"codex" | "claude", TrayWindow[]>>({
    codex: [],
    claude: [],
  });
  const [autostart, setAutostart] = useState(false);

  useEffect(() => {
    if (!inTauri()) {
      setWindows(PREVIEW_WINDOWS);
      return;
    }
    void invoke<AppPrefs>("get_app_prefs").then(setPrefs).catch(() => undefined);
    void invoke<Record<"codex" | "claude", TrayWindow[]>>("get_tray_windows")
      .then(setWindows)
      .catch(() => undefined);
    void invoke<boolean>("get_autostart").then(setAutostart).catch(() => undefined);
  }, []);

  const chooseWindow = useCallback((provider: "codex" | "claude", window: string) => {
    setPrefs((current) => ({
      ...current,
      [provider === "codex" ? "codexTrayWindow" : "claudeTrayWindow"]: window,
    }));
    if (inTauri()) void invoke("set_tray_window", { provider, window });
  }, []);

  return (
    <div
      className="modal-scrim"
      role="presentation"
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <section
        className="share-modal settings-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="settings-title"
      >
        <header>
          <div>
            <span className="eyebrow">PREFERENCES</span>
            <h2 id="settings-title">
              <Settings2 size={15} aria-hidden="true" /> Settings
            </h2>
          </div>
          <button className="icon-button" onClick={onClose} aria-label="Close settings">
            <X size={17} />
          </button>
        </header>

        <div className="settings-body">
          <p className="settings-intro">
            Choose which quota window each menu-bar meter follows. “Most used”
            always tracks whichever window is closest to its limit.
          </p>
          <WindowPicker
            title="Codex meter"
            windows={windows.codex}
            selected={prefs.codexTrayWindow}
            onSelect={(id) => chooseWindow("codex", id)}
          />
          <WindowPicker
            title="Claude Code meter"
            windows={windows.claude}
            selected={prefs.claudeTrayWindow}
            onSelect={(id) => chooseWindow("claude", id)}
          />

          <div className="setting-group">
            <span className="setting-label">General</span>
            <Toggle
              label="Compact meter"
              detail="Percentage only, no countdown."
              checked={prefs.compactTray}
              onChange={(next) => {
                setPrefs((current) => ({ ...current, compactTray: next }));
                if (inTauri()) void invoke("set_compact_tray", { enabled: next });
              }}
            />
            <Toggle
              label="Usage alerts"
              detail="Notify at 80% and 95% used, and on a fresh window."
              checked={prefs.usageAlerts}
              onChange={(next) => {
                setPrefs((current) => ({ ...current, usageAlerts: next }));
                if (inTauri()) void invoke("set_usage_alerts", { enabled: next });
              }}
            />
            <Toggle
              label="Launch at login"
              detail="Start UsageBar automatically."
              checked={autostart}
              onChange={(next) => {
                setAutostart(next);
                if (inTauri()) {
                  void invoke("set_autostart", { enabled: next }).catch(() => setAutostart(!next));
                }
              }}
            />
          </div>

          <button className="secondary-button settings-guide" onClick={onShowGuide}>
            Open setup guide
          </button>
        </div>
      </section>
    </div>
  );
}
