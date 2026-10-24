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
# tools/tauri.mjs moves the bundle into macos.noindex so Spotlight does not
# list the build-tree copy. Cargo may also redirect into CARGO_TARGET_DIR.
TARGET="${CARGO_TARGET_DIR:-$ROOT/src-tauri/target}"
APP_SRC=""
for candidate in \
  "$TARGET/release/bundle/macos.noindex/UsageBar.app" \
  "$TARGET/release/bundle/macos/UsageBar.app" \
  "$ROOT/src-tauri/target/release/bundle/macos.noindex/UsageBar.app" \
  "$ROOT/src-tauri/target/release/bundle/macos/UsageBar.app"
do
  if [[ -d "$candidate" ]]; then
    APP_SRC="$candidate"
    break
  fi
done
APP="/Applications/UsageBar.app"
MOV="$OUT/usagebar-macos-raw.mov"
mkdir -p "$OUT/frames"

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

set_display() {
  # A larger framebuffer makes the same 388pt popover more pixels, so the
  # 1080p composite stays sharp. The helper is best-effort: a VM with one
  # mode keeps it.
  cat > "$OUT/set-display.swift" <<'SWIFT'
import CoreGraphics
let display = CGMainDisplayID()
func describe(_ mode: CGDisplayMode) -> String {
  "\(mode.width)x\(mode.height) pixels \(mode.pixelWidth)x\(mode.pixelHeight)"
}
if let current = CGDisplayCopyDisplayMode(display) {
  print("current \(describe(current))")
}
guard let modes = CGDisplayCopyAllDisplayModes(display, nil) as? [CGDisplayMode], !modes.isEmpty else {
  print("no display modes")
  exit(0)
}
var best: CGDisplayMode?
var bestPixels = 0
for mode in modes {
  let pixels = mode.pixelWidth * mode.pixelHeight
  print("mode \(describe(mode))")
  // Prefer a retina backing store. A 1x 1920 mode does not add pixels to a
  // window that is already 388 points wide.
  let score = pixels * (mode.pixelWidth > mode.width ? 4 : 1)
  if mode.pixelWidth >= 1440 && mode.pixelHeight >= 900 && score > bestPixels {
    best = mode
    bestPixels = score
  }
}
guard let chosen = best else {
  print("keeping the current mode")
  exit(0)
}
var config: CGDisplayConfigRef?
if CGBeginDisplayConfiguration(&config) == .success, let config {
  let configured = CGConfigureDisplayWithDisplayMode(config, display, chosen, nil)
  let completed = CGCompleteDisplayConfiguration(config, .permanently)
  print("configure \(configured.rawValue) complete \(completed.rawValue) \(describe(chosen))")
} else {
  let result = CGDisplaySetDisplayMode(display, chosen, nil)
  print("setDisplayMode \(result.rawValue) \(describe(chosen))")
}
if let current = CGDisplayCopyDisplayMode(display) {
  print("now \(describe(current))")
}
SWIFT
  if swiftc -O -o "$OUT/set-display" "$OUT/set-display.swift" 2>"$OUT/set-display-build.log"; then
    "$OUT/set-display" | tee "$OUT/display-modes.txt" || log "display mode change failed"
    sleep 0.6
    screencapture -x "$OUT/display-check.png" >/dev/null 2>&1 || true
    if [[ -s "$OUT/display-check.png" ]]; then
      log "display check: $(sips -g pixelWidth -g pixelHeight "$OUT/display-check.png" 2>/dev/null | tr '\n' ' ')"
    fi
  else
    log "could not compile the display helper"
    tail -n 20 "$OUT/set-display-build.log" || true
  fi
}

write_crop() {
  local geo="$1"
  python3 - "$geo" "$OUT/shot.txt" "$OUT/geometry.txt" <<'PY'
import sys
geo, shot_path, geom_path = sys.argv[1:]
parts = geo.split()
open(geom_path, "w").write(geo + "\n")
if not parts or parts[0] != "GEOMETRY" or len(parts) < 7:
    print("geometry unavailable:", geo)
    sys.exit(0)
x, y, w, h, sw, sh = (int(float(v)) for v in parts[1:7])
scale = max(1, round(w / 388)) if w else 1
# The status item sits just left of the popover. Keep that slice of the
# menu bar and the window, and nothing else on the desktop.
pad_x = 180 * scale
pad_y = 8 * scale
crop_x = max(0, x - pad_x)
crop_r = min(sw, x + w + 12 * scale)
crop_b = min(sh, y + h + pad_y)
crop_w = int(crop_r - crop_x)
crop_h = int(crop_b)  # y starts at 0 so the menu bar is in frame
crop_w -= crop_w % 2
crop_h -= crop_h % 2
open(shot_path, "w").write(
    "kind=card\n"
    f"crop={crop_w}:{crop_h}:{crop_x}:0\n"
    "trim=0\n"
    "gif_ss=5\n"
    "gif_t=6\n"
)
print(f"crop {crop_w}:{crop_h}:{crop_x}:0 scale={scale} window={w}x{h} at {x},{y} screen={sw}x{sh}")
PY
}

