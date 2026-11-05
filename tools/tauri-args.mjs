// Pure argument handling for tools/tauri.mjs, kept separate so it can be
// tested without spawning the Tauri CLI.

// tauri.conf.json ships an updater public key with
// `bundle.createUpdaterArtifacts: true`, so `tauri build` refuses to finish
// without TAURI_SIGNING_PRIVATE_KEY. Only the release workflow holds that key;
// local builds (`npm run app:install`) do not need updater artifacts at all.
export const NO_UPDATER_ARTIFACTS = JSON.stringify({ bundle: { createUpdaterArtifacts: false } });

export const NO_UPDATER_NOTE =
  "tauri.mjs: TAURI_SIGNING_PRIVATE_KEY is not set, so this local build skips the signed updater artifacts.";

function isTruthyEnv(value) {
  return value !== undefined && value !== "" && !/^(0|false|no|off)$/i.test(value);
}

/**
 * Returns the argv to pass to the Tauri CLI, plus an optional note to print.
 *
 * For an unsigned local `build` the override is inserted directly after the
 * subcommand. That keeps it ahead of any `--` runner arguments, and because
 * Tauri merges `--config` values in the order given, any `-c/--config` the
 * caller passes still wins over it.
 *
 * Left untouched:
 * - anything other than `build`;
 * - builds with a signing key, which should produce updater artifacts;
 * - CI, where a missing key on a release must fail rather than quietly ship
 *   without `latest.json`;
 * - Windows, whose tauri.windows.conf.json already turns updater artifacts off.
 */
export function tauriArgs(args, { env = {}, platform = "darwin" } = {}) {
  const subcommand = args.findIndex((arg) => !arg.startsWith("-"));
  if (subcommand === -1 || args[subcommand] !== "build") return { args, note: null };
  if (env.TAURI_SIGNING_PRIVATE_KEY) return { args, note: null };
  if (isTruthyEnv(env.CI)) return { args, note: null };
  if (platform === "win32") return { args, note: null };
  return {
    args: [
      ...args.slice(0, subcommand + 1),
      "--config",
      NO_UPDATER_ARTIFACTS,
      ...args.slice(subcommand + 1),
    ],
    note: NO_UPDATER_NOTE,
  };
}
