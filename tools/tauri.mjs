import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

const require = createRequire(import.meta.url);
const root = fileURLToPath(new URL("../", import.meta.url));
const cli = require.resolve("@tauri-apps/cli/tauri.js");
// VS Code can retain its old PATH after rustup is installed. Make the default
// Windows rustup installation available to Tauri and its Cargo subprocesses.
if (process.platform === "win32") {
  const cargoBin = join(process.env.CARGO_HOME || join(homedir(), ".cargo"), "bin");
  if (existsSync(join(cargoBin, "cargo.exe"))) {
    const pathKey = Object.keys(process.env).find((key) => key.toLowerCase() === "path") || "PATH";
    process.env[pathKey] = `${process.env[pathKey] || ""};${cargoBin}`;
  }
}
const result = spawnSync(process.execPath, [cli, ...process.argv.slice(2)], {
  cwd: root,
  stdio: "inherit",
});
if (result.error) console.error(result.error.message);
if (result.status !== 0) process.exit(result.status ?? 1);
if (process.platform === "darwin" && process.argv[2] === "build") {
  const cleanup = spawnSync("bash", ["tools/hide-build-from-spotlight.sh"], {
    cwd: root, stdio: "inherit",
  });
  process.exit(cleanup.status ?? 1);
}
