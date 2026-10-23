#!/usr/bin/env bash
# Turn UsageBar demo recordings into a README GIF plus LinkedIn and Reddit MP4s.
#
# Usage:
#   tools/postprocess-demo.sh <macos-dir> <windows-dir>
# Missing directories are skipped. A crop.txt of "w:h:x:y" next to the video
# (or $CROP) crops before scaling. Output defaults to ./processed.
#
#   GIF_W=800            README GIF width (5–8s, under 2 MB)
#   macOS GIF window:    MAC_GIF_SS MAC_GIF_T   (default 6 / 7)
#   Windows GIF window:  WIN_GIF_SS WIN_GIF_T   (default 2 / 7)
set -euo pipefail

if ! command -v ffmpeg >/dev/null 2>&1; then
  sudo apt-get update -qq
  sudo apt-get install -y -qq ffmpeg
fi
command -v gifsicle >/dev/null 2>&1 || sudo apt-get install -y -qq gifsicle >/dev/null 2>&1 || true

MAC_DIR="${1:-}"
WIN_DIR="${2:-}"
OUT="${OUT:-processed}"
mkdir -p "$OUT/frames"

GIF_W="${GIF_W:-800}"
GIF_H="${GIF_H:-1400}"
GIF_FPS="${GIF_FPS:-10}"
MAC_GIF_SS="${MAC_GIF_SS:-6}"
MAC_GIF_T="${MAC_GIF_T:-6}"
WIN_GIF_SS="${WIN_GIF_SS:-2}"
WIN_GIF_T="${WIN_GIF_T:-6}"
LI_T="${LI_T:-26}"
RD_T="${RD_T:-60}"

X264=(-c:v libx264 -profile:v high -preset veryfast -crf 20 -pix_fmt yuv420p -r 30 -movflags +faststart)
SILENT=(-f lavfi -i anullsrc=channel_layout=stereo:sample_rate=48000)
AAC=(-c:a aac -b:a 128k -shortest)

fit() {
  echo "scale=$1:$2:force_original_aspect_ratio=decrease:flags=lanczos,pad=$1:$2:(ow-iw)/2:(oh-ih)/2:color=0x17121f,setsar=1"
}

find_raw() {
  find "$1" -type f \( -name '*.mov' -o -name '*.mp4' \) ! -name '*linkedin*' ! -name '*reddit*' -print -quit
}

crop_filter() {
  local dir="$1"
  local crop="${CROP:-}"
  if [[ -z "$crop" && -f "$dir/crop.txt" ]]; then
    crop="$(tr -d '[:space:]' < "$dir/crop.txt")"
  fi
  if [[ -n "$crop" && "$crop" != "full" ]]; then
    printf 'crop=%s,' "$crop"
  fi
}

try_gif() {
  local raw="$1" gif="$2" ss="$3" t="$4" colors="$5" cropf="$6"
  # Fit inside GIF_W x GIF_H so a tall Windows column does not become a
  # 1600px-tall README image, while a menu-bar popover stays near 800px wide.
  ffmpeg -v error -y -ss "$ss" -t "$t" -i "$raw" -filter_complex \
    "[0:v]${cropf}fps=${GIF_FPS},scale=${GIF_W}:${GIF_H}:force_original_aspect_ratio=decrease:flags=lanczos,split[a][b];[a]palettegen=max_colors=${colors}:stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=3:diff_mode=rectangle" \
    -loop 0 "$gif"
}

shrink_gif() {
  local gif="$1"
  command -v gifsicle >/dev/null 2>&1 || return 0
  local size lossy
  size="$(stat -c%s "$gif")"
  if (( size < 2000000 )); then
    gifsicle -O3 -o "$gif.opt" "$gif" && mv "$gif.opt" "$gif"
    return 0
  fi
  for lossy in 60 100 160; do
    gifsicle -O3 --lossy="$lossy" --colors 128 -o "$gif.opt" "$gif" || return 0
    mv "$gif.opt" "$gif"
    size="$(stat -c%s "$gif")"
    (( size < 2000000 )) && return 0
  done
}

