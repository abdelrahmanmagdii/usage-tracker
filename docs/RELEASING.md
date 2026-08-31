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

## Cutting a release

```bash
npm version 0.1.0 --no-git-tag-version
```

Then match that version in `src-tauri/tauri.conf.json` and
`src-tauri/Cargo.toml`, commit, and tag:

```bash
git tag v0.1.0 && git push origin main --tags
```

The workflow runs the test suite, builds a universal binary for Intel and
Apple Silicon, signs it, sends it to Apple for notarization, staples the
ticket, and opens a **draft** release with the `.dmg` attached. Review the
draft and publish it.

## Verifying a build locally

To sign a local build, export the same values and build as usual:

```bash
export APPLE_SIGNING_IDENTITY="Developer ID Application: Your Name (TEAMID)"
export APPLE_API_KEY=... APPLE_API_ISSUER=... APPLE_API_KEY_PATH=/path/to/key.p8
npm run tauri build
```

Check the result:

```bash
codesign -dv --verbose=4 src-tauri/target/release/bundle/macos/UsageBar.app
spctl -a -vvv -t install src-tauri/target/release/bundle/macos/UsageBar.app
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

## Mac App Store

Direct GitHub releases stay Developer ID + notarized (this workflow). The store
build is sandboxed and is **not** produced by `release.yml`.

One-time in App Store Connect: create the Mac app with bundle ID
`com.usagebar.app`, attach a Mac App Store Connect provisioning profile, and
paste the fields in [store/APP_STORE.md](../store/APP_STORE.md). Privacy and
support URLs are the GitHub Pages site (workflow `.github/workflows/pages.yml`).

Then, with `APPLE_TEAM_ID` and the profile on disk:

```bash
export APPLE_TEAM_ID=XXXXXXXXXX
export APPLE_MAS_PROFILE=/path/to/UsageBar.provisionprofile
./tools/mas-build.sh
```

Upload the `.pkg` with Transporter or `altool`. Capture 1280×800 (16:10) shots
from `store/screenshots/*.html`. The sandbox uses a few read-only temporary
exceptions so Claude/Cursor/OpenCode files and the Homebrew Codex binary can
still be read; Keychain and SQLite use public APIs. Answer App Review from
`store/APP_STORE.md`.

GitHub `.dmg` builds must keep using a **Developer ID Application** identity.
Do not ship the MAS-sandboxed entitlements on the notarized download.

