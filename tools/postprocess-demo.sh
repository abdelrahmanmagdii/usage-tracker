#!/usr/bin/env bash
# Turn UsageBar demo recordings into a README GIF plus LinkedIn and Reddit MP4s.
#
# Each raw directory needs shot.txt (written by the record scripts) describing
# the window rectangles. The social MP4s are posters: the windows sit on a
# brand gradient with rounded corners and a shadow. The GIF is the window
# itself, about 800px wide and under 2 MB.
#
# Usage: tools/postprocess-demo.sh <macos-dir> <windows-dir>
set -euo pipefail

if ! command -v ffmpeg >/dev/null 2>&1; then
  sudo apt-get update -qq
  sudo apt-get install -y -qq ffmpeg
fi
command -v gifsicle >/dev/null 2>&1 || sudo apt-get install -y -qq gifsicle >/dev/null 2>&1 || true
python3 -c 'import PIL' >/dev/null 2>&1 || sudo apt-get install -y -qq python3-pil >/dev/null 2>&1 || python3 -m pip install --user pillow >/dev/null 2>&1

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MAC_DIR="${1:-}"
WIN_DIR="${2:-}"
OUT="${OUT:-processed}"
mkdir -p "$OUT"

shot_value() {
  local file="$1" key="$2" fallback="$3"
  if [[ -f "$file" ]]; then
    local value
    value="$(awk -F= -v k="$key" '$1==k {print $2; exit}' "$file" | tr -d '\r')"
    if [[ -n "$value" ]]; then
      printf '%s' "$value"
      return
    fi
  fi
  printf '%s' "$fallback"
}

find_raw() {
  find "$1" -type f \( -name '*.mov' -o -name '*.mp4' \) ! -name '*linkedin*' ! -name '*reddit*' -print -quit
}

media_duration() {
  ffprobe -v error -show_entries format=duration -of csv=p=0 "$1"
}

shrink_gif() {
  local gif="$1"
  command -v gifsicle >/dev/null 2>&1 || return 0
  local size lossy
  size="$(stat -c%s "$gif" 2>/dev/null || stat -f%z "$gif")"
  if (( size < 2000000 )); then
    gifsicle -O3 -o "$gif.opt" "$gif" && mv "$gif.opt" "$gif"
    return 0
  fi
  for lossy in 60 100 160; do
    gifsicle -O3 --lossy="$lossy" --colors 128 -o "$gif.opt" "$gif" || return 0
    mv "$gif.opt" "$gif"
    size="$(stat -c%s "$gif" 2>/dev/null || stat -f%z "$gif")"
    (( size < 2000000 )) && return 0
  done
}

encode_poster() {
  local raw="$1" shot="$2" dest="$3" w="$4" h="$5" ss="$6" t="$7"
  python3 "$ROOT/tools/compose_social.py" \
    --raw "$raw" --shot "$shot" --dest "$dest" --mode mp4 \
    --width "$w" --height "$h" --ss "$ss" --t "$t"
}

