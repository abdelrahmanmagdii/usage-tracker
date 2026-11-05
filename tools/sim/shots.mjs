#!/usr/bin/env node
// Screenshots of the real UsageBar frontend running against the simulated
// backend (sim.html). Starts Vite on a free port, drives Chromium through
// Playwright, writes PNGs + report.json to sim-shots/ (gitignored), and
// exits non-zero if the page throws, logs a console error, or invokes an IPC
// command the simulator does not know.
//
//   npm run sim:shots
//   npm run sim:shots -- --scenarios busy,errors --themes dark --no-full
//   npm run sim:shots -- --prod          # production build + report-only CSP
//   npm run sim:shots -- --video --timelapse
//
// See docs/SIMULATION.md.
import { createRequire } from "node:module";
import { spawnSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync, renameSync } from "node:fs";
import { createServer as createNetServer } from "node:net";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const require = createRequire(import.meta.url);

// ---- options ---------------------------------------------------------------
const argv = process.argv.slice(2);
const opt = (name, fallback) => {
  const index = argv.indexOf(`--${name}`);
  if (index === -1) return fallback;
  const value = argv[index + 1];
  return value === undefined || value.startsWith("--") ? true : value;
};
const list = (name, fallback) => String(opt(name, fallback)).split(",").map((s) => s.trim()).filter(Boolean);

if (argv.includes("--help")) {
  console.log(`Usage: node tools/sim/shots.mjs [options]
  --out DIR            output folder (default sim-shots)
  --scenarios LIST     default,busy,idle,nearly-out,errors,fresh-install
  --themes LIST        light,dark
  --density LIST       roomy,dense
  --seed N             simulation seed (default 7)
  --start ISO          simulated start time (default 2026-11-05T15:00:00Z)
  --no-full            skip the full-height captures
  --no-extras          skip settings / onboarding / widget captures
  --timelapse          busy scenario, 15-minute steps, frames + mp4/gif (ffmpeg)
  --video              recorded walkthrough (webm, mp4 when ffmpeg exists)
  --prod               build sim.html for production and serve it with the
                       app's CSP (tauri.conf.json) as Content-Security-Policy-Report-Only
  --strict-csp         with --prod: CSP violations fail the run
  --copy-to DIR        also copy every output file into DIR
  --concurrency N      pages in parallel (default 4)`);
  process.exit(0);
}

const outDir = resolve(root, String(opt("out", "sim-shots")));
const scenarios = list("scenarios", "default,busy,idle,nearly-out,errors,fresh-install");
const themes = list("themes", "light,dark");
const densities = list("density", "roomy,dense");
const seed = String(opt("seed", "7"));
const start = String(opt("start", "2026-11-05T15:00:00Z"));
const full = !argv.includes("--no-full");
const extras = !argv.includes("--no-extras");
const prod = argv.includes("--prod");
const strictCsp = argv.includes("--strict-csp");
const concurrency = Number(opt("concurrency", "4")) || 4;
const copyTo = opt("copy-to", null);
const VIEWPORT = { width: 388, height: 720 };

let playwright;
try {
  playwright = require("playwright");
} catch {
  console.error("Playwright is not installed. Install it globally (npm i -g playwright) or as a devDependency.");
  process.exit(2);
}

function freePort() {
  return new Promise((resolvePort, reject) => {
    const server = createNetServer();
    server.unref();
    server.on("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const { port } = server.address();
      server.close(() => resolvePort(port));
    });
  });
}

/** The production CSP from tauri.conf.json, as a header value. */
function appCsp() {
  const conf = JSON.parse(readFileSync(join(root, "src-tauri/tauri.conf.json"), "utf8"));
  const csp = conf?.app?.security?.csp;
  if (!csp) return null;
  if (typeof csp === "string") return csp;
  return Object.entries(csp).map(([key, value]) => `${key} ${Array.isArray(value) ? value.join(" ") : value}`).join("; ");
}

// ---- server ----------------------------------------------------------------
async function startServer() {
  const vite = await import("vite");
  const port = await freePort();
  if (!prod) {
    const server = await vite.createServer({
      root,
      logLevel: "warn",
      clearScreen: false,
      server: { port, strictPort: true, host: "127.0.0.1", hmr: false },
    });
    await server.listen();
    return { url: `http://127.0.0.1:${port}`, close: () => server.close() };
  }
  const distDir = join(outDir, ".dist"); // per checkout; node_modules may be shared
  await vite.build({
    root,
    logLevel: "warn",
    build: { outDir: distDir, emptyOutDir: true, rollupOptions: { input: { sim: join(root, "sim.html") } } },
  });
  const csp = appCsp();
  const server = await vite.preview({
    root,
    logLevel: "warn",
    build: { outDir: distDir },
    preview: {
      port,
      strictPort: true,
      host: "127.0.0.1",
      headers: csp ? { "Content-Security-Policy-Report-Only": csp } : {},
    },
  });
  return { url: `http://127.0.0.1:${port}`, close: () => server.close() };
}

