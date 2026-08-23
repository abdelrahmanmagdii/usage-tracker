#!/usr/bin/env bash
# Keep Spotlight from listing the Tauri build-tree copy as a second UsageBar.
# That copy lives in bundle/macos/, which is why Spotlight labels it "macos".
# Folders named *.noindex are skipped by Spotlight; /Applications stays the
# one app that search should open.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="$ROOT/src-tauri/target"
MACOS="$TARGET/release/bundle/macos/UsageBar.app"
NOINDEX_DIR="$TARGET/release/bundle/macos.noindex"
NOINDEX="$NOINDEX_DIR/UsageBar.app"
LSREGISTER="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"

mkdir -p "$TARGET"
touch "$TARGET/.metadata_never_index"

if [ -d "$MACOS" ]; then
  mkdir -p "$NOINDEX_DIR"
  rm -rf "$NOINDEX"
  mv "$MACOS" "$NOINDEX"
fi

if [ -x "$LSREGISTER" ]; then
  "$LSREGISTER" -u "$MACOS" >/dev/null 2>&1 || true
  if [ -d "$NOINDEX" ]; then
    "$LSREGISTER" -u "$NOINDEX" >/dev/null 2>&1 || true
  fi
  if [ -d "/Applications/UsageBar.app" ]; then
    "$LSREGISTER" -f "/Applications/UsageBar.app" >/dev/null 2>&1 || true
  fi
fi
