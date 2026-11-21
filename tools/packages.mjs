#!/usr/bin/env node
// Package-manager and updater metadata for a UsageBar release, from the
// actual release artifacts. Standard library only; nothing here reads a secret.
//
//   node tools/packages.mjs winget <version> <installer> <out-dir>
//   node tools/packages.mjs windows-feed <version> <installer> <sig> <out-json>
//   node tools/packages.mjs cask <version> <dmg>
//   node tools/packages.mjs check-release <tag> <latest-public-tag>
//   node tools/packages.mjs verify-feeds <version> <download-dir>

import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

export const REPO_URL = "https://github.com/abdelrahmanmagdii/usage-tracker";
export const WINGET_ID = "AbdelrahmanAmer.UsageBar";
export const WINGET_SCHEMA = "1.6.0";
const CASK_FILES = ["Casks/usagebar.rb", "homebrew-tap/Casks/usagebar.rb"];
const SEGMENT = "(0|[1-9]\\d*)";
const STABLE = new RegExp(`^${SEGMENT}\\.${SEGMENT}\\.${SEGMENT}$`);
const defaultRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

/** The three numeric segments of a stable `M.m.p` version. Throws on anything else. */
export function parseStable(version) {
  const match = STABLE.exec(String(version));
  if (!match) throw new Error(`Not a stable version (M.m.p): ${version}`);
  const parts = match.slice(1).map(Number);
  if (!parts.every(Number.isSafeInteger)) throw new Error(`Version segment too large: ${version}`);
  return parts;
}

/** Negative, zero or positive, comparing the numeric segments. */
export function compareVersions(a, b) {
  const [x, y] = [parseStable(a), parseStable(b)];
  for (let index = 0; index < 3; index += 1) {
    if (x[index] !== y[index]) return x[index] - y[index];
  }
  return 0;
}

const stable = (version) => (parseStable(version), String(version));

export const installerName = (version) => `UsageBar_${stable(version)}_x64-setup.exe`;
export const dmgName = (version) => `UsageBar_${stable(version)}_universal.dmg`;
export const releaseUrl = (version, name) => `${REPO_URL}/releases/download/v${version}/${name}`;

/**
 * Whether a tag may be published: a stable `vM.m.p` strictly newer than the
 * latest public release, so a rerun can never overwrite or downgrade it.
 */
export function checkRelease(tag, latestTag) {
  const version = (value, what) => {
    if (!/^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(String(value))) throw new Error(`${what} is not a stable vM.m.p tag: ${value}`);
    return String(value).slice(1);
  };
  const next = version(tag, "Release tag");
  const current = version(latestTag, "Latest public tag");
  if (compareVersions(next, current) <= 0) {
    throw new Error(`${tag} is not newer than the public release ${latestTag}`);
  }
}

/** Reads a non-empty regular file whose name must be exactly `expectedName`. */
export function readArtifact(file, expectedName) {
  if (path.basename(file) !== expectedName) {
    throw new Error(`Expected a file named ${expectedName}, got ${path.basename(file)}`);
  }
  const info = statSync(file);
  if (!info.isFile() || info.size === 0) throw new Error(`${file} is not a non-empty regular file`);
  const bytes = readFileSync(file);
  return { bytes, sha256: createHash("sha256").update(bytes).digest("hex") };
}

const BASE64 = /^[A-Za-z0-9+/]+={0,2}$/;

/**
 * Checks a Tauri updater `.sig` is a minisign signature file, base64-encoded
 * as the Tauri signer writes it, and returns it trimmed. It is public data;
 * an encrypted secret key is refused.
 */
export function validateTauriSignature(text) {
  const signature = String(text).trim();
  if (!signature || !BASE64.test(signature)) throw new Error("Signature is not base64 text");
  const lines = Buffer.from(signature, "base64").toString("utf8").split(/\r?\n/);
  const [comment, signed, trusted, global] = lines;
  const decoded = (line) => (BASE64.test(line ?? "") ? Buffer.from(line, "base64").length : 0);
  const valid =
    comment?.startsWith("untrusted comment: ") &&
    !/encrypted secret key/i.test(comment) &&
    decoded(signed) === 74 &&
    trusted?.startsWith("trusted comment: ") &&
    decoded(global) === 64;
  if (!valid) throw new Error("Signature is not a minisign signature file");
  return signature;
}

