import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import { writeImage } from "@tauri-apps/plugin-clipboard-manager";
import { Check, Copy, Download, X } from "lucide-react";
import type { RateLimitBucket } from "../types/codex";
import { generateShareCard } from "../features/share-card/shareCard";

export function ShareModal({ bucket, onClose }: { bucket: RateLimitBucket; onClose: () => void }) {
  const [card, setCard] = useState<{ dataUrl: string; bytes: Uint8Array } | null>(null);
  const [message, setMessage] = useState("");
  useEffect(() => {
    let active = true;
    void generateShareCard(bucket)
      .then((next) => active && setCard(next))
      .catch((error) => active && setMessage(error instanceof Error ? error.message : "Could not generate card"));
    return () => {
      active = false;
    };
  }, [bucket]);

  const copy = async () => {
    if (!card) return;
    try {
      await writeImage(card.bytes);
      setMessage("Copied to clipboard");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Clipboard unavailable");
    }
  };
  const download = async () => {
    if (!card) return;
    const filePath = await save({
      defaultPath: "usagebar.png",
      filters: [{ name: "PNG image", extensions: ["png"] }],
    });
    if (!filePath) return;
    try {
      await invoke("write_share_card", { path: filePath, bytes: Array.from(card.bytes) });
      setMessage("Saved PNG");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Could not save PNG");
    }
  };

  return (
    <div className="modal-scrim" role="presentation" onMouseDown={(event) => event.target === event.currentTarget && onClose()}>
      <section className="share-modal" role="dialog" aria-modal="true" aria-labelledby="share-heading">
        <header>
          <div><span className="eyebrow">EXPORT</span><h2 id="share-heading">Share snapshot</h2></div>
          <button className="icon-button" onClick={onClose} aria-label="Close share card"><X size={17} /></button>
        </header>
        <div className="share-preview">
          {card ? <img src={card.dataUrl} alt="UsageBar share card preview" /> : <span className="spinner" />}
        </div>
        <p className="privacy-note">Quota only. No email, account ID, tokens, or file paths.</p>
        <div className="share-actions">
          <button className="secondary-button" onClick={copy} disabled={!card}><Copy size={16} /> Copy image</button>
          <button className="primary-button" onClick={download} disabled={!card}><Download size={16} /> Save PNG</button>
        </div>
        {message ? <p className="success-message" role="status"><Check size={14} /> {message}</p> : null}
      </section>
    </div>
  );
}