media_duration() {
  local file="$1"
  ffprobe -v error -show_entries format=duration -of csv=p=0 "$file" 2>/dev/null || echo 0
}

last_frame_pts() {
  ffprobe -v error -select_streams v:0 -show_entries frame=pts_time -of csv=p=0 "$1" 2>/dev/null | tail -1
}

if [[ -z "$APP_SRC" ]]; then
  log "UsageBar.app was not produced. Looked under $TARGET/release/bundle"
  find "$ROOT/src-tauri/target" -name 'UsageBar.app' -maxdepth 6 2>/dev/null || true
  exit 1
fi
log "using bundle $APP_SRC"

log "preparing desktop"
wake_display
prepare_desktop
set_display
grant_tcc

rm -f /tmp/usagebar-demo.log /tmp/usagebar-demo.port
osascript -e 'quit app "UsageBar"' >/dev/null 2>&1 || true
killall UsageBar >/dev/null 2>&1 || true
sleep 0.4
rm -rf "$APP"
ditto "$APP_SRC" "$APP"
xattr -cr "$APP" >/dev/null 2>&1 || true
codesign --force --deep --sign - "$APP" >/dev/null 2>&1 || log "ad-hoc codesign skipped"

BIN="$APP/Contents/MacOS/usagebar"
if [[ ! -x "$BIN" && -x "$APP/Contents/MacOS/UsageBar" ]]; then
  BIN="$APP/Contents/MacOS/UsageBar"
fi
if [[ ! -x "$BIN" ]]; then
  log "no executable in $APP/Contents/MacOS"
  ls -la "$APP/Contents/MacOS" || true
  exit 1
fi
log "launching $BIN --demo"
USAGEBAR_DEMO=1 "$BIN" --demo >"$OUT/app.log" 2>&1 &
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

# A second screencapture while -V is running stops the movie around the
# moment of that still and leaves the container duration padded. Stills are
# extracted from the finished movie instead.
play_story() {
  sleep 1.8
  click_tray && log "clicked status item" || log "status-item click failed; opening via demo hook"
  sleep 0.6
  visible="$(demo VISIBLE || true)"
  log "visible after click: ${visible:-?}"
  if [[ "$visible" != "VISIBLE 1" ]]; then
    log "show hook: $(demo SHOW)"
    sleep 0.7
  fi
  geo="$(demo "GEOMETRY main" || true)"
  log "geometry: $geo"
  write_crop "$geo" || true
  sleep 1.2
  local step
  for step in 1 2 3 4 5 6 7 8 9 10 11 12; do
    demo "SCROLL 140" >/dev/null || true
    sleep 0.95
  done
  sleep 0.5
  log "settings: $(demo SETTINGS)"
  sleep 5.4
  log "close settings: $(demo CLOSE_SETTINGS)"
  sleep 0.6
  demo SCROLL_TOP >/dev/null || true
  sleep 2.4
}

start_ffmpeg_record() {
  local ffmpeg dev
  ffmpeg="$(command -v ffmpeg || true)"
  [[ -n "$ffmpeg" ]] || return 1
  grant_tcc
  # Screen Recording is granted per binary. Point TCC at the real ffmpeg.
  local real
  real="$(python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$ffmpeg")"
  local db="$HOME/Library/Application Support/com.apple.TCC/TCC.db"
  if [[ -f "$db" ]]; then
    sqlite3 "$db" "INSERT OR REPLACE INTO access (service, client, client_type, auth_value, auth_reason, auth_version, csreq, policy_id, indirect_object_identifier_type, indirect_object_identifier, indirect_object_code_identity, flags, last_modified) VALUES ('kTCCServiceScreenCapture','$real',1,2,4,1,NULL,NULL,0,'UNUSED',NULL,0,CAST(strftime('%s','now') AS INTEGER));" \
      >/dev/null 2>&1 || log "TCC grant for ffmpeg was rejected"
  fi
  "$ffmpeg" -hide_banner -f avfoundation -list_devices true -i "" >"$OUT/avdevices.txt" 2>&1 || true
  dev="$(python3 - "$OUT/avdevices.txt" <<'PY'
