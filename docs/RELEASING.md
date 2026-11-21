# Releasing UsageBar

Releases are built, signed, and notarized by `.github/workflows/release.yml`
when a `v*` tag is pushed. The certificate setup below is a one-time cost;
after that a release is one `git tag`.

## One-time setup

### 1. Enroll in the Apple Developer Program

<https://developer.apple.com/programs/> — $99/year. Individual enrollment
usually clears within a day or two, sometimes longer if Apple asks for ID.
Nothing below works until enrollment is active.

### 2. Create a Developer ID Application certificate

This is the certificate type for apps distributed outside the App Store.
(Developer ID *Installer* is for `.pkg` files and is not needed here.)

1. Keychain Access → Certificate Assistant → **Request a Certificate from a
   Certificate Authority**. Enter your email, choose **Saved to disk**, and
   save the `.certSigningRequest` file.
2. <https://developer.apple.com/account/resources/certificates/list> → **+** →
   **Developer ID Application** → upload the request → download the `.cer`.
3. Double-click the `.cer` to install it into your login keychain.

### 3. Export the certificate for CI

In Keychain Access, expand the new certificate so its private key is included,
select both rows, right-click → **Export 2 items** → save as `certificate.p12`
and set a password.

```bash
base64 -i certificate.p12 | pbcopy
```

Then read the identity string you will need:

```bash
security find-identity -v -p codesigning
```

Copy the full name, which looks like
`Developer ID Application: Your Name (TEAMID)`.

### 4. Create an App Store Connect API key for notarization

An API key is used instead of your Apple ID password: it is scoped, revocable,
and does not expire when you change your password.

1. <https://appstoreconnect.apple.com/access/integrations/api> → **Team Keys**
   → **+**, with the **Developer** role (or higher).
2. Download the `.p8` file. **Apple only lets you download it once.**
3. Note the **Key ID** and the **Issuer ID** shown on that page.

### 5. Add the GitHub secrets

Repository → Settings → Secrets and variables → Actions:

| Secret | Value |
| --- | --- |
| `APPLE_CERTIFICATE` | base64 of `certificate.p12` (step 3) |
| `APPLE_CERTIFICATE_PASSWORD` | the password you set when exporting |
| `APPLE_SIGNING_IDENTITY` | `Developer ID Application: Your Name (TEAMID)` |
| `APPLE_API_KEY` | Key ID (step 4) |
| `APPLE_API_ISSUER` | Issuer ID (step 4) |
| `APPLE_API_KEY_P8` | full contents of the `.p8` file, including the BEGIN/END lines |
| `TAURI_SIGNING_PRIVATE_KEY` | contents of the updater private key (step 6) |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | the password you set, if the key has one — omit it if blank |

### 6. Generate the updater signing key

Self-updates are verified with a minisign signature: the private key signs
`latest.json`'s artifacts in CI, and the app checks them against the public
key baked into `src-tauri/tauri.conf.json`.

```bash
npx tauri signer generate -w ~/.tauri/usagebar-updater.key
```

Paste the contents of `~/.tauri/usagebar-updater.key` into the
`TAURI_SIGNING_PRIVATE_KEY` secret, and keep a copy somewhere safe — **if the
key is lost you must rotate the pair**: generate a fresh key, commit the new
pubkey to `tauri.conf.json`, and every build signed with the old key stops
auto-updating (those users land back on manual downloads).

## Cutting a release

The version lives in six fields across five files (`package.json`,
`package-lock.json`, `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock` and
`src-tauri/tauri.conf.json`). The casks are deliberately not among them: they
pin a published installer and are updated only after a release is public.
Set the fields at once:

```bash
npm run versions:set -- 0.1.0
```

`npm run versions:check` confirms they agree; CI runs it on every pull
request, and the Release workflow also fails if the pushed tag does not match.
Commit, and tag:

```bash
git tag v0.1.0 && git push origin main --tags
```

Only stable `vM.m.p` tags release. The workflow runs, in order:

1. `preflight`: the tag must be strictly newer than the latest public release
   (so a rerun cannot overwrite or downgrade it) and match the version fields.
   A draft left by a failed run can be retried.
2. `windows`: the signed Windows build (reusable `windows.yml`), producing the
   installer, its `.sig`, `latest-windows.json` and the WinGet manifests zip.
   It needs the `TAURI_SIGNING_PRIVATE_KEY` secrets.
