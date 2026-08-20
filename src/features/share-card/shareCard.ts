import type { RateLimitBucket } from "../../types/codex";
import { formatCountdown, windowDurationLabel } from "../../lib/rateLimits";

const WIDTH = 1200;
const HEIGHT = 675;

function traceRoundedRect(
  context: CanvasRenderingContext2D,
  x: number,
  y: number,
  width: number,
  height: number,
  radius: number,
) {
  const r = Math.min(radius, width / 2, height / 2);
  context.beginPath();
  context.moveTo(x + r, y);
  context.arcTo(x + width, y, x + width, y + height, r);
  context.arcTo(x + width, y + height, x, y + height, r);
  context.arcTo(x, y + height, x, y, r);
  context.arcTo(x, y, x + width, y, r);
  context.closePath();
}

function quipFor(remaining: number): string {
  if (remaining <= 5) return "Effectively out of quota.";
  if (remaining <= 35) return "Running low.";
  if (remaining >= 90) return "Freshly reset.";
  return "Holding steady.";
}

export async function generateShareCard(bucket: RateLimitBucket): Promise<{
  dataUrl: string;
  bytes: Uint8Array;
}> {
  const canvas = document.createElement("canvas");
  canvas.width = WIDTH;
  canvas.height = HEIGHT;
  const context = canvas.getContext("2d");
  if (!context) throw new Error("Canvas is unavailable");

  const remaining = Math.round(bucket.remainingPercent);
  const accent = remaining <= 10 ? "#ff453a" : remaining <= 35 ? "#ff9f0a" : "#30d158";

  // Ambient backdrop
  const backdrop = context.createLinearGradient(0, 0, 0, HEIGHT);
  backdrop.addColorStop(0, "#0d0d12");
  backdrop.addColorStop(1, "#08080b");
  context.fillStyle = backdrop;
  context.fillRect(0, 0, WIDTH, HEIGHT);

  const glow = context.createRadialGradient(920, 90, 40, 920, 90, 560);
  glow.addColorStop(0, `${accent}2e`);
  glow.addColorStop(1, `${accent}00`);
  context.fillStyle = glow;
  context.fillRect(0, 0, WIDTH, HEIGHT);

  const ember = context.createRadialGradient(140, 620, 20, 140, 620, 420);
  ember.addColorStop(0, "rgba(120, 160, 255, 0.10)");
  ember.addColorStop(1, "rgba(120, 160, 255, 0)");
  context.fillStyle = ember;
  context.fillRect(0, 0, WIDTH, HEIGHT);

  // Frosted glass panel
  traceRoundedRect(context, 56, 52, WIDTH - 112, HEIGHT - 104, 44);
  context.fillStyle = "rgba(255, 255, 255, 0.055)";
  context.fill();
  context.strokeStyle = "rgba(255, 255, 255, 0.13)";
  context.lineWidth = 1.5;
  context.stroke();
  context.save();
  traceRoundedRect(context, 56, 52, WIDTH - 112, HEIGHT - 104, 44);
  context.clip();
  const sheen = context.createLinearGradient(0, 52, 0, 220);
  sheen.addColorStop(0, "rgba(255, 255, 255, 0.075)");
  sheen.addColorStop(1, "rgba(255, 255, 255, 0)");
  context.fillStyle = sheen;
  context.fillRect(56, 52, WIDTH - 112, 168);
  context.restore();

  const left = 128;

  // Brand lockup: the two-bar UsageBar mark in its purple badge, sitting left
  // of the wordmark. Same geometry as the popover badge and the menu-bar glyph.
  const badgeX = 68;
  const badgeY = 100;
  traceRoundedRect(context, badgeX, badgeY, 44, 44, 14);
  const badgeFill = context.createLinearGradient(badgeX, badgeY, badgeX + 44, badgeY + 44);
  badgeFill.addColorStop(0, "#7e67f6");
  badgeFill.addColorStop(1, "#4e68f4");
  context.fillStyle = badgeFill;
  context.fill();
  context.strokeStyle = "rgba(255, 255, 255, 0.35)";
  context.lineWidth = 1.5;
  context.stroke();
  context.fillStyle = "rgba(255, 255, 255, 0.97)";
  traceRoundedRect(context, badgeX + 12, badgeY + 12, 8, 20, 4);
  context.fill();
  context.fillStyle = "#e8a081";
  traceRoundedRect(context, badgeX + 24, badgeY + 20, 8, 12, 4);
  context.fill();

  // Wordmark
  context.fillStyle = "rgba(245, 245, 247, 0.55)";
  context.font = "600 23px ui-monospace, SFMono-Regular, Menlo, monospace";
  context.fillText("USAGEBAR", left, 130);

  // Big number with a soft glow
  context.save();
  context.shadowColor = `${accent}73`;
  context.shadowBlur = 64;
  context.fillStyle = accent;
  context.font = "700 188px -apple-system, BlinkMacSystemFont, sans-serif";
  context.fillText(`${remaining}%`, left - 8, 330);
  context.restore();
  context.fillStyle = "#f5f5f7";
  context.font = "650 32px -apple-system, BlinkMacSystemFont, sans-serif";
  context.fillText(`${windowDurationLabel(bucket.windowDurationMins).toUpperCase()} QUOTA LEFT`, left, 386);

  // Glass capsule progress bar
  const barWidth = WIDTH - 256;
  traceRoundedRect(context, left, 428, barWidth, 22, 11);
  context.fillStyle = "rgba(255, 255, 255, 0.09)";
  context.fill();
  const fillWidth = Math.max(22, barWidth * (bucket.remainingPercent / 100));
  traceRoundedRect(context, left, 428, fillWidth, 22, 11);
  const fillGradient = context.createLinearGradient(0, 428, 0, 450);
  fillGradient.addColorStop(0, `${accent}f2`);
  fillGradient.addColorStop(1, accent);
  context.fillStyle = fillGradient;
  context.fill();

  // Countdown + status line
  context.fillStyle = "rgba(235, 235, 245, 0.66)";
  context.font = "500 26px -apple-system, BlinkMacSystemFont, sans-serif";
  context.fillText(formatCountdown(bucket.resetsAt).replace("Resets", "resets"), left, 512);
  context.fillStyle = "#f5f5f7";
  context.font = "550 24px -apple-system, BlinkMacSystemFont, sans-serif";
  context.fillText(quipFor(remaining), left, 566);

  context.textAlign = "right";
  context.fillStyle = "rgba(235, 235, 245, 0.42)";
  context.font = "500 19px ui-monospace, SFMono-Regular, Menlo, monospace";
  context.fillText("usagebar · local only", WIDTH - 128, 566);
  context.textAlign = "left";

  const blob = await new Promise<Blob>((resolve, reject) =>
    canvas.toBlob((value) => (value ? resolve(value) : reject(new Error("PNG export failed"))), "image/png"),
  );
  return { dataUrl: canvas.toDataURL("image/png"), bytes: new Uint8Array(await blob.arrayBuffer()) };
}
