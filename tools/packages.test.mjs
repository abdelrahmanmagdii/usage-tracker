import { test } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  checkRelease,
  compareVersions,
  installerName,
  pinCask,
  readArtifact,
  updateCasks,
  validateTauriSignature,
  verifyFeeds,
  windowsFeed,
  wingetManifests,
  writeWingetManifests,
} from "./packages.mjs";

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const sha = (bytes) => createHash("sha256").update(bytes).digest("hex");

function scratch() {
  return mkdtempSync(path.join(tmpdir(), "usagebar-packages-"));
}

/** A synthetic minisign signature file, base64-encoded the way Tauri writes it. */
function signature(comment = "signature from tauri secret key") {
  const body = [
    `untrusted comment: ${comment}`,
    Buffer.alloc(74, 1).toString("base64"),
    "trusted comment: timestamp:1\tfile:UsageBar.exe",
    Buffer.alloc(64, 2).toString("base64"),
    "",
  ].join("\n");
  return Buffer.from(body).toString("base64");
}

test("versions compare numerically and only stable M.m.p is accepted", () => {
  assert.ok(compareVersions("0.10.0", "0.9.9") > 0);
  assert.equal(compareVersions("1.2.3", "1.2.3"), 0);
  for (const bad of ["1.2", "v1.2.3", "1.2.3-beta.1", "01.2.3", "1.2.3.4", ""]) {
    assert.throws(() => compareVersions(bad, "1.0.0"), /stable version/, bad);
  }
});

test("a release must be a stable tag strictly newer than the public one", () => {
  checkRelease("v0.2.6", "v0.2.5");
  checkRelease("v0.10.0", "v0.9.9");
  assert.throws(() => checkRelease("v0.2.5", "v0.2.5"), /not newer/);
  assert.throws(() => checkRelease("v0.2.4", "v0.2.5"), /not newer/);
  assert.throws(() => checkRelease("v0.2.6-rc1", "v0.2.5"), /stable/);
  assert.throws(() => checkRelease("0.2.6", "v0.2.5"), /stable/);
  assert.throws(() => checkRelease("v0.2.6", "latest"), /stable/);
});

