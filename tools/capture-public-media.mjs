#!/usr/bin/env node
/**
 * Capture the Vite preview UI for the README GIF and the public site.
 *
 *   npm run dev
 *   node tools/capture-public-media.mjs
 *
 * Writes retina PNGs to docs/_capture/. tools/compose-demo.py turns those
 * into docs/usagebar-demo.gif and website/screens/*.png.
 */
import { mkdir } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import puppeteer from "puppeteer-core";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const outDir = path.join(root, "docs", "_capture");
const base = process.env.USAGEBAR_PREVIEW_URL ?? "http://127.0.0.1:1420";
const chrome = process.env.CHROME_PATH ?? "/usr/bin/google-chrome";

const VIEW = { width: 388, height: 560, deviceScaleFactor: 3 };

const shots = [
  { name: "popover-codex.png", query: "?preview&providers=codex&alert&resetIn=42", wait: ".reset-alert" },
  { name: "popover-claude.png", query: "?preview&providers=claude", wait: "[data-provider='claude']" },
  { name: "popover-cursor.png", query: "?preview&providers=cursor", wait: "[data-provider='cursor']" },
  { name: "popover-opencode.png", query: "?preview&providers=opencode", wait: "[data-provider='opencode']" },
  { name: "popover-devin.png", query: "?preview&providers=devin", wait: "[data-provider='devin']" },
  { name: "popover-antigravity.png", query: "?preview&providers=antigravity", wait: "[data-provider='antigravity']" },
];

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function prepare(page) {
  await page.addStyleTag({
    content: `
      *, *::before, *::after {
        animation: none !important;
        transition: none !important;
        caret-color: transparent !important;
      }
      html, body { background: #f4f1fb !important; }
    `,
  });
  await page.evaluate(() => document.fonts.ready);
  await sleep(120);
}

async function shoot(page, file, selector) {
  const target = selector ? await page.$(selector) : page;
  await target.screenshot({
    path: path.join(outDir, file),
    type: "png",
    captureBeyondViewport: false,
  });
  console.log(`captured ${file}`);
}

async function openPreview(page, query, waitFor, { fit = true } = {}) {
  await page.setViewport(VIEW);
  await page.goto(`${base}/${query}`, { waitUntil: "networkidle0", timeout: 20000 });
  await page.waitForSelector(waitFor, { timeout: 8000 });
  if (fit) {
    const extra = await page.evaluate(() => {
      const scroller = document.querySelector(".content-scroll");
      if (!scroller) return 0;
      return Math.max(0, scroller.scrollHeight - scroller.clientHeight);
    });
    if (extra > 0) {
      await page.setViewport({ ...VIEW, height: VIEW.height + extra + 12 });
      await sleep(60);
    }
  }
  await prepare(page);
}

const browser = await puppeteer.launch({
  executablePath: chrome,
  headless: "new",
  args: [
    "--no-sandbox",
    "--disable-dev-shm-usage",
    "--force-color-profile=srgb",
    "--font-render-hinting=none",
    "--disable-gpu",
  ],
});

try {
  await mkdir(outDir, { recursive: true });
  const page = await browser.newPage();
  await page.setViewport(VIEW);

  for (const shot of shots) {
    await openPreview(page, shot.query, shot.wait);
    await shoot(page, shot.name);
  }

  await openPreview(page, "?preview&providers=codex", ".quota-section", { fit: false });
  await page.click("button.footer-action");
  await page.waitForSelector(".settings-modal");
  await prepare(page);
  // The window pickers sit under the tool list and get sliced by the fold.
  // Hide them so this still is the complete tool list, not a half control.
  await page.evaluate(() => {
    for (const group of document.querySelectorAll(".settings-body > .setting-group")) {
      const label = group.querySelector(":scope > .setting-label")?.textContent?.trim();
      if (label !== "Tools") group.style.display = "none";
    }
    const guide = document.querySelector(".settings-guide");
    if (guide) guide.style.display = "none";
  });
  await shoot(page, "settings-tools.png", ".sheet-modal");

  await page.evaluate(() => {
    const intro = document.querySelector(".settings-intro");
    if (intro) intro.style.display = "none";
    for (const group of document.querySelectorAll(".settings-body > .setting-group")) {
      const label = group.querySelector(":scope > .setting-label")?.textContent?.trim();
      const keep = label === "Menu bar" || label === "General";
      group.style.display = keep ? "" : "none";
    }
    const guide = document.querySelector(".settings-guide");
    if (guide) guide.style.display = "";
  });
  await sleep(40);
  await shoot(page, "settings-layout.png", ".sheet-modal");
} finally {
  await browser.close();
}