// ---- capture helpers ---------------------------------------------------------
const results = [];
const files = [];

function simUrl(base, params) {
  const query = new URLSearchParams({ seed, start, speed: "0", ...params });
  return `${base}/sim.html?${query.toString().replace(/=(&|$)/g, "$1")}`;
}

async function openPage(browser, { theme = "light", viewport = VIEWPORT, video } = {}) {
  const context = await browser.newContext({
    viewport,
    deviceScaleFactor: 2,
    colorScheme: theme,
    timezoneId: "UTC",
    locale: "en-US",
    ...(video ? { recordVideo: { dir: video, size: viewport } } : {}),
  });
  const page = await context.newPage();
  const issues = { pageErrors: [], consoleErrors: [], warnings: [], csp: [], requestFailures: [] };
  page.on("pageerror", (error) => issues.pageErrors.push(String(error?.stack ?? error)));
  page.on("console", (message) => {
    const text = message.text();
    if (/Content Security Policy|Content-Security-Policy/i.test(text)) issues.csp.push(text);
    else if (message.type() === "error") issues.consoleErrors.push(text);
    else if (message.type() === "warning") issues.warnings.push(text);
  });
  page.on("requestfailed", (request) => {
    const failure = request.failure()?.errorText ?? "failed";
    // Media streams from a closing page are not interesting.
    if (!/ERR_ABORTED/.test(failure)) issues.requestFailures.push(`${request.url()} ${failure}`);
  });
  await page.addInitScript(() => {
    document.addEventListener("securitypolicyviolation", (event) => {
      console.warn(`Content Security Policy violation: ${event.violatedDirective} blocked ${event.blockedURI || "inline"} (${event.disposition})`);
    });
  });
  return { context, page, issues };
}

async function ready(page, url) {
  await page.goto(url, { waitUntil: "load" });
  await page.waitForSelector("html[data-sim-ready]", { timeout: 20_000 });
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(900); // first poll, transitions
}

async function shot(page, name) {
  const path = join(outDir, `${name}.png`);
  await page.screenshot({ path });
  files.push(path);
  return path;
}

/** Grows the viewport so a scrolled panel shows in one image. */
async function fullShot(page, name, scrollerSelector) {
  const extra = await page.evaluate((selector) => {
    const scroller = document.querySelector(selector);
    return scroller ? Math.max(0, scroller.scrollHeight - scroller.clientHeight) : 0;
  }, scrollerSelector);
  const size = page.viewportSize();
  await page.setViewportSize({ width: size.width, height: Math.min(6_000, size.height + extra + 2) });
  await page.waitForTimeout(400);
  const path = await shot(page, name);
  await page.setViewportSize(size);
  return path;
}

async function simState(page) {
  return page.evaluate(() => ({
    simNow: window.__sim.now(),
    tray: window.__sim.tray().title,
    unhandled: window.__sim.unhandled(),
    notifications: window.__sim.notifications.length,
    seededHistory: window.__sim.seededHistory,
    blocked: window.__sim.log.filter((entry) => entry.kind === "warn").map((entry) => entry.name),
  }));
}

async function job(browser, name, run, pageOptions) {
  const started = Date.now();
  const { context, page, issues } = await openPage(browser, pageOptions);
  let error = null;
  let state = null;
  try {
    await run(page);
    state = await simState(page).catch(() => null);
  } catch (caught) {
    error = String(caught?.stack ?? caught);
  }
  const video = pageOptions?.video ? await page.video()?.path() : null;
  await context.close();
  results.push({ name, ms: Date.now() - started, error, state, issues, video });
  const flag = error || issues.pageErrors.length || issues.consoleErrors.length || state?.unhandled?.length ? "FAIL" : "ok";
  console.log(`${flag.padEnd(4)} ${name}${error ? `: ${error.split("\n")[0]}` : ""}`);
  return video;
}

