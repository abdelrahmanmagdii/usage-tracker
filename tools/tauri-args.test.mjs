import assert from "node:assert/strict";
import { test } from "node:test";

import { NO_UPDATER_ARTIFACTS, NO_UPDATER_NOTE, tauriArgs } from "./tauri-args.mjs";

const local = { env: {}, platform: "darwin" };

test("an unsigned local build turns updater artifacts off", () => {
  const { args, note } = tauriArgs(["build", "--bundles", "app"], local);
  assert.deepEqual(args, ["build", "--config", NO_UPDATER_ARTIFACTS, "--bundles", "app"]);
  assert.equal(note, NO_UPDATER_NOTE);
  assert.deepEqual(JSON.parse(NO_UPDATER_ARTIFACTS), { bundle: { createUpdaterArtifacts: false } });
});

test("caller --config values come after the override so they still win", () => {
  const { args } = tauriArgs(["build", "-c", "extra.json", "--config", '{"a":1}'], local);
  assert.deepEqual(args, [
    "build", "--config", NO_UPDATER_ARTIFACTS, "-c", "extra.json", "--config", '{"a":1}',
  ]);
});

test("the override stays ahead of runner arguments after --", () => {
  const { args } = tauriArgs(["build", "--", "--locked"], local);
  assert.deepEqual(args, ["build", "--config", NO_UPDATER_ARTIFACTS, "--", "--locked"]);
});

test("global flags before the subcommand are kept in place", () => {
  const { args } = tauriArgs(["-v", "build"], local);
  assert.deepEqual(args, ["-v", "build", "--config", NO_UPDATER_ARTIFACTS]);
});

test("a build with a signing key is left alone", () => {
  const input = ["build", "--bundles", "app"];
  const result = tauriArgs(input, { env: { TAURI_SIGNING_PRIVATE_KEY: "key" }, platform: "darwin" });
  assert.deepEqual(result, { args: input, note: null });
});

test("CI is left alone so an unsigned release fails loudly", () => {
  const input = ["build"];
  assert.deepEqual(tauriArgs(input, { env: { CI: "true" }, platform: "darwin" }), { args: input, note: null });
  // CI=false (or 0) is a local build again.
  assert.equal(tauriArgs(input, { env: { CI: "false" }, platform: "darwin" }).note, NO_UPDATER_NOTE);
  assert.equal(tauriArgs(input, { env: { CI: "0" }, platform: "darwin" }).note, NO_UPDATER_NOTE);
});

test("Windows is left alone; its own config already skips updater artifacts", () => {
  const input = ["build", "--bundles", "nsis"];
  assert.deepEqual(tauriArgs(input, { env: {}, platform: "win32" }), { args: input, note: null });
});

test("other subcommands are untouched", () => {
  for (const input of [["dev"], ["info"], ["icon", "build"], [], ["--help"]]) {
    assert.deepEqual(tauriArgs(input, local), { args: input, note: null });
  }
});