/** The Tauri static update feed for the Windows NSIS installer. */
export function windowsFeed(version, signature) {
  const url = releaseUrl(version, installerName(version));
  const platform = { url, signature: validateTauriSignature(signature) };
  return {
    version,
    platforms: { "windows-x86_64": platform, "windows-x86_64-nsis": { ...platform } },
  };
}

/** The three community-repository manifests, keyed by file name. */
export function wingetManifests(version, sha256) {
  const name = installerName(version);
  const header = `PackageIdentifier: ${WINGET_ID}\nPackageVersion: ${version}\n`;
  const footer = (type) => `ManifestType: ${type}\nManifestVersion: ${WINGET_SCHEMA}\n`;
  const files = {};
  files[`${WINGET_ID}.yaml`] = `${header}DefaultLocale: en-US\n${footer("version")}`;
  files[`${WINGET_ID}.installer.yaml`] =
    `${header}InstallerLocale: en-US\nInstallerType: nullsoft\nScope: user\n` +
    `InstallModes:\n- interactive\n- silent\n- silentWithProgress\n` +
    `InstallerSwitches:\n  Silent: /S\n  SilentWithProgress: /P\n` +
    `UpgradeBehavior: install\nProductCode: UsageBar\n` +
    `AppsAndFeaturesEntries:\n- DisplayName: UsageBar\n  ProductCode: UsageBar\n` +
    `Installers:\n- Architecture: x64\n  InstallerUrl: ${releaseUrl(version, name)}\n` +
    `  InstallerSha256: ${sha256.toUpperCase()}\n${footer("installer")}`;
  files[`${WINGET_ID}.locale.en-US.yaml`] =
    `${header}PackageLocale: en-US\nPublisher: Abdelrahman Amer\n` +
    `PublisherUrl: https://github.com/abdelrahmanmagdii\nPackageName: UsageBar\n` +
    `PackageUrl: ${REPO_URL}\nLicense: MIT\nLicenseUrl: ${REPO_URL}/blob/v${version}/LICENSE\n` +
    `ShortDescription: "AI coding quota windows and reset times in the Windows system tray."\n` +
    `Tags:\n- codex\n- claude\n- quota\n- usage\n- tray\n${footer("defaultLocale")}`;
  return files;
}

/** Writes the manifests under `<outDir>/manifests/a/AbdelrahmanAmer/UsageBar/<version>`. */
export function writeWingetManifests(version, installer, outDir) {
  const { sha256 } = readArtifact(installer, installerName(version));
  const dir = path.join(outDir, "manifests", "a", "AbdelrahmanAmer", "UsageBar", version);
  mkdirSync(dir, { recursive: true });
  for (const [file, text] of Object.entries(wingetManifests(version, sha256))) {
    writeFileSync(path.join(dir, file), text);
  }
  return dir;
}

const MAC_BUNDLE = "UsageBar_universal.app.tar.gz";
const MAC_PLATFORMS = ["darwin-aarch64", "darwin-x86_64", "darwin-aarch64-app", "darwin-x86_64-app"];
const WINDOWS_PLATFORMS = ["windows-x86_64", "windows-x86_64-nsis"];

/**
 * Checks both update feeds in a draft release's downloaded files: each entry
 * must carry the release version, the versioned download URL and exactly the
 * signature from the matching sidecar. Expects `latest.json`,
 * `latest-windows.json`, `UsageBar_universal.app.tar.gz.sig` and
 * `<installer>.sig` in `dir`. Throws on the first problem.
 */
