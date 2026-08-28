#!/usr/bin/env bash
# Build a Mac App Store .app / .pkg and print upload steps.
# Needs APPLE_TEAM_ID. Optional: APPLE_MAS_PROFILE, APPLE_SIGNING_IDENTITY.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TEAM_ID="${APPLE_TEAM_ID:-}"
PROFILE="${APPLE_MAS_PROFILE:-}"
EMBEDDED="$ROOT/src-tauri/embedded.provisionprofile"

if [[ -z "$TEAM_ID" ]]; then
  echo "Set APPLE_TEAM_ID to your 10-character Team ID from developer.apple.com." >&2
  exit 1
fi
if [[ ! "$TEAM_ID" =~ ^[A-Z0-9]{10}$ ]]; then
  echo "APPLE_TEAM_ID must be the 10-character Team ID, not a placeholder." >&2
  exit 1
fi

TEMPLATE="$ROOT/src-tauri/Entitlements.mas.plist"
GENERATED="$ROOT/src-tauri/Entitlements.mas.generated.plist"
sed "s/TEAMID/${TEAM_ID}/g" "$TEMPLATE" > "$GENERATED"
if grep -q TEAMID "$GENERATED"; then
  echo "Entitlements still contain TEAMID after substitution. Refusing to build." >&2
  exit 1
fi

if [[ -n "$PROFILE" && "$PROFILE" != "$EMBEDDED" ]]; then
  cp "$PROFILE" "$EMBEDDED"
fi
if [[ ! -f "$EMBEDDED" ]]; then
  echo "No provisioning profile at $EMBEDDED. Set APPLE_MAS_PROFILE to the .provisionprofile." >&2
  exit 1
fi
# Safari/Downloads stamp com.apple.quarantine; App Store rejects that (91109).
xattr -c "$EMBEDDED" 2>/dev/null || true

IDENTITY="${APPLE_SIGNING_IDENTITY:-}"
if [[ -z "$IDENTITY" ]]; then
  IDENTITY="$(security find-identity -v -p codesigning | awk -F'"' '/3rd Party Mac Developer Application/ {print $2; exit}')"
fi
if [[ -z "$IDENTITY" ]]; then
  echo "No 3rd Party Mac Developer Application identity found. Set APPLE_SIGNING_IDENTITY." >&2
  exit 1
fi
INSTALLER="${APPLE_MAS_INSTALLER_IDENTITY:-}"
if [[ -z "$INSTALLER" ]]; then
  INSTALLER="$(security find-identity -v | awk -F'"' '/3rd Party Mac Developer Installer/ {print $2; exit}')"
fi

cd "$ROOT"
SYSROOT="$(rustc --print sysroot)"
if [[ -d "$SYSROOT/lib/rustlib/x86_64-apple-darwin" ]]; then
  TARGET=universal-apple-darwin
else
  echo "No x86_64 Rust std; building Apple Silicon only (macOS 12+)." >&2
  TARGET=aarch64-apple-darwin
fi
npx tauri build --bundles app --target "$TARGET" --config src-tauri/tauri.macos-app-store.conf.json

APP="$ROOT/src-tauri/target/$TARGET/release/bundle/macos/UsageBar.app"
if [[ ! -d "$APP" ]]; then
  APP="$ROOT/src-tauri/target/release/bundle/macos/UsageBar.app"
fi
PKG="$(dirname "$APP")/UsageBar.pkg"
if [[ ! -d "$APP" ]]; then
  echo "mas-build: UsageBar.app was not produced." >&2
  exit 1
fi

cp "$EMBEDDED" "$APP/Contents/embedded.provisionprofile"
# Downloads/Safari stamp quarantine; error 91109 rejects that in the payload.
xattr -cr "$APP" 2>/dev/null || true
xattr -c "$APP/Contents/embedded.provisionprofile" 2>/dev/null || true
CLEAN="$(mktemp -d)/UsageBar.app"
ditto --norsrc --noextattr --noqtn "$APP" "$CLEAN"
codesign --force --options runtime --entitlements "$GENERATED" --sign "$IDENTITY" "$CLEAN/Contents/MacOS/usagebar"
codesign --force --options runtime --entitlements "$GENERATED" --sign "$IDENTITY" "$CLEAN"

if [[ -n "$INSTALLER" ]]; then
  rm -f "$PKG"
  productbuild --sign "$INSTALLER" --component "$CLEAN" /Applications "$PKG"
  xattr -c "$PKG" 2>/dev/null || true
  if xattr -lr "$CLEAN" 2>/dev/null | grep -q com.apple.quarantine; then
    echo "mas-build: quarantine attribute still present in the app. Refusing to ship." >&2
    xattr -lr "$CLEAN" | grep quarantine >&2 || true
    exit 1
  fi
  echo "Pkg: $PKG"
else
  echo "No 3rd Party Mac Developer Installer identity; skipped productbuild." >&2
fi
rm -rf "$(dirname "$CLEAN")"

echo "App: $APP"
echo "Next: upload the .pkg with Transporter, then paste store/APP_STORE.md into App Store Connect."