process_one() {
  local dir="$1" name="$2" square="$3"
  [[ -d "$dir" ]] || { echo "skip $name (no dir)"; return 0; }
  local raw shot
  raw="$(find_raw "$dir" || true)"
  [[ -n "$raw" ]] || { echo "skip $name (no video)"; return 0; }
  shot="$dir/shot.txt"
  [[ -f "$shot" ]] || { echo "skip $name (no shot.txt)"; return 0; }

  local trim gif_ss gif_t duration story_end available li_t rd_t
  trim="$(shot_value "$shot" trim 0)"
  gif_ss="$(shot_value "$shot" gif_ss 4)"
  gif_t="$(shot_value "$shot" gif_t 6)"
  story_end="$(shot_value "$shot" end 0)"
  duration="$(media_duration "$raw")"
  # `end` is when the interactions finished. Anything after that is the
  # recorder's idle tail, and it should not become a frozen ending.
  available="$(python3 -c 'import sys
d,t,e=map(float,sys.argv[1:])
span=(e-t) if e>t+0.5 else (d-t)
if d>1 and e>t+0.5:
    span=min(span, d-t)
print(max(0.5, span))' "$duration" "$trim" "$story_end")"
  li_t="$(python3 -c 'import sys; print(min(28.0, float(sys.argv[1])))' "$available")"
  rd_t="$(python3 -c 'import sys; print(min(58.0, float(sys.argv[1])))' "$available")"
  echo "processing $name from $raw duration=$duration trim=$trim end=$story_end li=$li_t rd=$rd_t"

  local gif="$OUT/${name}-readme.gif"
  python3 "$ROOT/tools/compose_social.py" \
    --raw "$raw" --shot "$shot" --dest "$gif" --mode gif \
    --width 800 --ss "$gif_ss" --t "$gif_t"
  shrink_gif "$gif"

  local wide="$OUT/${name}-linkedin-1920x1080.mp4"
  encode_poster "$raw" "$shot" "$wide" 1920 1080 "$trim" "$li_t"
  if python3 -c 'import sys; raise SystemExit(0 if abs(float(sys.argv[1])-float(sys.argv[2]))<0.4 else 1)' "$li_t" "$rd_t"; then
    cp -f "$wide" "$OUT/${name}-reddit-1920x1080.mp4"
  else
    encode_poster "$raw" "$shot" "$OUT/${name}-reddit-1920x1080.mp4" 1920 1080 "$trim" "$rd_t"
  fi
  if [[ "$square" == 1 ]]; then
    encode_poster "$raw" "$shot" "$OUT/${name}-linkedin-1080x1080.mp4" 1080 1080 "$trim" "$li_t"
  fi
}

process_one "$MAC_DIR" "usagebar-macos" 1
process_one "$WIN_DIR" "usagebar-windows" 0

mac_wide="$OUT/usagebar-macos-linkedin-1920x1080.mp4"
win_wide="$OUT/usagebar-windows-linkedin-1920x1080.mp4"
if [[ -f "$mac_wide" && -f "$win_wide" ]]; then
  # 14s of each platform stays inside LinkedIn's 30s limit. The mac half
  # starts mid-scroll and includes Settings; the Windows half includes the
  # dashboard scroll and the Settings page.
  ffmpeg -v error -y -i "$mac_wide" -i "$win_wide" \
    -f lavfi -t 28 -i anullsrc=channel_layout=stereo:sample_rate=48000 \
    -filter_complex "[0:v]trim=8:22,setpts=PTS-STARTPTS[a];[1:v]trim=4:18,setpts=PTS-STARTPTS[b];[a][b]concat=n=2:v=1:a=0[v]" \
    -map "[v]" -map 2:a -c:v libx264 -profile:v high -preset veryfast -crf 18 -pix_fmt yuv420p -r 30 \
    -movflags +faststart -c:a aac -b:a 128k -shortest -t 28 \
    "$OUT/usagebar-combined-linkedin-1920x1080.mp4"
fi

mkdir -p "$OUT/frames"
grab() {
  local src="$1" ss="$2" dest="$3"
  [[ -f "$src" ]] || return 0
  ffmpeg -v error -y -ss "$ss" -i "$src" -frames:v 1 -update 1 "$dest" || true
}
grab "$mac_wide" 8 "$OUT/frames/macos-popover.png"
grab "$mac_wide" 21 "$OUT/frames/macos-settings.png"
grab "$win_wide" 0.7 "$OUT/frames/windows-dashboard.png"
grab "$win_wide" 16 "$OUT/frames/windows-settings.png"

shopt -s nullglob
for f in "$OUT"/*.gif "$OUT"/*.mp4; do
  printf '%-72s %8s  %s\n' "$f" "$(du -h "$f" | cut -f1)" \
    "$(ffprobe -v error -show_entries stream=width,height:format=duration -of csv=p=0 "$f" | tr '\n' ' ')"
done
