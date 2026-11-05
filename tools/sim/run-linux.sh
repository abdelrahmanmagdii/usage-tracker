#!/usr/bin/env bash
# Builds and runs the REAL UsageBar Tauri binary on Linux under Xvfb, in demo
# mode, and screenshots its window. Linux is not a supported target; this is
# for looking at the real WebKitGTK webview + Rust backend from a Linux box or
# container. See docs/SIMULATION.md ("Real app on Linux").
#
#   npm run sim:linux                       # dev build, frontend from Vite
#   npm run sim:linux -- --embedded         # frontend built into the binary (real CSP)
#   npm run sim:linux -- --no-demo          # real providers, isolated $HOME
#   npm run sim:linux -- --binary path/to/usagebar --skip-build
#
# Needs: cargo + the GTK/WebKitGTK/appindicator dev packages, Xvfb,
# dbus-run-session (dbus), ImageMagick `import`/`convert`.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT="$ROOT/sim-shots/linux"
PORT=""
MODE=dev
DEMO=1
BUILD=1
BINARY=""
HOLD=0

while [ $# -gt 0 ]; do
  case "$1" in
    --embedded) MODE=embedded ;;
    --no-demo) DEMO=0 ;;
    --skip-build) BUILD=0 ;;
    --binary) BINARY="$2"; BUILD=0; shift ;;
    --port) PORT="$2"; PORT_GIVEN=1; shift ;;
    --out) OUT="$2"; shift ;;
    --hold) HOLD="$2"; shift ;;
    -h|--help) sed -n '2,16p' "$0"; exit 0 ;;
    *) echo "unknown option $1" >&2; exit 2 ;;
  esac
  shift
done

for tool in Xvfb dbus-run-session import convert; do
  command -v "$tool" >/dev/null || { echo "missing $tool (apt install xvfb dbus imagemagick)" >&2; exit 2; }
done

free_port() { node -e 'const s=require("net").createServer().listen(0,"127.0.0.1",()=>{console.log(s.address().port);s.close()})'; }
mkdir -p "$OUT" && OUT="$(cd "$OUT" && pwd)"
# A dev binary has its Vite port baked in (devUrl); reuse it with --skip-build.
if [ -z "$PORT" ] && [ "$BUILD" = 0 ] && [ -f "${BINARY:-$OUT/usagebar}.devport" ]; then
  PORT="$(cat "${BINARY:-$OUT/usagebar}.devport")"
fi
[ -n "$PORT" ] || PORT="$(free_port)"
CONTROL_PORT="$(free_port)"
mkdir -p "$OUT"
LOG="$OUT/linux-run.log"
: > "$LOG"
say() { echo "sim:linux: $*" | tee -a "$LOG"; }

PIDS=()
APP_PID=""
APP_HOME=""
cleanup() {
  # The app runs in its own process group with its D-Bus session, which
  # activates xdg-desktop-portal / xdg-document-portal: take them all down.
  if [ -n "$APP_PID" ]; then kill -TERM -- "-$APP_PID" 2>/dev/null || true; fi
  for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null || true; done
  sleep 0.5
  if [ -n "$APP_PID" ]; then kill -KILL -- "-$APP_PID" 2>/dev/null || true; fi
  # The document portal leaves a FUSE mount under the isolated runtime dir.
  if [ -n "$APP_HOME" ]; then
    grep -o " $APP_HOME[^ ]*" /proc/mounts 2>/dev/null | while read -r mnt; do
      fusermount3 -u "$mnt" 2>/dev/null || fusermount -u "$mnt" 2>/dev/null || true
    done
  fi
  wait 2>/dev/null || true
}
trap 'rc=$?; set +e; cleanup; exit $rc' EXIT

# ---- build -------------------------------------------------------------------
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/src-tauri/target}"
if [ "$BUILD" = 1 ]; then
  if [ "$MODE" = embedded ]; then
    say "building frontend (npm run build) and the binary with the frontend embedded"
    (cd "$ROOT" && npm run build) >>"$LOG" 2>&1
    cargo build --manifest-path "$ROOT/src-tauri/Cargo.toml" --features tauri/custom-protocol >>"$LOG" 2>&1
  else
    # A debug build loads `devUrl`; point it at this run's Vite port.
    say "building the binary (debug, devUrl http://localhost:$PORT)"
    TAURI_CONFIG="{\"build\":{\"devUrl\":\"http://localhost:$PORT\"}}" \
      cargo build --manifest-path "$ROOT/src-tauri/Cargo.toml" >>"$LOG" 2>&1
  fi
  # The target dir may be shared; keep our own copy of what we just built.
  cp "$CARGO_TARGET_DIR/debug/usagebar" "$OUT/usagebar"
  BINARY="$OUT/usagebar"
  if [ "$MODE" = dev ]; then echo "$PORT" > "$BINARY.devport"; else rm -f "$BINARY.devport"; fi
fi
[ -n "$BINARY" ] || BINARY="$OUT/usagebar"
[ -x "$BINARY" ] || { echo "no binary at $BINARY (drop --skip-build)" >&2; exit 2; }

# ---- frontend ----------------------------------------------------------------
if [ "$MODE" = dev ] && [ "$BUILD" = 0 ] && [ ! -f "$BINARY.devport" ] && [ -z "${PORT_GIVEN:-}" ]; then
  say "warning: dev binary without a recorded devUrl port; pass --port or --embedded"