test("artifacts must have the exact name and real, non-empty content", () => {
  const dir = scratch();
  try {
    const name = installerName("1.2.3");
    assert.equal(name, "UsageBar_1.2.3_x64-setup.exe");
    writeFileSync(path.join(dir, name), "bytes");
    assert.equal(readArtifact(path.join(dir, name), name).sha256, sha("bytes"));
    writeFileSync(path.join(dir, "empty.exe"), "");
    assert.throws(() => readArtifact(path.join(dir, "empty.exe"), "empty.exe"), /non-empty/);
    assert.throws(() => readArtifact(path.join(dir, name), "UsageBar_1.2.4_x64-setup.exe"), /named/);
    mkdirSync(path.join(dir, "sub", name), { recursive: true });
    assert.throws(() => readArtifact(path.join(dir, "sub", name), name), /regular/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("WinGet manifests have the community layout, identity and installer facts", () => {
  const dir = scratch();
  try {
    const installer = path.join(dir, "UsageBar_1.2.3_x64-setup.exe");
    writeFileSync(installer, "installer bytes");
    const out = writeWingetManifests("1.2.3", installer, path.join(dir, "out"));
    assert.equal(out, path.join(dir, "out", "manifests", "a", "AbdelrahmanAmer", "UsageBar", "1.2.3"));
    const read = (suffix) => readFileSync(path.join(out, `AbdelrahmanAmer.UsageBar${suffix}.yaml`), "utf8");
    assert.match(read(""), /^PackageIdentifier: AbdelrahmanAmer\.UsageBar$/m);
    assert.match(read(""), /^ManifestType: version$/m);
    assert.match(read(".locale.en-US"), /^ManifestType: defaultLocale$/m);
    assert.match(read(".locale.en-US"), /^PackageLocale: en-US$/m);
    const installerYaml = read(".installer");
    assert.match(installerYaml, /^ManifestVersion: 1\.6\.0$/m);
    assert.match(installerYaml, /^InstallerType: nullsoft$/m);
    assert.match(installerYaml, /^Scope: user$/m);
    assert.match(installerYaml, /^UpgradeBehavior: install$/m);
    assert.match(installerYaml, /^ProductCode: UsageBar$/m);
    assert.match(installerYaml, /DisplayName: UsageBar\n {2}ProductCode: UsageBar/);
    assert.match(installerYaml, /Silent: \/S\n {2}SilentWithProgress: \/P/);
    assert.match(installerYaml, /Architecture: x64/);
    assert.match(
      installerYaml,
      /InstallerUrl: https:\/\/github\.com\/abdelrahmanmagdii\/usage-tracker\/releases\/download\/v1\.2\.3\/UsageBar_1\.2\.3_x64-setup\.exe$/m,
    );
    assert.ok(installerYaml.includes(`InstallerSha256: ${sha("installer bytes").toUpperCase()}`));
    assert.doesNotMatch(installerYaml, /Publisher:/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("WinGet manifests refuse a bad version or installer name", () => {
  const dir = scratch();
  try {
    const wrong = path.join(dir, "setup.exe");
    writeFileSync(wrong, "x");
    assert.throws(() => writeWingetManifests("1.2.3", wrong, dir), /named/);
    assert.throws(() => writeWingetManifests("1.2.3-beta", wrong, dir), /stable/);
    assert.deepEqual(Object.keys(wingetManifests("1.2.3", "a".repeat(64))).length, 3);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("the committed 0.2.5 manifests describe the published installer", () => {
  const dir = path.join(repo, "packaging/winget/manifests/a/AbdelrahmanAmer/UsageBar/0.2.5");
  const installerYaml = readFileSync(path.join(dir, "AbdelrahmanAmer.UsageBar.installer.yaml"), "utf8");
  assert.ok(
    installerYaml.includes("InstallerSha256: DBC51F7E5546F22B2044CD7F1755E7D30A08D9F46FA9626536D6BB6D368C2E59"),
  );
  assert.deepEqual(
    wingetManifests("0.2.5", "dbc51f7e5546f22b2044cd7f1755e7d30a08d9f46fa9626536d6bb6d368c2e59")[
      "AbdelrahmanAmer.UsageBar.installer.yaml"
    ],
    installerYaml.replace(/\r\n/g, "\n"),
  );
});

test("the Windows feed names the versioned installer and carries the signature", () => {
  const feed = windowsFeed("1.2.3", signature());
  const url =
    "https://github.com/abdelrahmanmagdii/usage-tracker/releases/download/v1.2.3/UsageBar_1.2.3_x64-setup.exe";
  assert.equal(feed.version, "1.2.3");
  assert.deepEqual(Object.keys(feed.platforms), ["windows-x86_64", "windows-x86_64-nsis"]);
  for (const platform of Object.values(feed.platforms)) {
    assert.equal(platform.url, url);
    assert.equal(platform.signature, signature());
  }
});

test("only a real minisign signature file is accepted", () => {
  assert.equal(validateTauriSignature(`${signature()}\n`), signature());
  for (const bad of [
    "",
    sha("x"),
    "not base64!",
    Buffer.from("just text").toString("base64"),
    signature("minisign encrypted secret key"),
  ]) {
    assert.throws(() => validateTauriSignature(bad), /ignature|base64/, bad);
  }
});

const CASK = `cask "usagebar" do
  version "0.2.5"
  sha256 "${"a".repeat(64)}"
  url "x"
end
`;

test("a cask is pinned after publication, idempotently, never backwards", () => {
  const next = "b".repeat(64);
  const updated = pinCask(CASK, "0.2.6", next);
  assert.match(updated, /version "0\.2\.6"/);
  assert.ok(updated.includes(`sha256 "${next}"`));
  assert.equal(pinCask(updated, "0.2.6", next), updated);
  assert.throws(() => pinCask(updated, "0.2.6", "c".repeat(64)), /rewrite/);
  assert.throws(() => pinCask(updated, "0.2.5", "a".repeat(64)), /downgrade/);
  assert.match(pinCask(CASK.replace(/"a{64}"/, ":no_check"), "0.2.5", next), /sha256 "b{64}"/);
});

test("both cask files are updated together or not at all", () => {
  const root = scratch();
  try {
    for (const file of ["Casks/usagebar.rb", "homebrew-tap/Casks/usagebar.rb"]) {
      mkdirSync(path.dirname(path.join(root, file)), { recursive: true });
      cpSync(path.join(repo, file), path.join(root, file));
    }
    const dmg = path.join(root, "UsageBar_9.9.9_universal.dmg");
    writeFileSync(dmg, "dmg bytes");
    assert.equal(updateCasks("9.9.9", dmg, root), 2);
    assert.equal(updateCasks("9.9.9", dmg, root), 0);
    for (const file of ["Casks/usagebar.rb", "homebrew-tap/Casks/usagebar.rb"]) {
      const text = readFileSync(path.join(root, file), "utf8");
      assert.ok(text.includes(`sha256 "${sha("dmg bytes")}"`));
    }
    const older = path.join(root, "UsageBar_0.1.0_universal.dmg");
    writeFileSync(older, "old");
    assert.throws(() => updateCasks("0.1.0", older, root), /downgrade/);
    assert.throws(() => updateCasks("9.9.9", path.join(root, "UsageBar_9.9.8_universal.dmg"), root));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("the repository casks are a valid, identical, pinned release", () => {
  const texts = ["Casks/usagebar.rb", "homebrew-tap/Casks/usagebar.rb"].map((file) =>
    readFileSync(path.join(repo, file), "utf8").replace(/\r\n/g, "\n"),
  );
  assert.equal(texts[0], texts[1]);
  const version = /^\s*version "([^"]+)"/m.exec(texts[0])?.[1];
  assert.ok(version);
  compareVersions(version, "0.0.0");
  const pinned = /^\s*sha256 "([0-9a-f]{64})"/m.exec(texts[0])?.[1];
  assert.ok(pinned, "the cask pins a real digest");
  assert.doesNotMatch(texts[0], /:no_check|auto_updates/);
  if (version === "0.2.5") {
    assert.equal(pinned, "ffa8f8796f118922480a5753908376e2da67f34a996e065e03a14dfcbaf7ace1");
  }
});

test("the next release is accepted by the cask and CRLF line endings survive", () => {
  const next = "d".repeat(64);
  const crlf = CASK.replace(/\n/g, "\r\n");
  const updated = pinCask(crlf, "0.2.6", next);
  assert.equal(updated, crlf.replace('"0.2.5"', '"0.2.6"').replace(/"a{64}"/, `"${next}"`));
  assert.ok(!updated.replace(/\r\n/g, "").includes("\n"));
});

function feedDir({ version = "1.2.3", mutate = () => {} } = {}) {
  const dir = scratch();
  const sig = signature();
  const exe = installerName(version);
  const base = `https://github.com/abdelrahmanmagdii/usage-tracker/releases/download/v${version}/`;
  const mac = { version, platforms: {} };
  for (const key of ["darwin-aarch64", "darwin-x86_64", "darwin-aarch64-app", "darwin-x86_64-app"]) {
    mac.platforms[key] = { url: `${base}UsageBar_universal.app.tar.gz`, signature: sig };
  }
  const win = windowsFeed(version, sig);
  mutate({ mac, win });
  writeFileSync(path.join(dir, "latest.json"), JSON.stringify(mac));
  writeFileSync(path.join(dir, "latest-windows.json"), JSON.stringify(win));
  writeFileSync(path.join(dir, "UsageBar_universal.app.tar.gz.sig"), `${sig}\n`);
  writeFileSync(path.join(dir, `${exe}.sig`), `${sig}\n`);
  return dir;
}

test("a complete pair of feeds with matching sidecar signatures verifies", () => {
  const dir = feedDir();
  try {
    verifyFeeds("1.2.3", dir);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("feeds with a missing Mac architecture, wrong URL, signature or version are refused", () => {
  const cases = {
    "missing architecture": ({ mac }) => delete mac.platforms["darwin-x86_64"],
    "wrong url": ({ win }) => (win.platforms["windows-x86_64-nsis"].url += "x"),
    "wrong signature": ({ mac }) => (mac.platforms["darwin-aarch64-app"].signature = signature("other")),
    "empty signature": ({ win }) => (win.platforms["windows-x86_64"].signature = ""),
    "mixed version": ({ win }) => (win.version = "1.2.4"),
  };
  for (const [name, mutate] of Object.entries(cases)) {
    const dir = feedDir({ mutate });
    try {
      assert.throws(() => verifyFeeds("1.2.3", dir), undefined, name);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  }
});