make_gif() {
  local raw="$1" gif="$2" ss="$3" t="$4" cropf="$5"
  if ! try_gif "$raw" "$gif" "$ss" "$t" 128 "$cropf"; then
    try_gif "$raw" "$gif" 0 "$t" 96 "$cropf"
  fi
  shrink_gif "$gif"
}

encode_mp4() {
  local raw="$1" dest="$2" ss="$3" t="$4" w="$5" h="$6" cropf="$7"
  ffmpeg -v error -y -ss "$ss" -t "$t" -i "$raw" "${SILENT[@]}" \
    -vf "${cropf}$(fit "$w" "$h")" -map 0:v -map 1:a "${X264[@]}" \
    -maxrate 8M -bufsize 16M "${AAC[@]}" "$dest"
}

extract_frames() {
  local raw="$1" name="$2" cropf="$3"
  mkdir -p "$OUT/frames"
  ffmpeg -v error -y -i "$raw" -vf "${cropf}fps=1/2" "$OUT/frames/${name}-%02d.png" || \
    ffmpeg -v error -y -i "$raw" -vf "fps=1/2" "$OUT/frames/${name}-%02d.png"
}

process_one() {
  local dir="$1" name="$2" gif_ss="$3" gif_t="$4" square="$5"
  [[ -d "$dir" ]] || { echo "skip $name (no dir)"; return 0; }
  local raw
  raw="$(find_raw "$dir" || true)"
  [[ -n "$raw" ]] || { echo "skip $name (no video)"; return 0; }
  local cropf
  cropf="$(crop_filter "$dir")"
  echo "processing $name from $raw crop=${cropf:-none}"
  extract_frames "$raw" "$name" "$cropf"
  make_gif "$raw" "$OUT/${name}-readme.gif" "$gif_ss" "$gif_t" "$cropf"
  if [[ "$square" == 1 ]]; then
    encode_mp4 "$raw" "$OUT/${name}-linkedin-1080x1080.mp4" 0 "$LI_T" 1080 1080 "$cropf"
  fi
  encode_mp4 "$raw" "$OUT/${name}-linkedin-1920x1080.mp4" 0 "$LI_T" 1920 1080 "$cropf"
  encode_mp4 "$raw" "$OUT/${name}-reddit-1920x1080.mp4" 0 "$RD_T" 1920 1080 "$cropf"
}

process_one "$MAC_DIR" "usagebar-macos" "$MAC_GIF_SS" "$MAC_GIF_T" 1
process_one "$WIN_DIR" "usagebar-windows" "$WIN_GIF_SS" "$WIN_GIF_T" 0

mac_rd="$OUT/usagebar-macos-reddit-1920x1080.mp4"
win_rd="$OUT/usagebar-windows-reddit-1920x1080.mp4"
if [[ -f "$mac_rd" && -f "$win_rd" ]]; then
  # Storyboard is the first ~22s. Two of those stay under Reddit's 60s cap.
  ffmpeg -v error -y -i "$mac_rd" -i "$win_rd" "${SILENT[@]}" -filter_complex \
    "[0:v]trim=0:22,setpts=PTS-STARTPTS[a];[1:v]trim=0:22,setpts=PTS-STARTPTS[b];[a][b]concat=n=2:v=1:a=0[v]" \
    -map "[v]" -map 2:a "${X264[@]}" -t 44 "${AAC[@]}" \
    "$OUT/usagebar-combined-reddit-1920x1080.mp4"
  ffmpeg -v error -y -i "$mac_rd" -i "$win_rd" "${SILENT[@]}" -filter_complex \
    "[0:v]trim=0:14,setpts=PTS-STARTPTS[a];[1:v]trim=0:14,setpts=PTS-STARTPTS[b];[a][b]concat=n=2:v=1:a=0[v]" \
    -map "[v]" -map 2:a "${X264[@]}" -t 28 "${AAC[@]}" \
    "$OUT/usagebar-combined-linkedin-1920x1080.mp4"
fi

shopt -s nullglob
for f in "$OUT"/*.gif "$OUT"/*.mp4; do
  printf '%-72s %8s  %s\n' "$f" "$(du -h "$f" | cut -f1)" \
    "$(ffprobe -v error -show_entries stream=width,height:format=duration -of csv=p=0 "$f" | tr '\n' ' ')"
done