fi
if [ "$MODE" = dev ]; then
  say "starting Vite on :$PORT"
  # exec node directly (not npx) so the PID we kill is Vite itself.
  (cd "$ROOT" && exec node "$ROOT/node_modules/vite/bin/vite.js" --port "$PORT" --strictPort --host 127.0.0.1) >>"$LOG" 2>&1 &
  PIDS+=($!)
  for _ in $(seq 1 60); do curl -sf "http://localhost:$PORT/" >/dev/null && break; sleep 0.5; done
fi

# ---- display -------------------------------------------------------------------
DISPLAY_NUM=""
for n in $(seq 90 140); do
  if [ ! -e "/tmp/.X11-unix/X$n" ] && [ ! -e "/tmp/.X$n-lock" ]; then DISPLAY_NUM=$n; break; fi
done
[ -n "$DISPLAY_NUM" ] || { echo "no free X display" >&2; exit 1; }
Xvfb ":$DISPLAY_NUM" -screen 0 1280x800x24 -nolisten tcp >>"$LOG" 2>&1 &
PIDS+=($!)
for _ in $(seq 1 40); do [ -e "/tmp/.X11-unix/X$DISPLAY_NUM" ] && break; sleep 0.25; done
say "Xvfb on :$DISPLAY_NUM"

# ---- app -----------------------------------------------------------------------
APP_HOME="$OUT/home"
rm -rf "$APP_HOME" 2>/dev/null || true
mkdir -p "$APP_HOME/run" && chmod 700 "$APP_HOME/run"
ARGS=()
ENVS=(DISPLAY=":$DISPLAY_NUM" HOME="$APP_HOME" XDG_CONFIG_HOME="$APP_HOME/.config"
  XDG_DATA_HOME="$APP_HOME/.local/share" XDG_CACHE_HOME="$APP_HOME/.cache" XDG_RUNTIME_DIR="$APP_HOME/run"
  USAGEBAR_DEMO_PORT="$CONTROL_PORT" NO_AT_BRIDGE=1)
if [ "$DEMO" = 1 ]; then ARGS+=(--demo); fi
say "launching $BINARY ${ARGS[*]:-} (isolated HOME=$APP_HOME)"
setsid env "${ENVS[@]}" dbus-run-session -- "$BINARY" "${ARGS[@]}" >>"$LOG" 2>&1 &
APP_PID=$!

control() {
  # One command per connection, one reply line (src-tauri/src/demo.rs).
  local reply=""
  exec 3<>"/dev/tcp/127.0.0.1/$CONTROL_PORT" || return 1
  printf '%s\n' "$1" >&3
  IFS= read -r -t 8 reply <&3 || true
  exec 3<&- 3>&-
  echo "$reply"
}

shoot() {
  local name="$1"
  import -display ":$DISPLAY_NUM" -window root "$OUT/$name-screen.png"
  if [ -n "${GEOM:-}" ]; then
    # GEOMETRY x y w h (outer, physical px)
    read -r gx gy gw gh _ <<<"$GEOM"
    convert "$OUT/$name-screen.png" -crop "${gw}x${gh}+${gx}+${gy}" +repage "$OUT/$name.png"
  fi
  say "screenshot $OUT/$name.png"
}

# The first frame of a transparent WebKitGTK window can take a while to reach
# Xvfb's framebuffer (software GL, no compositor): retry until it is not black.
shoot_painted() {
  local name="$1" mean=0
  for _ in $(seq 1 20); do
    shoot "$name" >/dev/null
    mean="$(convert "$OUT/$name.png" -format '%[fx:mean]' info: 2>/dev/null || echo 0)"
    awk -v m="$mean" 'BEGIN { exit !(m > 0.05) }' && break
    sleep 1
  done
  say "screenshot $OUT/$name.png (mean brightness $mean)"
}

if [ "$DEMO" = 1 ]; then
  for _ in $(seq 1 120); do
    kill -0 "$APP_PID" 2>/dev/null || { say "app exited early (see $LOG)"; exit 1; }
    [ "$(control PING 2>/dev/null)" = "PONG demo" ] && break
    sleep 0.5
  done
  say "control port: $(control PING)"
  say "SHOW: $(control SHOW)"
  sleep 3
  GEOM="$(control 'GEOMETRY main' | sed -n 's/^GEOMETRY //p')"
  say "main window geometry: ${GEOM:-unknown}"
  shoot_painted linux-main
  control "SCROLL 700" >/dev/null; sleep 1.5; shoot linux-scrolled
  control SCROLL_TOP >/dev/null; control SETTINGS >/dev/null; sleep 1.5; shoot linux-settings
  control CLOSE_SETTINGS >/dev/null
  say "VISIBLE: $(control VISIBLE)"
  if [ "$HOLD" -gt 0 ]; then say "holding for ${HOLD}s on :$DISPLAY_NUM"; sleep "$HOLD"; fi
  control QUIT >/dev/null || true
  sleep 1
else
  sleep 15
  kill -0 "$APP_PID" 2>/dev/null || { say "app exited early (see $LOG)"; exit 1; }
  GEOM="0 0 1280 800"
  shoot_painted linux-nodemo
  if [ "$HOLD" -gt 0 ]; then sleep "$HOLD"; fi
fi
say "done; log at $LOG"
