#!/usr/bin/env bash
# Swaps the freshly built bundle into /Applications, which is the copy that
# actually runs. Building alone only refreshes src-tauri/target/release/bundle,
# so without this step the menu bar keeps running whatever was installed last.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Cursor (and some CI) redirects Cargo into CARGO_TARGET_DIR. Prefer that so
# we do not install a stale src-tauri/target bundle from an earlier machine build.
TARGET="${CARGO_TARGET_DIR:-$ROOT/src-tauri/target}"
MACOS="$TARGET/release/bundle/macos/UsageBar.app"
NOINDEX="$TARGET/release/bundle/macos.noindex/UsageBar.app"
FALLBACK_MACOS="$ROOT/src-tauri/target/release/bundle/macos/UsageBar.app"
FALLBACK_NOINDEX="$ROOT/src-tauri/target/release/bundle/macos.noindex/UsageBar.app"
INSTALLED="/Applications/UsageBar.app"

if [ -d "$MACOS" ]; then
  BUILT="$MACOS"
elif [ -d "$NOINDEX" ]; then
  BUILT="$NOINDEX"
elif [ -d "$FALLBACK_MACOS" ]; then
  BUILT="$FALLBACK_MACOS"
elif [ -d "$FALLBACK_NOINDEX" ]; then
  BUILT="$FALLBACK_NOINDEX"
else
  echo "install-app: no build at $MACOS — run 'npm run tauri build -- --bundles app' first" >&2
  exit 1
fi
echo "Using bundle $BUILT"

# The running copy holds its menu bar item open; replacing it underneath leaves
# a stale icon behind.
if pgrep -f "$INSTALLED/Contents/MacOS/usagebar" >/dev/null; then
  echo "Quitting the running UsageBar…"
  pkill -f "$INSTALLED/Contents/MacOS/usagebar" || true
  sleep 1
fi

rm -rf "$INSTALLED"
cp -R "$BUILT" "$INSTALLED"
"$ROOT/tools/hide-build-from-spotlight.sh"
echo "Installed $(date -r "$INSTALLED/Contents/MacOS/usagebar" '+%b %e %H:%M') build to $INSTALLED"
open "$INSTALLED"