import re, sys
text = open(sys.argv[1], errors="replace").read()
match = re.search(r"\[(\d+)\] Capture screen", text)
print(match.group(1) if match else "")
PY
)"
  [[ -n "$dev" ]] || { log "no avfoundation screen device"; return 1; }
  log "recording ${SECS}s with ffmpeg avfoundation device $dev"
  MOV="$OUT/usagebar-macos-raw.mp4"
  "$ffmpeg" -y -f avfoundation -framerate 30 -capture_cursor 1 -i "${dev}:none" \
    -t "$SECS" -c:v libx264 -preset veryfast -crf 16 -pix_fmt yuv420p \
    "$MOV" >"$OUT/ffmpeg-record.log" 2>&1 &
  REC_PID=$!
}

start_screencapture_record() {
  log "recording ${SECS}s with screencapture"
  MOV="$OUT/usagebar-macos-raw.mov"
  screencapture -v -C -V "$SECS" "$MOV" &
  REC_PID=$!
}

finish_recording() {
  local label="$1"
  wait "$REC_PID" || log "$label recorder exited $?"
  if [[ ! -s "$MOV" ]]; then
    log "$label recording is empty"
    return 1
  fi
  local duration last luma
  duration="$(media_duration "$MOV")"
  last="$(last_frame_pts "$MOV")"
  log "$label duration=${duration:-?} last_frame=${last:-?} size=$(du -h "$MOV" | awk '{print $1}')"
  python3 - "$duration" "$last" "$SECS" <<'PY'
import sys
duration, last, want = (float(v or 0) for v in sys.argv[1:4])
# Frames have to keep coming until the storyboard is over. A container that
# claims the full length with its last frame several seconds earlier is the
# frozen-ending failure.
if last < want - 5:
    raise SystemExit(2)
PY
  if command -v ffmpeg >/dev/null 2>&1; then
    luma="$(ffmpeg -v error -ss 2 -i "$MOV" -frames:v 1 -f rawvideo -pix_fmt gray - | python3 -c 'import sys; d=sys.stdin.buffer.read(); print(0 if not d else sum(d)/len(d))')"
    log "frame luma ${luma:-0}"
    python3 -c 'import sys; raise SystemExit(0 if float(sys.argv[1]) > 12 else 3)' "${luma:-0}"
  fi
}

# The storyboard is about 26s. The cap is a little longer so a slow click
# still finishes inside the file. postprocess cuts at the story end written
# below, which drops the idle tail instead of publishing a frozen frame.
if (( SECS > 36 )); then
  log "capping recording at 36s"
  SECS=36
elif (( SECS < 32 )); then
  log "recording 32s so the storyboard can finish"
  SECS=32
fi

note_story_end() {
  [[ -f "$OUT/shot.txt" ]] || return 0
  local ended
  ended="$(python3 -c 'import time,sys; print(round(time.time()-float(sys.argv[1]), 2))' "$RECORD_STARTED")"
  grep -v '^end=' "$OUT/shot.txt" >"$OUT/shot.txt.tmp" || true
  mv "$OUT/shot.txt.tmp" "$OUT/shot.txt"
  printf 'end=%s\n' "$ended" >>"$OUT/shot.txt"
  log "story end ${ended}s"
}

RECORD_STARTED="$(python3 -c 'import time; print(time.time())')"
REC_LABEL="ffmpeg"
if ! start_ffmpeg_record; then
  REC_LABEL="screencapture"
  start_screencapture_record
fi
sleep 0.4
play_story
note_story_end
if ! finish_recording "$REC_LABEL"; then
  log "capture looks short; recording once more with screencapture"
  rm -f "$MOV"
  if [[ -n "${REC_PID:-}" ]] && kill -0 "$REC_PID" 2>/dev/null; then
    kill "$REC_PID" 2>/dev/null || true
    wait "$REC_PID" 2>/dev/null || true
  fi
  demo HIDE >/dev/null 2>&1 || true
  sleep 0.4
  REC_LABEL="screencapture"
  RECORD_STARTED="$(python3 -c 'import time; print(time.time())')"
  start_screencapture_record
  sleep 0.4
  play_story
  note_story_end
  finish_recording "$REC_LABEL" || log "second capture is still short; keeping it"
fi
if [[ ! -f "$OUT/shot.txt" ]]; then
  log "shot.txt was not written; the popover geometry never arrived"
  exit 1
fi

demo QUIT >/dev/null 2>&1 || true
wait "$APP_PID" 2>/dev/null || true

if [[ ! -s "$MOV" ]]; then
  log "recording is empty"
  exit 1
fi
log "wrote $MOV"
