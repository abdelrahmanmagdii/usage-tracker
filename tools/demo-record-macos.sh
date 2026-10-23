#!/usr/bin/env bash
# Record the real UsageBar menu-bar app on a GitHub-hosted macOS runner.
# The app must already be built (src-tauri/target/release/bundle/macos/UsageBar.app).
# Launch is `--demo` / USAGEBAR_DEMO=1, so the meters are sample data.
#
# Usage: tools/demo-record-macos.sh [seconds]
set -euo pipefail

SECS="${1:-40}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/out"
APP_SRC="$ROOT/src-tauri/target/release/bundle/macos/UsageBar.app"
APP="/Applications/UsageBar.app"
MOV="$OUT/usagebar-macos-raw.mov"
mkdir -p "$OUT"

log() { printf '[demo-record] %s\n' "$*"; }

cleanup() {
  cp /tmp/usagebar-demo.log "$OUT/demo.log" 2>/dev/null || true
  cp /tmp/usagebar-demo.port "$OUT/port.txt" 2>/dev/null || true
}
trap cleanup EXIT

demo() {
  python3 - "$1" <<'PY'
import socket, sys
port = int(open("/tmp/usagebar-demo.port").read().strip())
with socket.create_connection(("127.0.0.1", port), 5) as sock:
    sock.sendall((sys.argv[1] + "\n").encode())
    sock.shutdown(socket.SHUT_WR)
    sock.settimeout(5)
    data = b""
    while True:
        try:
            chunk = sock.recv(256)
        except socket.timeout:
            break
        if not chunk:
            break
        data += chunk
        if b"\n" in data:
            break
    sys.stdout.write(data.decode().strip())
PY
}

wake_display() {
  caffeinate -dimsu -t $((SECS + 90)) >/dev/null 2>&1 &
  # A throwaway capture often wakes a sleeping virtual display.
  screencapture -x "$OUT/wake.png" >/dev/null 2>&1 || true
}

grant_tcc() {
  # The runner image already grants Screen Recording and Accessibility to
  # /bin/bash and osascript. If a capture still fails, add this shell too.
  # The system database is SIP-protected; only the user database is writable.
  local db="$HOME/Library/Application Support/com.apple.TCC/TCC.db"
  [[ -f "$db" ]] || return 0
  local shell
  shell="$(command -v bash)"
  sqlite3 "$db" "INSERT OR REPLACE INTO access (service, client, client_type, auth_value, auth_reason, auth_version, csreq, policy_id, indirect_object_identifier_type, indirect_object_identifier, indirect_object_code_identity, flags, last_modified) VALUES ('kTCCServiceScreenCapture','$shell',1,2,4,1,NULL,NULL,0,'UNUSED',NULL,0,CAST(strftime('%s','now') AS INTEGER));" \
    >/dev/null 2>&1 || log "user TCC insert for screencapture was rejected (continuing)"
  sqlite3 "$db" "INSERT OR REPLACE INTO access (service, client, client_type, auth_value, auth_reason, auth_version, csreq, policy_id, indirect_object_identifier_type, indirect_object_identifier, indirect_object_code_identity, flags, last_modified) VALUES ('kTCCServiceAccessibility','$shell',1,2,4,1,NULL,NULL,0,'UNUSED',NULL,0,CAST(strftime('%s','now') AS INTEGER));" \
    >/dev/null 2>&1 || true
}

prepare_desktop() {
  python3 - "$OUT/wallpaper.png" <<'PY'
import struct, sys, zlib
path, w, h = sys.argv[1], 1920, 1080
def chunk(tag, data):
    return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
rows = []
for y in range(h):
    t = y / (h - 1)
    r = int(246 + (228 - 246) * t)
    g = int(243 + (233 - 243) * t)
    b = 255
    rows.append(b"\x00" + bytes((r, g, b)) * w)
raw = zlib.compress(b"".join(rows), 9)
png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) + chunk(b"IDAT", raw) + chunk(b"IEND", b"")
open(path, "wb").write(png)
PY
  defaults write com.apple.finder CreateDesktop -bool false || true
  killall Finder >/dev/null 2>&1 || true
  sleep 1
  osascript <<AS || log "could not set the desktop picture"
tell application "System Events"
  tell every desktop
    set picture to "$OUT/wallpaper.png"
  end tell
end tell
AS
  osascript -e 'tell application "System Events" to set autohide of dock preferences to true' \
    >/dev/null 2>&1 || log "could not autohide the Dock"
  osascript -e 'tell application "System Events" to set visible of (every process whose background only is false and name is not "Finder" and name is not "UsageBar") to false' \
    >/dev/null 2>&1 || true
}

click_tray() {
  osascript <<'AS' || return 1
tell application "System Events" to tell process "UsageBar"
  if (count of menu bars) < 2 then error "no status bar"
  click menu bar item 1 of menu bar 2
end tell
AS
}

