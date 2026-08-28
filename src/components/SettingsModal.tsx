import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Check, Settings2, X } from "lucide-react";
import {
  AUTO_WINDOW,
  DEFAULT_PREFS,
  PROVIDER_CATALOG,
  isVisible,
  normalizePrefs,
  trayWindow,
  withTrayWindow,
  withVisible,
  type AppPrefs,
  type ProviderId,
} from "../lib/providers";

type TrayWindow = {
  id: string;
  label: string;
  usedPercent: number;
  durationMins?: number | null;
};

type TrayWindows = Record<ProviderId, TrayWindow[]>;

const PREVIEW_WINDOWS: TrayWindows = {
  codex: [
    { id: "codex:primary", label: "5-hour", usedPercent: 18 },
    { id: "codex:secondary", label: "Weekly", usedPercent: 36 },
  ],
  claude: [
    { id: "session:primary", label: "5-hour", usedPercent: 8 },
    { id: "weekly-all:secondary", label: "Weekly", usedPercent: 57 },
    { id: "weekly-scoped-fable:secondary", label: "Fable", usedPercent: 92 },
  ],
  cursor: [
    { id: "plan:primary", label: "Monthly", usedPercent: 41 },
    { id: "auto:secondary", label: "Auto", usedPercent: 12 },
  ],
  opencode: [
    { id: "rolling:primary", label: "5-hour", usedPercent: 18 },
    { id: "weekly:secondary", label: "Weekly", usedPercent: 27 },
    { id: "monthly:secondary", label: "Monthly", usedPercent: 11 },
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
  const options = [{ id: AUTO_WINDOW, label: "Most used", usedPercent: -1 }, ...windows];
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
                <span className="window-option-value">{Math.round(100 - option.usedPercent)}% left</span>
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
  const [windows, setWindows] = useState<TrayWindows>(() =>
    inTauri()
      ? { codex: [], claude: [], cursor: [], opencode: [] }
      : PREVIEW_WINDOWS,
  );
  const [autostart, setAutostart] = useState(false);

  useEffect(() => {
    if (!inTauri()) {
      setWindows(PREVIEW_WINDOWS);
      return;
    }
    void invoke<AppPrefs>("get_app_prefs").then((next) => setPrefs(normalizePrefs(next))).catch(() => undefined);
    void invoke<TrayWindows>("get_tray_windows")
      .then(setWindows)
      .catch(() => undefined);
    void invoke<boolean>("get_autostart").then(setAutostart).catch(() => undefined);
    const unlisten = listen<AppPrefs>("usagebar://prefs", (event) => setPrefs(normalizePrefs(event.payload)));
    return () => {
      void unlisten.then((off) => off());
    };
  }, []);

  const chooseWindow = useCallback((provider: ProviderId, window: string) => {
    setPrefs((current) => withTrayWindow(current, provider, window));
    if (inTauri()) void invoke("set_tray_window", { provider, window });
  }, []);

  const setToolVisible = useCallback((provider: ProviderId, visible: boolean) => {
    setPrefs((current) => withVisible(current, provider, visible));
    if (inTauri()) void invoke("set_provider_visible", { provider, visible });
  }, []);

  const visibleCount = PROVIDER_CATALOG.filter((tool) => isVisible(prefs, tool.id)).length;

  return (
    <div
      className="modal-scrim"
      role="presentation"
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <section
        className="sheet-modal settings-modal"
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
            Choose which tools to track. Hidden tools stay off the menu bar and
            are not polled. Each visible meter can follow a specific quota
            window, or whichever one you are closest to using up.
          </p>

          <div className="setting-group">
            <span className="setting-label">Tools</span>
            {PROVIDER_CATALOG.map((tool) => {
              const on = isVisible(prefs, tool.id);
              const found = windows[tool.id].length > 0;
              const detail = on
                ? found
                  ? "Shown in the popover and menu bar."
                  : "On — looking for a login on this Mac."
                : "Hidden. Turn on to track this tool.";
              return (
                <Toggle
                  key={tool.id}
                  label={tool.label}
                  detail={detail}
                  checked={on}
                  onChange={(next) => setToolVisible(tool.id, next)}
                />
              );
            })}
          </div>

          {PROVIDER_CATALOG.map((tool) =>
            isVisible(prefs, tool.id) ? (
              <WindowPicker
                key={`${tool.id}-windows`}
                title={`${tool.label} meter`}
                windows={windows[tool.id]}
                selected={trayWindow(prefs, tool.id)}
                onSelect={(id) => chooseWindow(tool.id, id)}
              />
            ) : null,
          )}

          <div className="setting-group">
            <span className="setting-label">Menu bar</span>
            <div className="layout-choice" role="radiogroup" aria-label="Menu bar layout">
              {[
                { value: true, name: "Compact", detail: "Every visible meter in one icon." },
                { value: false, name: "Extended", detail: "One icon per tool." },
              ].map((option) => {
                const active = prefs.combinedTray === option.value;
                return (
                  <button
                    key={option.name}
                    className={`layout-option${active ? " selected" : ""}`}
                    role="radio"
                    aria-checked={active}
                    onClick={() => {
                      setPrefs((current) => ({ ...current, combinedTray: option.value }));
                      if (inTauri()) void invoke("set_combined_tray", { enabled: option.value });
                    }}
                  >
                    <strong>{option.name}</strong>
                    <small>{option.detail}</small>
                    {active ? <Check size={14} aria-hidden="true" /> : null}
                  </button>
                );
              })}
            </div>
            <p className="setting-hint">
              {visibleCount > 2
                ? "With several tools on, Compact is much less likely to be hidden when the menu bar is crowded."
                : "Compact is less likely to be hidden when the menu bar is crowded."}
            </p>
          </div>

          <div className="setting-group">
            <span className="setting-label">General</span>
            <Toggle
              label="Usage alerts"
              detail="Get notified at 80% and 95%, and when a limit resets."
              checked={prefs.usageAlerts}
              onChange={(next) => {
                setPrefs((current) => ({ ...current, usageAlerts: next }));
                if (inTauri()) void invoke("set_usage_alerts", { enabled: next });
              }}
            />
            <Toggle
              label="Launch at login"
              detail="Start UsageBar when you log in."
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
