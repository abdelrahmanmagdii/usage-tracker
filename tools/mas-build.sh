#!/usr/bin/env bash
# Build a Mac App Store .app and print productbuild / upload steps.
# Needs APPLE_TEAM_ID. Optional: APPLE_MAS_PROFILE, APPLE_MAS_IDENTITY.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TEAM_ID="${APPLE_TEAM_ID:-}"
PROFILE="${APPLE_MAS_PROFILE:-}"

if [[ -z "$TEAM_ID" ]]; then
  echo "Set APPLE_TEAM_ID to your 10-character Team ID from developer.apple.com." >&2
  exit 1
fi

TEMPLATE="$ROOT/src-tauri/Entitlements.mas.plist"
GENERATED="$ROOT/src-tauri/Entitlements.mas.generated.plist"
sed "s/TEAMID/${TEAM_ID}/g" "$TEMPLATE" > "$GENERATED"

CONFIG="$ROOT/src-tauri/tauri.macos-app-store.conf.json"
TMP_CONFIG="$(mktemp)"
python3 - "$CONFIG" "$TMP_CONFIG" <<'PY'
import json, sys
src, dest = sys.argv[1], sys.argv[2]
with open(src) as f:
    data = json.load(f)
data["bundle"]["macOS"]["entitlements"] = "Entitlements.mas.generated.plist"
with open(dest, "w") as f:
    json.dump(data, f, indent=2)
    f.write("\n")
PY

if [[ -n "$PROFILE" ]]; then
  cp "$PROFILE" "$ROOT/src-tauri/embedded.provisionprofile"
fi

cd "$ROOT"
npx tauri build --bundles app --target universal-apple-darwin --config "$TMP_CONFIG"
rm -f "$TMP_CONFIG"

APP="$ROOT/src-tauri/target/universal-apple-darwin/release/bundle/macos/UsageBar.app"
PKG="$ROOT/src-tauri/target/universal-apple-darwin/release/bundle/macos/UsageBar.pkg"
if [[ ! -d "$APP" ]]; then
  APP="$ROOT/src-tauri/target/release/bundle/macos/UsageBar.app"
  PKG="$ROOT/src-tauri/target/release/bundle/macos/UsageBar.pkg"
fi

echo "App: $APP"
echo "Next:"
echo "  productbuild --sign \"3rd Party Mac Developer Installer\" --component \"$APP\" /Applications \"$PKG\""
echo "  xcrun altool --upload-app --type macos --file \"$PKG\" --apiKey \"\$APPLE_API_KEY\" --apiIssuer \"\$APPLE_API_ISSUER\""
echo "Paste store/APP_STORE.md into App Store Connect before submitting."
