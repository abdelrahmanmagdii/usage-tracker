import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Check, PanelTop, X } from "lucide-react";

type NotchMode = "off" | "automatic" | "always";
type NotchStatus = { mode: NotchMode; notchAvailable: boolean; visible: boolean };

const options: Array<{ mode: NotchMode; title: string; detail: string }> = [
  { mode: "automatic", title: "Automatic", detail: "Show only on a display with a camera notch." },
  { mode: "always", title: "Always", detail: "Use a top-center capsule on every Mac display." },
  { mode: "off", title: "Off", detail: "Keep UsageBar in the menu bar only." },
];

export function NotchSettings({ onClose }: { onClose: () => void }) {
  const [status, setStatus] = useState<NotchStatus>({ mode: "off", notchAvailable: false, visible: false });
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if ("__TAURI_INTERNALS__" in window) {
      void invoke<NotchStatus>("get_notch_status").then(setStatus).catch(() => undefined);
    }
  }, []);

  const choose = async (mode: NotchMode) => {
    setStatus((current) => ({ ...current, mode }));
    if (!("__TAURI_INTERNALS__" in window)) return;
    setSaving(true);
    try {
      await invoke("set_notch_mode", { mode });
      window.setTimeout(() => {
        void invoke<NotchStatus>("get_notch_status").then(setStatus).catch(() => undefined);
      }, 120);
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="modal-scrim" role="presentation" onMouseDown={(event) => event.target === event.currentTarget && onClose()}>
      <section className="share-modal notch-settings" role="dialog" aria-modal="true" aria-labelledby="notch-settings-title">
        <header>
          <div>
            <span className="eyebrow">DESKTOP COMPANION</span>
            <h2 id="notch-settings-title"><PanelTop size={15} aria-hidden="true" /> Notch meter</h2>
          </div>
          <button className="icon-button" onClick={onClose} aria-label="Close notch settings"><X size={17} /></button>
        </header>
        <p className="notch-settings-intro">Keep your tightest Codex quota visible without opening the meter.</p>
        <div className="notch-mode-list" role="radiogroup" aria-label="Notch meter mode">
          {options.map((option) => (
            <button
              key={option.mode}
              className={`notch-mode-option${status.mode === option.mode ? " selected" : ""}`}
              role="radio"
              aria-checked={status.mode === option.mode}
              disabled={saving}
              onClick={() => void choose(option.mode)}
            >
              <span><strong>{option.title}</strong><small>{option.detail}</small></span>
              {status.mode === option.mode ? <Check size={16} aria-hidden="true" /> : null}
            </button>
          ))}
        </div>
        <p className="notch-availability">
          {status.notchAvailable ? "A compatible notch display is connected." : "No notch detected. “Always” uses the fallback capsule."}
        </p>
      </section>
    </div>
  );
}
