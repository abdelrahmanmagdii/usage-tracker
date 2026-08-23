#!/usr/bin/env bash
# Runs the Tauri CLI, then hides the build-tree .app from Spotlight after a
# bundle so search only lists /Applications/UsageBar.app.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
"$ROOT/node_modules/.bin/tauri" "$@"
if [ "${1:-}" = "build" ]; then
  "$ROOT/tools/hide-build-from-spotlight.sh"
fi
