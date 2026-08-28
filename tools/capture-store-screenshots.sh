#!/usr/bin/env bash
# Captures the real UsageBar popover at App Store size (1280×800, 16:10).
# Run in Terminal.app (needs Screen Recording). Cursor's agent cannot.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$ROOT/store/screenshots"
mkdir -p "$OUT"

open -a UsageBar
sleep 1

osascript <<'EOF'
tell application "System Events" to activate
display dialog "App Store screenshots from the real app.

1. Click the UsageBar meter in the menu bar so the popover is open.
2. Click OK here.

Do not cover the popover with this dialog — drag this window down if needed." buttons {"Cancel", "Capture popover"} default button 2 with title "UsageBar screenshots"
EOF

screencapture -x -t png "$OUT/_raw-popover.png"

osascript <<'EOF'
tell application "System Events" to activate
display dialog "Now click Settings in the popover so the sheet is open.

Then click OK. Keep the popover on screen." buttons {"Cancel", "Capture settings"} default button 2 with title "UsageBar screenshots"
EOF

screencapture -x -t png "$OUT/_raw-settings.png"

python3 - "$OUT" <<'PY'
import sys
from pathlib import Path

try:
    from PIL import Image
except ImportError:
    sys.exit("Need Pillow: python3 -m pip install Pillow")

out = Path(sys.argv[1])
w, h = 1280, 800

def fit(src: Path, dest: Path) -> None:
    im = Image.open(src).convert("RGB")
    # Keep the menu bar: crop 16:10 from the top, then scale.
    target_ratio = w / h
    cw, ch = im.size
    if cw / ch > target_ratio:
        nw = int(ch * target_ratio)
        left = cw - nw  # popover sits on the right
        im = im.crop((left, 0, cw, ch))
    else:
        nh = int(cw / target_ratio)
        im = im.crop((0, 0, cw, nh))
    im = im.resize((w, h), Image.Resampling.LANCZOS)
    im.save(dest, "PNG")
    print(f"Wrote {dest} ({w}×{h})")

fit(out / "_raw-popover.png", out / "01-menubar.png")
fit(out / "_raw-settings.png", out / "02-settings.png")
PY

echo
echo "Upload these in App Store Connect (Mac 1280×800):"
echo "  $OUT/01-menubar.png"
echo "  $OUT/02-settings.png"
open "$OUT"
