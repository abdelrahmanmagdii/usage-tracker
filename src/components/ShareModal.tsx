import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import { writeImage } from "@tauri-apps/plugin-clipboard-manager";
import { Check, Copy, Download, Share, X } from "lucide-react";
import type { RateLimitBucket } from "../types/codex";
import { generateShareCard } from "../features/share-card/shareCard";
import {
  linkedInShareText,
  linkedInShareUrl,
  xIntentUrl,
  xShareText,
} from "../features/share-card/shareCopy";

function XMark({ size = 14 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">
      <path
        fill="currentColor"
        d="M18.244 2.25h3.308l-7.227 8.26 8.502 11.24H16.17l-4.714-6.231-5.401 6.231H2.744l7.727-8.835L1.254 2.25H8.08l4.259 5.672Zm-1.161 17.52h1.833L7.084 4.126H5.117Z"
      />
    </svg>
  );
}

function LinkedInMark({ size = 15 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">
      <path
        fill="currentColor"
        d="M4.98 3.5C4.98 4.88 3.88 6 2.5 6S0 4.88 0 3.5 1.12 1 2.5 1s2.48 1.12 2.48 2.5zM.24 8.25h4.52V23H.24zM8.23 8.25h4.33v2.01h.06c.6-1.14 2.08-2.34 4.28-2.34 4.58 0 5.42 3.01 5.42 6.93V23h-4.52v-7.07c0-1.69-.03-3.86-2.35-3.86-2.35 0-2.71 1.84-2.71 3.74V23H8.23z"
      />
    </svg>
  );
}

export function ShareModal({ bucket, onClose }: { bucket: RateLimitBucket; onClose: () => void }) {
  const [card, setCard] = useState<{ dataUrl: string; bytes: Uint8Array } | null>(null);
  const [message, setMessage] = useState("");
  const [network, setNetwork] = useState<"x" | "linkedin">("x");
  const captions = useMemo(
    () => ({ x: xShareText(bucket), linkedin: linkedInShareText(bucket) }),
    [bucket],
  );
  const [caption, setCaption] = useState(captions.x);

  useEffect(() => {
    let active = true;
    void generateShareCard(bucket)
      .then((next) => active && setCard({ dataUrl: next.dataUrl, bytes: next.bytes }))
      .catch((error) =>
        active && setMessage(error instanceof Error ? error.message : "Could not generate card"),
      );
    return () => {
      active = false;
    };
  }, [bucket]);

  useEffect(() => {
    setCaption(network === "x" ? captions.x : captions.linkedin);
  }, [captions, network]);

  const copyImage = async () => {
    if (!card) return;
    try {
      await writeImage(card.bytes);
      setMessage("Image copied — paste it onto the post");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Clipboard unavailable");
    }
  };

  const copyCaption = async () => {
    try {
      await navigator.clipboard.writeText(caption);
      setMessage("Caption copied");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Clipboard unavailable");
    }
  };

  const savePng = async () => {
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

  const postToX = async () => {
    await copyImage();
    try {
      await invoke("open_url", { url: xIntentUrl(caption) });
      setMessage("Image copied. Paste it onto the X post.");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Could not open X");
    }
  };

  const postToLinkedIn = async () => {
    await copyImage();
    try {
      await invoke("open_url", { url: linkedInShareUrl() });
      setMessage("Image copied. Paste it into the LinkedIn post, then Copy caption if you want the text.");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Could not open LinkedIn");
    }
  };

  const systemShare = async () => {
    if (!card) return;
    try {
      await invoke("present_share_sheet", { png: Array.from(card.bytes), caption });
      setMessage("Pick X, LinkedIn, or Messages in the share sheet");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Share sheet unavailable");
    }
  };

  return (
    <div
      className="modal-scrim"
      role="presentation"
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <section className="share-modal" role="dialog" aria-modal="true" aria-labelledby="share-heading">
        <header>
          <div>
            <span className="eyebrow">SHARE</span>
            <h2 id="share-heading">Post this snapshot</h2>
          </div>
          <button className="icon-button" onClick={onClose} aria-label="Close share card">
            <X size={17} />
          </button>
        </header>
        <div className="share-preview">
          {card ? <img src={card.dataUrl} alt="UsageBar share card preview" /> : <span className="spinner" />}
        </div>
        <p className="privacy-note">
          Quota only. No email, account ID, tokens, or file paths. The image is copied on this Mac; X
          and LinkedIn never receive it from us.
        </p>
        <div className="share-network" role="tablist" aria-label="Caption network">
          <button
            type="button"
            role="tab"
            aria-selected={network === "x"}
            className={network === "x" ? "is-active" : undefined}
            onClick={() => setNetwork("x")}
          >
            X caption
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={network === "linkedin"}
            className={network === "linkedin" ? "is-active" : undefined}
            onClick={() => setNetwork("linkedin")}
          >
            LinkedIn caption
          </button>
        </div>
        <label className="share-caption-label" htmlFor="share-caption">
          Caption
          {network === "x" ? <span>{caption.length}/280</span> : null}
        </label>
        <textarea
          id="share-caption"
          className="share-caption"
          rows={network === "x" ? 4 : 6}
          value={caption}
          onChange={(event) => setCaption(event.target.value)}
        />
        <div className="share-actions share-actions-social">
          <button className="share-x" onClick={() => void postToX()} disabled={!card}>
            <XMark /> Post on X
          </button>
          <button className="share-linkedin" onClick={() => void postToLinkedIn()} disabled={!card}>
            <LinkedInMark /> LinkedIn
          </button>
          <button className="secondary-button share-system" onClick={() => void systemShare()} disabled={!card}>
            <Share size={15} /> macOS share sheet
          </button>
        </div>
        <div className="share-actions">
          <button className="secondary-button" onClick={() => void copyImage()} disabled={!card}>
            <Copy size={16} /> Copy image
          </button>
          <button className="secondary-button" onClick={() => void copyCaption()}>
            <Copy size={16} /> Copy caption
          </button>
          <button className="primary-button share-save" onClick={() => void savePng()} disabled={!card}>
            <Download size={16} /> Save PNG
          </button>
        </div>
        {message ? (
          <p className="success-message" role="status">
            <Check size={14} /> {message}
          </p>
        ) : null}
      </section>
    </div>
  );
}
