import { test } from "node:test";
import assert from "node:assert/strict";
import { layout, problems, renderIndex, renderSummary } from "./contact-sheet.mjs";

const clean = (name, extra = {}) => ({
  name,
  ms: 1,
  error: null,
  state: { tray: "53% · 9%", unhandled: [] },
  issues: { pageErrors: [], consoleErrors: [], warnings: [], csp: [], requestFailures: [] },
  ...extra,
});

function sampleReport() {
  return {
    createdAt: "2026-11-05T15:00:00.000Z",
    mode: "prod",
    strictCsp: true,
    seed: "7",
    start: "2026-11-05T15:00:00Z",
    scenarios: ["default", "nearly-out"],
    themes: ["light", "dark"],
    densities: ["roomy"],
    fonts: { ui: [{ familyName: "Inter" }, { familyName: "Inter" }], mono: [{ familyName: "DejaVu Sans Mono" }] },
    files: [
      "default-light-roomy.png",
      "default-light-roomy-full.png",
      "default-dark-roomy.png",
      "nearly-out-light-roomy.png",
      "settings-light.png",
      "settings-light-full.png",
      "onboarding-light-step1.png",
      "onboarding-light-step2.png",
      "timelapse/frame-000.png",
      "walkthrough.webm",
    ],
    failures: ["nearly-out-dark-roomy", "settings-light"],
    results: [
      clean("default-light-roomy"),
      clean("default-dark-roomy"),
      clean("nearly-out-light-roomy"),
      clean("nearly-out-dark-roomy", { error: "TimeoutError: waiting for selector\n    at stack" }),
      clean("settings-light", {
        issues: { pageErrors: ["TypeError: <x> is undefined"], consoleErrors: [], warnings: ["w"], csp: ["Content Security Policy violation: img-src"], requestFailures: [] },
        state: { unhandled: ["set_new_thing"] },
      }),
      clean("onboarding-light"),
      clean("timelapse"),
      clean("walkthrough"),
    ],
  };
}

test("layout puts scenario captures in a grid and the rest in their own rows", () => {
  const { columns, grid, extras, loose } = layout(sampleReport());
  assert.deepEqual(columns, ["light-roomy", "dark-roomy"]);
  assert.deepEqual(grid.map((row) => row.scenario), ["default", "nearly-out"]);
  assert.equal(grid[0].cells[0].png, "default-light-roomy.png");
  assert.equal(grid[0].cells[0].full, "default-light-roomy-full.png");
  assert.equal(grid[1].cells[1].png, null, "a capture that threw has no PNG but keeps its cell");
  assert.equal(grid[1].cells[1].result.name, "nearly-out-dark-roomy");

  const byName = Object.fromEntries(extras.map((extra) => [extra.result.name, extra]));
  assert.deepEqual(byName["settings-light"].shots, [{ png: "settings-light.png", full: "settings-light-full.png" }]);
  assert.deepEqual(byName["onboarding-light"].shots.map((shot) => shot.png), ["onboarding-light-step1.png", "onboarding-light-step2.png"]);
  assert.deepEqual(byName.timelapse.shots.map((shot) => shot.png), ["timelapse/frame-000.png"]);
  assert.deepEqual(byName.walkthrough.other, ["walkthrough.webm"]);
  assert.equal(byName["settings-light"].stem, "settings");
  assert.deepEqual(extras.map((extra) => extra.result.name), ["onboarding-light", "settings-light", "timelapse", "walkthrough"]);
  assert.deepEqual(loose, []);
});

test("layout derives the grid from capture names when report.json predates the fields", () => {
  const report = sampleReport();
  delete report.scenarios;
  delete report.themes;
  delete report.densities;
  const { columns, grid } = layout(report);
  assert.deepEqual(columns, ["light-roomy", "dark-roomy"]);
  assert.deepEqual(grid.map((row) => row.scenario), ["default", "nearly-out"]);
});

test("problems lists what failed a capture, CSP only when it is strict", () => {
  const settings = sampleReport().results.find((result) => result.name === "settings-light");
  assert.deepEqual(problems(settings), ["pageerror: TypeError: <x> is undefined", "unhandled IPC: set_new_thing"]);
  assert.equal(problems(settings, true).at(-1), "csp: Content Security Policy violation: img-src");
});

test("renderIndex escapes messages, marks failures, links full-height captures", () => {
  const html = renderIndex(sampleReport(), { siblings: ["dev"] });
  assert.match(html, /^<!doctype html>/);
  assert.ok(html.includes("TypeError: &lt;x&gt; is undefined"));
  assert.ok(!html.includes("<x>"));
  assert.ok(html.includes('href="default-light-roomy-full.png"'));
  assert.ok(html.includes('href="dev/index.html"'));
  assert.ok(html.includes("2 failed: nearly-out-dark-roomy, settings-light"));
  assert.ok(html.includes("Fonts: ui: Inter · mono: DejaVu Sans Mono"));
  assert.match(html, /<figure class="bad"><div class="missing">/);
  assert.ok(html.includes("<h2>settings — failed: settings-light</h2>"), "light and dark of a kind share one row");
  assert.ok(html.indexOf("onboarding-light-step1.png") < html.indexOf("onboarding-light-step2.png"));
});

test("renderIndex says so when everything passed", () => {
  const report = sampleReport();
  report.failures = [];
  assert.ok(renderIndex(report).includes("all captures clean"));
});

test("renderSummary is a pass/fail table plus the failures' first lines", () => {
  const markdown = renderSummary(sampleReport(), "sim-shots");
  assert.ok(markdown.startsWith("### Simulator: prod (`sim-shots`) — 2 failed"));
  assert.ok(markdown.includes("| scenario | light-roomy | dark-roomy |"));
  assert.ok(markdown.includes("| nearly-out | ok | **FAIL** |"));
  assert.ok(markdown.includes("settings-light: **FAIL**"));
  assert.ok(markdown.includes("error: TimeoutError: waiting for selector\n"));
  assert.ok(!markdown.includes("at stack"));
});
