import { test } from "node:test";
import assert from "node:assert/strict";
import { cpSync, mkdtempSync, mkdirSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { VERSION_FILES, readVersions, setVersion, versionProblems } from "./versions.mjs";

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

/** A scratch copy of just the files that carry the version. */
function scratchCopy() {
  const root = mkdtempSync(path.join(tmpdir(), "usagebar-versions-"));
  for (const file of new Set(VERSION_FILES.map((entry) => entry.file))) {
    mkdirSync(path.dirname(path.join(root, file)), { recursive: true });
    cpSync(path.join(repo, file), path.join(root, file));
  }
  return root;
}

test("every version field in the repository is found and agrees", () => {
  const readings = readVersions(repo);
  assert.deepEqual(versionProblems(readings), []);
  assert.ok(readings.every(({ version }) => version !== null));
});

test("set rewrites only the version fields and check then passes", () => {
  const root = scratchCopy();
  try {
    const before = readFileSync(path.join(root, "src-tauri/Cargo.lock"), "utf8");
    setVersion("9.8.7", root);
    const readings = readVersions(root);
    assert.ok(readings.every(({ version }) => version === "9.8.7"));
    assert.deepEqual(versionProblems(readings, "v9.8.7"), []);
    // Other packages in the lockfile keep their own versions.
    const after = readFileSync(path.join(root, "src-tauri/Cargo.lock"), "utf8");
    assert.equal(after.split("\n").length, before.split("\n").length);
    assert.equal(
      after.split("\n").filter((line, index) => line !== before.split("\n")[index]).length,
      1,
    );
    // Both JSON files still parse.
    JSON.parse(readFileSync(path.join(root, "package-lock.json"), "utf8"));
    JSON.parse(readFileSync(path.join(root, "src-tauri/tauri.conf.json"), "utf8"));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("a disagreeing file, a missing field, or a wrong tag is reported", () => {
  const agreeing = [
    { label: "a", version: "1.0.0" },
    { label: "b", version: "1.0.0" },
  ];
  assert.deepEqual(versionProblems(agreeing, "v1.0.0"), []);
  assert.match(versionProblems(agreeing, "v1.0.1").join("\n"), /tag v1.0.1 does not match/);
  assert.match(
    versionProblems([...agreeing, { label: "c", version: "1.1.0" }]).join("\n"),
    /versions disagree[\s\S]*c: 1.1.0/,
  );
  assert.match(versionProblems([{ label: "d", version: null }]).join("\n"), /d: version field not found/);
});

test("set refuses something that is not a version", () => {
  assert.throws(() => setVersion("v1.2", repo), /Not a semantic version/);
});
