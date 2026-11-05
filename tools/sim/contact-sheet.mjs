#!/usr/bin/env node
// Turns a sim:shots output folder (report.json + PNGs) into something a
// reviewer can skim in a few seconds:
//
//   index.html  one self-contained page: scenario × theme × density grid,
//               Settings / onboarding / widget rows, every issue next to the
//               capture that logged it, full-height captures one click away.
//               Relative links, so it works straight out of the CI artifact.
//   summary     Markdown for $GITHUB_STEP_SUMMARY (pass/fail per capture,
//               failures with their first lines, fonts used).
//
// shots.mjs calls renderIndex() itself after every run. The CLI is for CI:
//
//   node tools/sim/contact-sheet.mjs sim-shots sim-shots/dev
//   node tools/sim/contact-sheet.mjs --summary sim-shots sim-shots/dev >> "$GITHUB_STEP_SUMMARY"
import { existsSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { basename, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const escapeHtml = (value) =>
  String(value ?? "").replace(/[&<>"']/g, (char) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[char]);

const escapeUrl = (path) => path.split("/").map(encodeURIComponent).join("/");

const firstLine = (text) => String(text ?? "").split("\n")[0].slice(0, 300);

/** What failed in one capture, in the order the run reports it. */
export function problems(result, strictCsp = false) {
  const out = [];
  if (result.error) out.push(`error: ${firstLine(result.error)}`);
  for (const text of result.issues?.pageErrors ?? []) out.push(`pageerror: ${firstLine(text)}`);
  for (const text of result.issues?.consoleErrors ?? []) out.push(`console.error: ${firstLine(text)}`);
  for (const text of result.state?.unhandled ?? []) out.push(`unhandled IPC: ${firstLine(text)}`);
  if (strictCsp) for (const text of result.issues?.csp ?? []) out.push(`csp: ${firstLine(text)}`);
  return out;
}

/** Splits report.json into the grid, the extra rows, and loose files. */
export function layout(report) {
  const files = report.files ?? [];
  const fileSet = new Set(files);
  const results = report.results ?? [];
  const byName = new Map(results.map((result) => [result.name, result]));
  const names = [...byName.keys()];
  const capture = /^(.+)-(light|dark)-(roomy|dense)$/;
  const scenarios = report.scenarios ?? [...new Set(names.map((name) => name.match(capture)?.[1]).filter(Boolean))];
  const themes = report.themes ?? ["light", "dark"].filter((theme) => names.some((name) => capture.exec(name)?.[2] === theme));
  const densities = report.densities ?? ["roomy", "dense"].filter((density) => names.some((name) => capture.exec(name)?.[3] === density));
  const columns = themes.flatMap((theme) => densities.map((density) => `${theme}-${density}`));
  const used = new Set();
  const cell = (name) => {
    const result = byName.get(name) ?? null;
    const png = `${name}.png`;
    const fullPng = `${name}-full.png`;
    if (fileSet.has(png)) used.add(png);
    if (fileSet.has(fullPng)) used.add(fullPng);
    if (result) used.add(`result:${name}`);
    return { name, result, png: fileSet.has(png) ? png : null, full: fileSet.has(fullPng) ? fullPng : null };
  };
  const grid = scenarios.map((scenario) => ({ scenario, cells: columns.map((column) => cell(`${scenario}-${column}`)) }));

  // Settings, onboarding, widget, timelapse, walkthrough: every remaining job
  // with the files whose names start with it.
  const extras = [];
  const remaining = results.filter((result) => !used.has(`result:${result.name}`));
  // Longest name first, so "settings-light" does not claim "settings-light-x" of a longer job.
  const owners = [...remaining].sort((a, b) => b.name.length - a.name.length);
  const owned = new Map(remaining.map((result) => [result.name, []]));
  for (const file of files) {
    if (used.has(file)) continue;
    const owner = owners.find((result) => file.startsWith(`${result.name}.`) || file.startsWith(`${result.name}-`) || file.startsWith(`${result.name}/`));
    if (!owner) continue;
    owned.get(owner.name).push(file);
    used.add(file);
  }
  // Runs finish in any order; the page should not. Settings, onboarding,
  // widget…, each light before dark.
  const themeRank = (name) => {
    const index = themes.findIndex((theme) => name.endsWith(`-${theme}`));
    return index === -1 ? themes.length : index;
  };
  const stem = (name) => name.replace(/-(light|dark)$/, "");
  remaining.sort((a, b) => stem(a.name).localeCompare(stem(b.name)) || themeRank(a.name) - themeRank(b.name));
  for (const result of remaining) {
    // "widget-light" before "widget-light-hover", "step1" before "step2".
    const own = owned.get(result.name).sort((a, b) => a.replace(/\.png$/, "").localeCompare(b.replace(/\.png$/, "")));
    const pngs = own.filter((file) => file.endsWith(".png"));
    const shots = pngs
      .filter((file) => !(file.endsWith("-full.png") && pngs.includes(file.replace(/-full\.png$/, ".png"))))
      .map((png) => {
        const fullPng = png.replace(/\.png$/, "-full.png");
        return { png, full: pngs.includes(fullPng) ? fullPng : null };
      });
    extras.push({ result, stem: stem(result.name), shots, other: own.filter((file) => !file.endsWith(".png")) });
  }
  const loose = files.filter((file) => !used.has(file));
  return { columns, grid, extras, loose };
}

const STYLE = `
:root { color-scheme: light dark; --bg: #eceaf1; --card: #fff; --text: #1d1b22; --muted: #6b6676; --line: #d6d2de; --bad: #c62839; --bad-bg: #fde8ea; --ok: #2f7d4f; }
@media (prefers-color-scheme: dark) { :root { --bg: #18171c; --card: #24222a; --text: #ecebf0; --muted: #a29eac; --line: #3a3742; --bad: #ff6b7a; --bad-bg: #3b1d22; --ok: #6fcf97; } }
* { box-sizing: border-box; }
body { margin: 0; padding: 16px; background: var(--bg); color: var(--text); font: 13px/1.4 system-ui, -apple-system, "Segoe UI", sans-serif; }
h1 { margin: 0 0 4px; font-size: 18px; }
h2 { margin: 22px 0 8px; font-size: 14px; }
.meta { color: var(--muted); margin: 0 0 4px; }
.meta code { font-size: 12px; }
.verdict { display: inline-block; margin: 6px 0 0; padding: 3px 9px; border-radius: 999px; font-weight: 600; color: #fff; background: var(--ok); }
.verdict.bad { background: var(--bad); }
.grid { display: grid; gap: 10px; align-items: start; overflow-x: auto; }
.label { font-weight: 600; padding-top: 6px; word-break: break-word; }
.head { font-weight: 600; color: var(--muted); text-align: center; }
figure { margin: 0; padding: 6px; background: var(--card); border: 1px solid var(--line); border-radius: 10px; }
figure.bad { border: 2px solid var(--bad); background: var(--bad-bg); }
figure img { display: block; width: 100%; height: auto; border-radius: 6px; }
figcaption { margin-top: 4px; font-size: 11px; color: var(--muted); display: flex; gap: 6px; flex-wrap: wrap; }
figcaption a { color: inherit; }
.missing { display: grid; place-items: center; aspect-ratio: 388 / 720; color: var(--muted); font-size: 11px; }
.issues { margin: 4px 0 0; padding-left: 16px; font: 11px/1.35 ui-monospace, Menlo, monospace; color: var(--bad); word-break: break-word; }
.warn { color: var(--muted); }
.row { display: flex; gap: 10px; flex-wrap: wrap; align-items: flex-start; }
.row figure { width: 210px; }
details { margin-top: 18px; }
summary { cursor: pointer; font-weight: 600; }
.fulls { display: flex; gap: 8px; overflow-x: auto; align-items: flex-start; padding: 8px 0; }
.fulls figure { flex: none; width: 140px; }
ul.files { columns: 3 220px; }
`;

function issueList(result, strictCsp) {
  if (!result) return "";
  const items = problems(result, strictCsp).slice(0, 6).map((text) => `<li>${escapeHtml(text)}</li>`);
  const warnings = result.issues?.warnings?.length ?? 0;
  const csp = strictCsp ? 0 : result.issues?.csp?.length ?? 0;
  if (warnings) items.push(`<li class="warn">${warnings} console warning(s)</li>`);
  if (csp) items.push(`<li class="warn">${csp} CSP report(s)</li>`);
  return items.length ? `<ul class="issues">${items.join("")}</ul>` : "";
}

function figure({ png, full, title, result, failed, strictCsp, tray = false }) {
  const img = png
    ? `<a href="${escapeUrl(png)}" target="_blank"><img loading="lazy" src="${escapeUrl(png)}" alt="${escapeHtml(title)}"></a>`
    : `<div class="missing">no capture</div>`;
  const links = [`<span>${escapeHtml(title)}</span>`];
  if (full) links.push(`<a href="${escapeUrl(full)}" target="_blank">full height</a>`);
  if (tray && result?.state?.tray) links.push(`<span title="simulated tray title">${escapeHtml(result.state.tray)}</span>`);
  return `<figure class="${failed ? "bad" : ""}">${img}<figcaption>${links.join("")}</figcaption>${issueList(result, strictCsp)}</figure>`;
}

function fontLine(fonts) {
  if (!fonts) return "";
  if (fonts.error) return `probe failed (${fonts.error})`;
  const describe = (list) =>
    list?.length ? [...new Set(list.map((font) => `${font.familyName}${font.isCustomFont ? " (web font)" : ""}`))].join(", ") : "?";
  return Object.entries(fonts)
    .map(([role, list]) => `${role}: ${describe(list)}`)
    .join(" · ");
}

/** The whole index.html for one run. `siblings` are other runs to link to (relative paths). */
export function renderIndex(report, { siblings = [] } = {}) {
  const strictCsp = Boolean(report.strictCsp);
  const failed = new Set(report.failures ?? []);
  const { columns, grid, extras, loose } = layout(report);
  const captures = (report.files ?? []).filter((file) => file.endsWith(".png")).length;
  const csp = (report.results ?? []).reduce((sum, result) => sum + (result.issues?.csp?.length ?? 0), 0);
  const parts = [];
  parts.push(`<h1>UsageBar simulator — ${escapeHtml(report.mode ?? "?")}</h1>`);
  parts.push(
    `<p class="meta">${captures} screenshots from ${(report.results ?? []).length} pages · seed <code>${escapeHtml(report.seed)}</code> · start <code>${escapeHtml(report.start)}</code> · run ${escapeHtml(report.createdAt)}${report.durationMs ? ` in ${Math.round(report.durationMs / 1000)} s` : ""}</p>`,
  );
  if (report.fonts) parts.push(`<p class="meta">Fonts: ${escapeHtml(fontLine(report.fonts))}</p>`);
  if (report.mode === "prod") parts.push(`<p class="meta">CSP reports: ${csp}${strictCsp ? " (fail the run)" : " (not failing)"}</p>`);
  if (siblings.length) parts.push(`<p class="meta">Other runs: ${siblings.map((path) => `<a href="${escapeUrl(path)}/index.html">${escapeHtml(path)}</a>`).join(", ")}</p>`);
  parts.push(failed.size ? `<span class="verdict bad">${failed.size} failed: ${escapeHtml([...failed].join(", "))}</span>` : `<span class="verdict">all captures clean</span>`);

  if (grid.length) {
    parts.push(`<h2>Popover</h2>`);
    parts.push(`<div class="grid" style="grid-template-columns: 96px repeat(${columns.length}, minmax(150px, 210px))">`);
    parts.push(`<div></div>${columns.map((column) => `<div class="head">${escapeHtml(column)}</div>`).join("")}`);
    for (const row of grid) {
      parts.push(`<div class="label">${escapeHtml(row.scenario)}</div>`);
      for (const [index, cell] of row.cells.entries()) {
        parts.push(figure({ png: cell.png, full: cell.full, title: columns[index], result: cell.result, failed: failed.has(cell.name) || (!cell.png && cell.result), strictCsp, tray: true }));
      }
    }
    parts.push(`</div>`);
  }

  // One row per kind (settings, onboarding, widget…), light and dark side by side.
  const stems = [...new Set(extras.map((extra) => extra.stem))];
  for (const stem of stems) {
    const group = extras.filter((extra) => extra.stem === stem);
    const bad = group.filter((extra) => failed.has(extra.result.name)).map((extra) => extra.result.name);
    parts.push(`<h2>${escapeHtml(stem)}${bad.length ? ` — failed: ${escapeHtml(bad.join(", "))}` : ""}</h2><div class="row">`);
    const other = [];
    for (const extra of group) {
      const isBad = failed.has(extra.result.name);
      if (!extra.shots.length) parts.push(figure({ png: null, title: extra.result.name, result: extra.result, failed: isBad, strictCsp }));
      extra.shots.forEach((shot, index) => {
        const name = basename(shot.png, ".png");
        const title = name.startsWith(`${stem}-`) ? name.slice(stem.length + 1) : name;
        // The issues belong to the page, not to each step: show them once.
        parts.push(figure({ png: shot.png, full: shot.full, title, result: index === 0 ? extra.result : null, failed: isBad, strictCsp }));
      });
      other.push(...extra.other);
    }
    parts.push(`</div>`);
    if (other.length) parts.push(`<ul class="files">${other.map((file) => `<li><a href="${escapeUrl(file)}">${escapeHtml(file)}</a></li>`).join("")}</ul>`);
  }

  const fulls = grid.flatMap((row) => row.cells.filter((cell) => cell.full));
  if (fulls.length) {
    parts.push(`<details><summary>Full-height popover captures (${fulls.length})</summary><div class="fulls">`);
    for (const cell of fulls) parts.push(figure({ png: cell.full, title: cell.name, failed: failed.has(cell.name), strictCsp }));
    parts.push(`</div></details>`);
  }
  if (loose.length) {
    parts.push(`<details><summary>Other files (${loose.length})</summary><ul class="files">${loose.map((file) => `<li><a href="${escapeUrl(file)}">${escapeHtml(file)}</a></li>`).join("")}</ul></details>`);
  }
  parts.push(`<p class="meta"><a href="report.json">report.json</a> has every console message, CSP report and request failure.</p>`);
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>UsageBar simulator (${escapeHtml(report.mode ?? "?")})</title>
<style>${STYLE}</style>
</head>
<body>
${parts.join("\n")}
</body>
</html>
`;
}

/** Markdown for one run, for $GITHUB_STEP_SUMMARY. `label` names the folder. */
export function renderSummary(report, label) {
  const strictCsp = Boolean(report.strictCsp);
  const failed = new Set(report.failures ?? []);
  const results = report.results ?? [];
  const captures = (report.files ?? []).filter((file) => file.endsWith(".png")).length;
  const csp = results.reduce((sum, result) => sum + (result.issues?.csp?.length ?? 0), 0);
  const warnings = results.reduce((sum, result) => sum + (result.issues?.warnings?.length ?? 0), 0);
  const lines = [];
  lines.push(`### Simulator: ${report.mode ?? "?"} (\`${label}\`) — ${failed.size ? `${failed.size} failed` : "passed"}`);
  lines.push("");
  lines.push(
    `${captures} screenshots from ${results.length} pages · seed \`${report.seed}\` · start \`${report.start}\` · ${csp} CSP report(s)${strictCsp ? " (failing)" : ""} · ${warnings} console warning(s)`,
  );
  if (report.fonts) lines.push(`\nFonts: ${fontLine(report.fonts)}`);
  const { columns, grid, extras } = layout(report);
  if (grid.length) {
    lines.push("");
    lines.push(`| scenario | ${columns.join(" | ")} |`);
    lines.push(`| --- | ${columns.map(() => "---").join(" | ")} |`);
    for (const row of grid) {
      const marks = row.cells.map((cell) => (!cell.result ? "–" : failed.has(cell.name) ? "**FAIL**" : "ok"));
      lines.push(`| ${row.scenario} | ${marks.join(" | ")} |`);
    }
  }
  if (extras.length) {
    lines.push("");
    lines.push(extras.map((extra) => `${extra.result.name}: ${failed.has(extra.result.name) ? "**FAIL**" : "ok"}`).join(" · "));
  }
  for (const result of results.filter((entry) => failed.has(entry.name))) {
    lines.push("");
    lines.push(`<details open><summary><b>${escapeHtml(result.name)}</b></summary>`);
    lines.push("");
    lines.push("```");
    lines.push(...problems(result, strictCsp).slice(0, 12).map((text) => text.replace(/```/g, "'''")));
    lines.push("```");
    lines.push("</details>");
  }
  lines.push("");
  return lines.join("\n");
}

/** Run folders under `dir` (one level), relative to it. */
export function siblingRuns(dir) {
  if (!existsSync(dir)) return [];
  return readdirSync(dir)
    .filter((entry) => !entry.startsWith("."))
    .filter((entry) => {
      const path = join(dir, entry);
      return statSync(path).isDirectory() && existsSync(join(path, "report.json"));
    })
    .sort();
}

export function writeIndex(dir) {
  const report = JSON.parse(readFileSync(join(dir, "report.json"), "utf8"));
  const path = join(dir, "index.html");
  writeFileSync(path, renderIndex(report, { siblings: siblingRuns(dir) }));
  return path;
}

// ---- CLI ---------------------------------------------------------------------
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  const summary = args.includes("--summary");
  const dirs = args.filter((arg) => !arg.startsWith("--"));
  if (!dirs.length) {
    console.error("Usage: node tools/sim/contact-sheet.mjs [--summary] DIR [DIR...]");
    process.exit(2);
  }
  for (const dir of dirs) {
    const label = relative(process.cwd(), resolve(dir)) || ".";
    if (!existsSync(join(dir, "report.json"))) {
      // The run died before writing its report (build error, browser crash).
      if (summary) console.log(`### Simulator (\`${label}\`) — no report.json\n\nThe run did not finish; see the step log.\n`);
      else console.error(`${label}: no report.json`);
      continue;
    }
    const path = writeIndex(dir);
    if (summary) console.log(renderSummary(JSON.parse(readFileSync(join(dir, "report.json"), "utf8")), label));
    else console.error(`wrote ${path}`);
  }
}
