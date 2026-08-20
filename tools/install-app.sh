#!/usr/bin/env bash
# Swaps the freshly built bundle into /Applications, which is the copy that
# actually runs. Building alone only refreshes src-tauri/target/release/bundle,
# so without this step the menu bar keeps running whatever was installed last.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUILT="$ROOT/src-tauri/target/release/bundle/macos/UsageBar.app"
INSTALLED="/Applications/UsageBar.app"

if [ ! -d "$BUILT" ]; then
  echo "install-app: no build at $BUILT — run 'npm run tauri build -- --bundles app' first" >&2
  exit 1
fi

# The running copy holds its menu bar item open; replacing it underneath leaves
# a stale icon behind.
if pgrep -f "$INSTALLED/Contents/MacOS/usagebar" >/dev/null; then
  echo "Quitting the running UsageBar…"
  pkill -f "$INSTALLED/Contents/MacOS/usagebar" || true
  sleep 1
fi

rm -rf "$INSTALLED"
cp -R "$BUILT" "$INSTALLED"
echo "Installed $(date -r "$INSTALLED/Contents/MacOS/usagebar" '+%b %e %H:%M') build to $INSTALLED"
open "$INSTALLED"
