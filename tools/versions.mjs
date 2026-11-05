#!/usr/bin/env node
// One release version lives in eight fields across seven files. This keeps
// them in step.
//
//   node tools/versions.mjs check        fail if any file disagrees (CI)
//   node tools/versions.mjs set 0.2.5    rewrite every file to 0.2.5
//
// `check` also fails when it runs for a pushed `v*` tag that does not match,
// so a release can never publish a build whose version is not its tag.

import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

const SEMVER = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/;

/**
 * Each entry reads and rewrites one version field with a targeted pattern,
 * so `set` leaves the rest of the file (formatting included) untouched.
 * Patterns capture the text before the version as group 1. Line breaks are
 * matched as `\r?\n`: Windows runners check the repository out with CRLF.
 */
export const VERSION_FILES = [
  { label: "package.json", file: "package.json", pattern: /^(\s*"version":\s*")[^"]+/m },
  { label: "package-lock.json", file: "package-lock.json", pattern: /^(\s{2}"version":\s*")[^"]+/m },
  {
    label: 'package-lock.json packages[""]',
    file: "package-lock.json",
    pattern: /^(\s{4}"": \{\r?\n\s{6}"name": "usagebar",\r?\n\s{6}"version":\s*")[^"]+/m,
  },
  { label: "src-tauri/Cargo.toml", file: "src-tauri/Cargo.toml", pattern: /^(\[package\][^[]*?\r?\nversion\s*=\s*")[^"]+/m },
  { label: "src-tauri/Cargo.lock", file: "src-tauri/Cargo.lock", pattern: /^(name = "usagebar"\r?\nversion = ")[^"]+/m },
  { label: "src-tauri/tauri.conf.json", file: "src-tauri/tauri.conf.json", pattern: /^(\s*"version":\s*")[^"]+/m },
  { label: "Casks/usagebar.rb", file: "Casks/usagebar.rb", pattern: /^(\s*version\s+")[^"]+/m },
  { label: "homebrew-tap/Casks/usagebar.rb", file: "homebrew-tap/Casks/usagebar.rb", pattern: /^(\s*version\s+")[^"]+/m },
];

const defaultRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

/** Reads every version field. A field that cannot be found reads as `null`. */
export function readVersions(root = defaultRoot) {
  return VERSION_FILES.map(({ label, file, pattern }) => {
    const text = readFileSync(path.join(root, file), "utf8");
    const match = text.match(pattern);
    return { label, version: match ? match[0].slice(match[1].length) : null };
  });
}

/**
 * Problems with a set of readings, as human-readable lines. Empty means every
 * file agrees (and matches `tag`, when one is given).
 */
export function versionProblems(readings, tag = "") {
  const problems = [];
  for (const { label, version } of readings) {
    if (version === null) problems.push(`${label}: version field not found`);
  }
  const found = readings.filter((reading) => reading.version !== null);
  const distinct = [...new Set(found.map((reading) => reading.version))];
  if (distinct.length > 1) {
    problems.push(
      "versions disagree:",
      ...found.map(({ label, version }) => `  ${label}: ${version}`),
    );
  }
  if (tag && distinct.length === 1 && tag !== `v${distinct[0]}`) {
    problems.push(`tag ${tag} does not match version ${distinct[0]}`);
  }
  return problems;
}

/** Rewrites every version field to `version`. Throws on a field it cannot find. */
export function setVersion(version, root = defaultRoot) {
  if (!SEMVER.test(version)) throw new Error(`Not a semantic version: ${version}`);
  const byFile = new Map();
  for (const entry of VERSION_FILES) {
    const file = path.join(root, entry.file);
    const text = byFile.get(file) ?? readFileSync(file, "utf8");
    if (!entry.pattern.test(text)) throw new Error(`${entry.label}: version field not found`);
    byFile.set(file, text.replace(entry.pattern, (_, before) => `${before}${version}`));
  }
  for (const [file, text] of byFile) writeFileSync(file, text);
}

function main([command, value]) {
  if (command === "set" && value) {
    setVersion(value);
    console.log(`Set every version field to ${value}.`);
    return 0;
  }
  if (command === "check") {
    // Only a pushed tag is checked against the version: a manual workflow run
    // on a branch has no tag to compare.
    const tag =
      process.env.GITHUB_REF_TYPE === "tag" && process.env.GITHUB_REF_NAME?.startsWith("v")
        ? process.env.GITHUB_REF_NAME
        : "";
    const readings = readVersions();
    const problems = versionProblems(readings, tag);
    if (problems.length) {
      console.error(problems.join("\n"));
      console.error("\nFix with: node tools/versions.mjs set <version>");
      return 1;
    }
    console.log(`All ${readings.length} version fields agree on ${readings[0].version}${tag ? ` (tag ${tag})` : ""}.`);
    return 0;
  }
  console.error("Usage: node tools/versions.mjs check | set <version>");
  return 2;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  process.exitCode = main(process.argv.slice(2));
}