write_crop() {
  local geo="$1"
  python3 - "$geo" "$OUT/crop.txt" "$OUT/geometry.txt" <<'PY'
import sys
geo, crop_path, geom_path = sys.argv[1:]
parts = geo.split()
open(geom_path, "w").write(geo + "\n")
if not parts or parts[0] != "GEOMETRY" or len(parts) < 7:
    print("geometry unavailable:", geo)
    sys.exit(0)
x, y, w, h, sw, sh = (int(float(v)) for v in parts[1:7])
scale = max(1, round(w / 388)) if w else 1
pad = 160 * scale
crop_x = max(0, x - pad)
crop_w = max(2, sw - crop_x)
crop_h = max(2, min(sh, y + h + 20 * scale))
# Even dimensions for libx264.
crop_w -= crop_w % 2
crop_h -= crop_h % 2
open(crop_path, "w").write(f"{crop_w}:{crop_h}:{crop_x}:0\n")
print(f"crop {crop_w}:{crop_h}:{crop_x}:0 scale={scale} window={w}x{h} at {x},{y} screen={sw}x{sh}")
PY
}

if [[ ! -d "$APP_SRC" ]]; then
  log "missing $APP_SRC — build with: npm run tauri build -- --bundles app"
  exit 1
fi

log "preparing desktop"
wake_display
prepare_desktop
grant_tcc

rm -f /tmp/usagebar-demo.log /tmp/usagebar-demo.port
osascript -e 'quit app "UsageBar"' >/dev/null 2>&1 || true
killall UsageBar >/dev/null 2>&1 || true
sleep 0.4
rm -rf "$APP"
ditto "$APP_SRC" "$APP"
xattr -cr "$APP" >/dev/null 2>&1 || true
codesign --force --deep --sign - "$APP" >/dev/null 2>&1 || log "ad-hoc codesign skipped"

log "launching UsageBar --demo"
USAGEBAR_DEMO=1 "$APP/Contents/MacOS/UsageBar" --demo >"$OUT/app.log" 2>&1 &
APP_PID=$!

ready=0
for _ in $(seq 1 70); do
  if [[ -f /tmp/usagebar-demo.port ]] && grep -q "listening" /tmp/usagebar-demo.log 2>/dev/null; then
    ready=1
    break
  fi
  if ! kill -0 "$APP_PID" 2>/dev/null; then
    log "UsageBar exited early"
    cat "$OUT/app.log" || true
    exit 1
  fi
  sleep 0.5
done
if [[ "$ready" != 1 ]]; then
  log "demo control did not start"
  cat "$OUT/app.log" || true
  exit 1
fi
pong="$(demo PING)"
log "ping: $pong"
[[ "$pong" == "PONG demo" ]] || { log "demo mode did not answer"; exit 1; }
sleep 1

# Permission probe. GitHub's macOS image grants this to bash; if the frame is
# missing or tiny, try the user TCC database once and probe again.
if ! screencapture -x "$OUT/perm-test.png" || [[ ! -s "$OUT/perm-test.png" ]]; then
  log "screencapture probe failed; trying a user TCC grant"
  grant_tcc
  sleep 1
  screencapture -x "$OUT/perm-test.png" || true
fi
if [[ ! -s "$OUT/perm-test.png" ]]; then
  log "screen recording is blocked; perm-test.png was not written"
  exit 1
fi
log "probe still: $(sips -g pixelWidth -g pixelHeight "$OUT/perm-test.png" 2>/dev/null | tr '\n' ' ' || true)"

log "recording ${SECS}s"
screencapture -v -C -V "$SECS" "$MOV" &
REC_PID=$!
sleep 1.5

# Menu bar only, then open the popover. A real status-item click when
# Accessibility allows it; the demo hook shows the same window if it misses.
sleep 1.5
click_tray && log "clicked status item" || log "status-item click failed; opening via demo hook"
sleep 0.7
visible="$(demo VISIBLE || true)"
log "visible after click: ${visible:-?}"
if [[ "$visible" != "VISIBLE 1" ]]; then
  log "show hook: $(demo SHOW)"
  sleep 0.8
fi
geo="$(demo "GEOMETRY main" || true)"
log "geometry: $geo"
write_crop "$geo" || true

sleep 0.8
for _ in 1 2 3 4 5 6 7 8; do
  demo "SCROLL 175" >/dev/null || true
  sleep 1.05
done
sleep 0.4
log "settings: $(demo SETTINGS)"
sleep 3.2
log "close settings: $(demo CLOSE_SETTINGS)"
sleep 0.8
demo SCROLL_TOP >/dev/null || true
sleep 1.4
log "hide: $(demo HIDE)"
sleep 1.5

wait "$REC_PID" || { log "screencapture exited $?"; exit 1; }
screencapture -x "$OUT/after.png" || true
demo QUIT >/dev/null 2>&1 || true
wait "$APP_PID" 2>/dev/null || true

if [[ ! -s "$MOV" ]]; then
  log "recording is empty"
  exit 1
fi
log "wrote $MOV ($(du -h "$MOV" | awk '{print $1}'))"
