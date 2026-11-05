#!/usr/bin/env bash
# Type-checks and lints the macOS-only Rust code (Keychain, AppKit, vibrancy,
# cfg(target_os = "macos") branches) from a Linux machine, without a Mac or
# the macOS SDK:
#
#   npm run check:macos            # clippy for aarch64-apple-darwin
#   npm run check:macos -- -- -D warnings   # extra clippy args (CI enforces this)
#
# `cargo clippy` never links, but a few dependencies (objc2-exception-helper,
# ring, mac-notification-sys) compile Objective-C/C in their build scripts and
# need Apple headers for that. This script stands in for the C compiler and
# emits empty Mach-O objects instead, which is enough for type-checking. The
# result is NOT a usable build: run the real thing on macOS (CI does).
#
# Needs: `rustup target add aarch64-apple-darwin`, clang and llvm-ar.
set -euo pipefail

# When cc-rs calls us as the C compiler, compile an empty translation unit for
# the requested target instead of the real source. Anything else (cc-rs
# probing the compiler, for example) goes to clang unchanged.
if [ "${USAGEBAR_STUB_CC:-}" = "1" ]; then
  out=""; compile=0; target="--target=arm64-apple-macosx"
  args=("$@")
  for ((i = 0; i < ${#args[@]}; i++)); do
    case "${args[i]}" in
      -c) compile=1 ;;
      -o) out="${args[i + 1]:-}" ;;
      --target=*) target="${args[i]}" ;;
    esac
  done
  if [ "$compile" = 1 ] && [ -n "$out" ]; then
    exec clang "$target" -c -x c /dev/null -o "$out"
  fi
  exec clang "$@"
fi

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="${MACOS_CHECK_TARGET:-aarch64-apple-darwin}"

if [ "$(uname -s)" = "Darwin" ]; then
  # A Mac has the SDK, so the ordinary check is the accurate one.
  exec cargo clippy --manifest-path "$ROOT/src-tauri/Cargo.toml" --all-targets "$@"
fi

for tool in clang llvm-ar; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "check-macos: $tool is required (e.g. apt-get install clang llvm)" >&2
    exit 1
  fi
done
if command -v rustup >/dev/null 2>&1 && ! rustup target list --installed | grep -qx "$TARGET"; then
  echo "check-macos: run 'rustup target add $TARGET' first" >&2
  exit 1
fi

target_env="${TARGET//-/_}"
export USAGEBAR_STUB_CC=1
export "CC_${target_env}=$ROOT/tools/check-macos.sh"
export "AR_${target_env}=llvm-ar"
exec cargo clippy --manifest-path "$ROOT/src-tauri/Cargo.toml" --target "$TARGET" --all-targets "$@"