// ---- jobs ------------------------------------------------------------------
function captureJobs(base) {
  const jobs = [];
  for (const scenario of scenarios) {
    for (const theme of themes) {
      for (const density of densities) {
        const name = `${scenario}-${theme}-${density}`;
        jobs.push((browser) =>
          job(
            browser,
            name,
            async (page) => {
              await ready(page, simUrl(base, { scenario, ...(density === "dense" ? { dense: "1" } : {}) }));
              if (scenario === "fresh-install") {
                // Let the Codex App Server "start" (3 simulated seconds).
                await page.evaluate(() => window.__sim.advance("5s"));
                await page.waitForTimeout(500);
              }
              await shot(page, name);
              // fresh-install opens on the walkthrough; the full view is the app.
              if (full && scenario !== "fresh-install") await fullShot(page, `${name}-full`, ".content-scroll");
            },
            { theme },
          ),
        );
      }
    }
  }
  if (!extras) return jobs;
  for (const theme of themes) {
    jobs.push((browser) =>
      job(
        browser,
        `settings-${theme}`,
        async (page) => {
          await ready(page, simUrl(base, { scenario: "default" }));
          await page.locator(".app-footer").getByRole("button", { name: /settings/i }).click();
          await page.waitForSelector(".settings-modal, [aria-labelledby='settings-title']", { timeout: 5_000 });
          await page.waitForTimeout(500);
          await shot(page, `settings-${theme}`);
          if (full) await fullShot(page, `settings-${theme}-full`, ".settings-body");
        },
        { theme },
      ),
    );
    jobs.push((browser) =>
      job(
        browser,
        `onboarding-${theme}`,
        async (page) => {
          await ready(page, simUrl(base, { scenario: "fresh-install" }));
          await page.evaluate(() => window.__sim.advance("5s")); // Codex "starting" → connected
          await page.waitForSelector("[aria-label='UsageBar setup']", { timeout: 5_000 });
          for (let step = 1; step <= 6; step += 1) {
            await page.waitForTimeout(400);
            await shot(page, `onboarding-${theme}-step${step}`);
            const next = page.getByRole("button", { name: /^next/i });
            if (!(await next.count())) break;
            await next.click();
          }
        },
        { theme },
      ),
    );
    jobs.push((browser) =>
      job(
        browser,
        `widget-${theme}`,
        async (page) => {
          await ready(page, simUrl(base, { scenario: "default", widget: "", bg: "wallpaper" }));
          await shot(page, `widget-${theme}`);
          await page.mouse.move(180, 14);
          await page.waitForTimeout(900);
          await shot(page, `widget-${theme}-hover`);
        },
        { theme, viewport: { width: 380, height: 200 } },
      ),
    );
  }
  return jobs;
}

function haveFfmpeg() {
  return spawnSync("ffmpeg", ["-version"], { stdio: "ignore" }).status === 0;
}

async function timelapse(browser, base) {
  const frames = join(outDir, "timelapse");
  rmSync(frames, { recursive: true, force: true });
  mkdirSync(frames, { recursive: true });
  await job(browser, "timelapse", async (page) => {
    await ready(page, simUrl(base, { scenario: "busy", hud: "1" }));
    for (let frame = 0; frame < 48; frame += 1) {
      if (frame) await page.evaluate(() => window.__sim.advance("15m"));
      await page.waitForTimeout(350);
      const path = join(frames, `frame-${String(frame).padStart(3, "0")}.png`);
      await page.screenshot({ path });
      files.push(path);
    }
  });
  if (!haveFfmpeg()) return;
  const input = join(frames, "frame-%03d.png");
  const mp4 = join(outDir, "timelapse.mp4");
  spawnSync("ffmpeg", ["-y", "-loglevel", "error", "-framerate", "4", "-i", input, "-vf", "scale=388:-2", "-pix_fmt", "yuv420p", mp4]);
  const gif = join(outDir, "timelapse.gif");
  spawnSync("ffmpeg", ["-y", "-loglevel", "error", "-framerate", "4", "-i", input, "-vf", "scale=388:-1:flags=lanczos,split[a][b];[a]palettegen[p];[b][p]paletteuse", gif]);
  for (const path of [mp4, gif]) if (existsSync(path)) files.push(path);
}