3. `release`: tests, then the universal macOS build, signed, notarised and
   stapled, uploaded to a **draft** release. The Windows files (versioned
   installer, `.sig`, `latest-windows.json`, WinGet zip and the fixed
   `UsageBar_windows_x64-setup.exe` the website links to) are attached to the
   same draft, every required asset and both feeds are checked, and only then
   is the release made public with `gh release edit --draft=false --latest`.
4. `cask`: pins both `Casks/usagebar.rb` files to the published `.dmg` digest
   and pushes a normal commit to the default branch. If branch protection
   refuses it, download the dmg and run
   `node tools/packages.mjs cask <version> <dmg>` locally, then commit.

Running copies check each feed (`latest.json`, `latest-windows.json`) on a six-hour cycle; the update packages are signed with the updater key, the JSON files themselves are not. macOS gains **Restart to Update**;
Windows gains **Install Update and Restart**, and never installs without that
click. The Authenticode certificate (SmartScreen) is separate from the Tauri
updater signature and is not part of this flow.

## Homebrew and WinGet

This repository is the Homebrew tap:

```bash
brew tap abdelrahmanmagdii/usagebar https://github.com/abdelrahmanmagdii/usage-tracker.git
brew install --cask usagebar
```

To check a digest by hand:

```bash
curl -sL "https://github.com/abdelrahmanmagdii/usage-tracker/releases/download/v$VERSION/UsageBar_${VERSION}_universal.dmg" | shasum -a 256
```

WinGet manifests are generated from the real installer
(`node tools/packages.mjs winget <version> <installer> <out-dir>`) and checked
against Microsoft's schemas by `tools/check-winget.py` in CI. Nothing is
submitted to the community repository automatically; the release zip is there
for a manual pull request to `microsoft/winget-pkgs`. To try it locally:
`winget install --manifest <dir>` (needs the `LocalManifestFiles` setting).

## Verifying a build locally

To sign a local build, export the same values and build as usual:

```bash
export APPLE_SIGNING_IDENTITY="Developer ID Application: Your Name (TEAMID)"
export APPLE_API_KEY=... APPLE_API_ISSUER=... APPLE_API_KEY_PATH=/path/to/key.p8
npm run tauri build
```

Without `TAURI_SIGNING_PRIVATE_KEY` in the environment, `npm run tauri build`
(via `tools/tauri.mjs`) turns `bundle.createUpdaterArtifacts` off for that
build and says so, since Tauri otherwise refuses to bundle. Export the key (and
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`) to also produce the `.app.tar.gz` and
`.sig` updater bundle. The fallback is off in CI (`CI=true`), so a release
missing the key fails instead of shipping without `latest.json`.

After the build, `tools/tauri.mjs` moves the `.app` into `bundle/macos.noindex/`
so Spotlight only lists the installed copy. Check the result:

```bash
codesign -dv --verbose=4 src-tauri/target/release/bundle/macos.noindex/UsageBar.app
spctl -a -vvv -t install src-tauri/target/release/bundle/macos.noindex/UsageBar.app
```

`spctl` should report **accepted** with source *Notarized Developer ID*. That
is the state where a downloaded copy opens without any Gatekeeper warning.

## Notes

- The app needs no special entitlements. Notarization requires the hardened
  runtime, which the Tauri bundler enables; UsageBar only spawns separate
  child processes (`codex`, `security`) and makes ordinary network requests,
  none of which the hardened runtime restricts.
- The macOS build produces the DMG with a Finder AppleScript step. On a local
  machine that may prompt for automation permission the first time; CI has no
  such prompt.
- Notarization typically takes a few minutes. If Apple rejects a build, the
  workflow log contains the submission ID; `xcrun notarytool log <id>` explains
  why.

## Mac App Store (not pursuing)

GitHub Releases are the distribution path (Developer ID + notarized
`release.yml`). Apple will not grant the sandbox temporary exceptions the
store binary needs to read other tools’ logins (Guideline 2.4.5(i)).

`./tools/mas-build.sh` and `store/APP_STORE.md` remain in the repo in case that
changes. Do not ship MAS-sandboxed entitlements on the notarized `.dmg`.