export function verifyFeeds(version, dir) {
  const exe = installerName(version);
  const read = (name) => readFileSync(path.join(dir, name), "utf8");
  const feeds = [
    ["latest.json", MAC_PLATFORMS, MAC_BUNDLE],
    ["latest-windows.json", WINDOWS_PLATFORMS, exe],
  ];
  for (const [file, platforms, asset] of feeds) {
    const feed = JSON.parse(read(file));
    if (feed.version !== version) throw new Error(`${file} has version ${feed.version}, not ${version}`);
    const signature = validateTauriSignature(read(`${asset}.sig`));
    for (const key of platforms) {
      const entry = feed.platforms?.[key];
      if (entry?.url !== releaseUrl(version, asset)) throw new Error(`${file} ${key} has the wrong url`);
      if (String(entry.signature ?? "").trim() !== signature) {
        throw new Error(`${file} ${key} does not carry the signature of ${asset}`);
      }
    }
  }
}

const VERSION_LINE = /^(\s*version\s+")([^"]+)(")/m;
const SHA_LINE = /^(\s*sha256\s+)(?::no_check|"[0-9a-f]{64}")/m;

/**
 * The cask text pinned to a published `version` and DMG digest. Returns the
 * same text for an identical pin; refuses a downgrade and refuses to rewrite
 * the digest already pinned for the same version.
 */
export function pinCask(text, version, sha256) {
  const current = VERSION_LINE.exec(text)?.[2];
  const sha = SHA_LINE.exec(text);
  if (!current || !sha) throw new Error("Cask version or sha256 line not found");
  if (!/^[0-9a-f]{64}$/.test(sha256)) throw new Error("Not a SHA-256 digest");
  const order = compareVersions(version, current);
  if (order < 0) throw new Error(`Refusing to downgrade the cask from ${current} to ${version}`);
  const pinned = /"([0-9a-f]{64})"/.exec(sha[0])?.[1];
  if (order === 0 && pinned && pinned !== sha256) {
    throw new Error(`Refusing to rewrite the digest pinned for ${version}`);
  }
  return text
    .replace(VERSION_LINE, (_, before, __, after) => `${before}${version}${after}`)
    .replace(SHA_LINE, (_, before) => `${before}"${sha256}"`);
}

/** Pins every cask file, writing none unless all of them accept the change. */
export function updateCasks(version, dmg, root = defaultRoot) {
  const { sha256 } = readArtifact(dmg, dmgName(version));
  const updates = CASK_FILES.map((file) => {
    const full = path.join(root, file);
    const before = readFileSync(full, "utf8");
    return { full, before, after: pinCask(before, version, sha256) };
  });
  const changed = updates.filter((update) => update.after !== update.before);
  for (const update of changed) writeFileSync(update.full, update.after);
  return changed.length;
}

function main([command, ...args]) {
  if (command === "winget" && args.length === 3) {
    console.log(writeWingetManifests(args[0], args[1], args[2]));
    return 0;
  }
  if (command === "windows-feed" && args.length === 4) {
    const [version, installer, sigFile, out] = args;
    readArtifact(installer, installerName(version));
    readArtifact(sigFile, `${installerName(version)}.sig`);
    const feed = windowsFeed(version, readFileSync(sigFile, "utf8"));
    mkdirSync(path.dirname(path.resolve(out)), { recursive: true });
    writeFileSync(out, `${JSON.stringify(feed, null, 2)}\n`);
    return 0;
  }
  if (command === "cask" && args.length === 2) {
    const changed = updateCasks(args[0], args[1]);
    console.log(changed ? `Pinned ${changed} cask file(s) to ${args[0]}.` : "Casks already pinned.");
    return 0;
  }
  if (command === "check-release" && args.length === 2) {
    checkRelease(args[0], args[1]);
    return 0;
  }
  if (command === "verify-feeds" && args.length === 2) {
    verifyFeeds(args[0], args[1]);
    return 0;
  }
  console.error("Usage: node tools/packages.mjs winget|windows-feed|cask|check-release|verify-feeds ...");
  return 2;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    process.exitCode = main(process.argv.slice(2));
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