async function walkthrough(browser, base) {
  const videoDir = join(outDir, ".video");
  rmSync(videoDir, { recursive: true, force: true });
  const video = await job(
    browser,
    "walkthrough",
    async (page) => {
      await ready(page, simUrl(base, { scenario: "default", hud: "1", speed: "60" }));
      const scroller = page.locator(".content-scroll");
      for (let i = 0; i < 6; i += 1) {
        await scroller.evaluate((el) => el.scrollBy({ top: 260, behavior: "smooth" }));
        await page.waitForTimeout(700);
      }
      await scroller.evaluate((el) => el.scrollTo({ top: 0, behavior: "smooth" }));
      await page.waitForTimeout(800);
      // Fable's week renews 47 simulated minutes in.
      await page.evaluate(() => window.__sim.advance("46m"));
      await page.waitForTimeout(2_500);
      await page.evaluate(() => window.__sim.signOut("claude", 600));
      await page.waitForTimeout(1_500);
      await page.locator(".app-footer").getByRole("button", { name: /settings/i }).click();
      await page.waitForTimeout(1_200);
      await page.locator(".settings-body").evaluate((el) => el.scrollBy({ top: 400, behavior: "smooth" }));
      await page.waitForTimeout(1_200);
      await page.keyboard.press("Escape");
      await page.waitForTimeout(600);
      await page.evaluate(() => window.__sim.setPrefs({ denseLayout: true }));
      await page.waitForTimeout(1_500);
      await page.evaluate(() => window.__sim.advance("10m"));
      await page.waitForTimeout(1_500);
    },
    { theme: "light", video: videoDir },
  );
  if (!video || !existsSync(video)) return;
  const webm = join(outDir, "walkthrough.webm");
  renameSync(video, webm);
  files.push(webm);
  if (haveFfmpeg()) {
    const mp4 = join(outDir, "walkthrough.mp4");
    spawnSync("ffmpeg", ["-y", "-loglevel", "error", "-i", webm, "-pix_fmt", "yuv420p", "-vf", "scale=388:-2", mp4]);
    if (existsSync(mp4)) files.push(mp4);
  }
  rmSync(videoDir, { recursive: true, force: true });
}

async function pool(tasks, size, browser) {
  const queue = [...tasks];
  await Promise.all(
    Array.from({ length: Math.min(size, queue.length) }, async () => {
      while (queue.length) await queue.shift()(browser);
    }),
  );
}

// ---- main ------------------------------------------------------------------
mkdirSync(outDir, { recursive: true });
for (const entry of readdirSync(outDir)) {
  if (entry.endsWith(".png") || entry === "report.json") rmSync(join(outDir, entry), { force: true });
}
const server = await startServer();
console.log(`sim page: ${server.url}/sim.html (${prod ? "production build, report-only CSP" : "vite dev"})`);
const browser = await playwright.chromium.launch();
let exitCode = 0;
try {
  await pool(captureJobs(server.url), concurrency, browser);
  if (argv.includes("--timelapse")) await timelapse(browser, server.url);
  if (argv.includes("--video")) await walkthrough(browser, server.url);
} finally {
  await browser.close();
  await server.close();
}

const failures = results.filter(
  (result) =>
    result.error ||
    result.issues.pageErrors.length ||
    result.issues.consoleErrors.length ||
    result.state?.unhandled?.length ||
    (strictCsp && result.issues.csp.length),
);
const cspCount = results.reduce((sum, result) => sum + result.issues.csp.length, 0);
const report = {
  createdAt: new Date().toISOString(),
  mode: prod ? "prod" : "dev",
  seed,
  start,
  files: files.map((path) => path.slice(outDir.length + 1)),
  failures: failures.map((result) => result.name),
  results,
};
writeFileSync(join(outDir, "report.json"), JSON.stringify(report, null, 2));

if (copyTo && copyTo !== true) {
  const target = resolve(String(copyTo));
  mkdirSync(target, { recursive: true });
  // Only this run's output (sim-shots/ may also hold sim:linux builds).
  for (const path of [...files, join(outDir, "report.json")]) {
    const dest = join(target, path.slice(outDir.length + 1));
    mkdirSync(dirname(dest), { recursive: true });
    cpSync(path, dest);
  }
}

console.log(`\n${files.length} files in ${outDir}`);
if (cspCount) console.log(`${cspCount} CSP report(s) — see report.json (issues.csp)`);
for (const result of failures) {
  console.log(`\nFAILED ${result.name}`);
  if (result.error) console.log(`  error: ${result.error}`);
  for (const text of result.issues.pageErrors) console.log(`  pageerror: ${text}`);
  for (const text of result.issues.consoleErrors) console.log(`  console.error: ${text}`);
  for (const text of result.state?.unhandled ?? []) console.log(`  unhandled IPC: ${text}`);
  if (strictCsp) for (const text of result.issues.csp) console.log(`  csp: ${text}`);
}
if (failures.length) exitCode = 1;
process.exit(exitCode);
