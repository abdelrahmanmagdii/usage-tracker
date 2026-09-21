#!/usr/bin/env python3
"""Turn docs/_capture PNGs into the public stills and the README demo GIF.

Capture first:

    npm run dev
    node tools/capture-public-media.mjs
    python3 tools/compose-demo.py
"""

from __future__ import annotations

import shutil
import subprocess
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont

ROOT = Path(__file__).resolve().parents[1]
CAPTURE = ROOT / "docs" / "_capture"
DOCS = ROOT / "docs"
SCREENS = ROOT / "website" / "screens"
FRAMES = CAPTURE / "frames"

# Captures are 3x. Public stills are 2x, which stays sharp at the size the
# site and README actually display.
STILL_SCALE = 2 / 3
WINDOW_RADIUS_CSS = 22

# GIF is composed at 2x and scaled down so the menu-bar type stays crisp.
GIF_W, GIF_H = 800, 960
RENDER = 2

FONT_REGULAR = "/usr/share/fonts/truetype/macos/Inter-Regular.ttf"
FONT_MEDIUM = "/usr/share/fonts/truetype/macos/Inter-Medium.ttf"
FONT_SEMIBOLD = "/usr/share/fonts/truetype/macos/Inter-SemiBold.ttf"


def font(path: str, size: int) -> ImageFont.FreeTypeFont | ImageFont.ImageFont:
    try:
        return ImageFont.truetype(path, size)
    except OSError:
        return ImageFont.load_default()


def rounded_mask(size: tuple[int, int], radius: int) -> Image.Image:
    mask = Image.new("L", size, 0)
    draw = ImageDraw.Draw(mask)
    draw.rounded_rectangle((0, 0, size[0] - 1, size[1] - 1), radius=radius, fill=255)
    return mask


def export_still(src: Path, dest: Path) -> None:
    image = Image.open(src).convert("RGBA")
    sized = image.resize(
        (round(image.width * STILL_SCALE), round(image.height * STILL_SCALE)),
        Image.Resampling.LANCZOS,
    )
    radius = round(WINDOW_RADIUS_CSS * 2)
    sized.putalpha(rounded_mask(sized.size, radius))
    dest.parent.mkdir(parents=True, exist_ok=True)
    sized.save(dest, "PNG")
    print(f"Wrote {dest.relative_to(ROOT)} {sized.size[0]}x{sized.size[1]}")


def gradient(size: tuple[int, int], top: tuple[int, int, int], bottom: tuple[int, int, int]) -> Image.Image:
    width, height = size
    column = Image.new("RGB", (1, height))
    for y in range(height):
        t = y / max(height - 1, 1)
        column.putpixel((0, y), tuple(int(top[i] + (bottom[i] - top[i]) * t) for i in range(3)))
    return column.resize((width, height), Image.Resampling.BILINEAR)


def draw_bars(draw: ImageDraw.ImageDraw, x: int, y: int, scale: int) -> None:
    dark = (28, 28, 30)
    draw.rounded_rectangle((x, y, x + 3 * scale, y + 11 * scale), 1 * scale, fill=dark)
    draw.rounded_rectangle((x + 5 * scale, y + 4 * scale, x + 8 * scale, y + 11 * scale), 1 * scale, fill=dark)


def draw_bolt(draw: ImageDraw.ImageDraw, x: int, y: int, scale: int) -> None:
    s = scale
    draw.polygon(
        [
            (x + 6 * s, y),
            (x + 2 * s, y + 7 * s),
            (x + 5 * s, y + 7 * s),
            (x + 1 * s, y + 13 * s),
            (x + 8 * s, y + 5 * s),
            (x + 5 * s, y + 5 * s),
        ],
        fill=(184, 112, 28),
    )


def wallpaper(size: tuple[int, int]) -> Image.Image:
    base = gradient(size, (214, 226, 242), (236, 226, 240)).convert("RGBA")
    width, height = size
    glow = Image.new("RGBA", size, (0, 0, 0, 0))
    draw = ImageDraw.Draw(glow)
    draw.ellipse((-width * 0.1, -height * 0.05, width * 0.55, height * 0.42), fill=(255, 255, 255, 70))
    draw.ellipse((width * 0.35, height * 0.45, width * 1.05, height * 1.05), fill=(214, 196, 232, 50))
    # A quiet window behind the popover, so the frame reads as a desktop.
    panel = Image.new("RGBA", size, (0, 0, 0, 0))
    panel_draw = ImageDraw.Draw(panel)
    panel_draw.rounded_rectangle(
        (28 * RENDER, 58 * RENDER, 300 * RENDER, height - 28 * RENDER),
        16 * RENDER,
        fill=(255, 255, 255, 78),
    )
    panel = panel.filter(ImageFilter.GaussianBlur(8))
    base.alpha_composite(glow)
    base.alpha_composite(panel)
    return base


def ease(t: float) -> float:
    return 1 - (1 - t) ** 3


def menu_bar(scene: Image.Image, bolt: bool) -> Image.Image:
    frame = scene.copy()
    width, _ = frame.size
    bar_h = 32 * RENDER
    bar = Image.new("RGBA", (width, bar_h), (255, 255, 255, 214))
    frame.alpha_composite(bar, (0, 0))
    draw = ImageDraw.Draw(frame)
    draw.line((0, bar_h - 1, width, bar_h - 1), fill=(0, 0, 0, 28))

    menus = font(FONT_MEDIUM, 13 * RENDER)
    finder = font(FONT_SEMIBOLD, 13 * RENDER)
    clock_font = font(FONT_MEDIUM, 13 * RENDER)
    meter_font = font(FONT_SEMIBOLD, 13 * RENDER)
    ink = (29, 29, 31)

    draw.text((16 * RENDER, 8 * RENDER), "Finder", font=finder, fill=ink)
    x = 78 * RENDER
    for label in ("File", "Edit", "View", "Go", "Window", "Help"):
        draw.text((x, 8 * RENDER), label, font=menus, fill=ink)
        x += int(draw.textlength(label, font=menus)) + 16 * RENDER

    clock = "Mon 9:41"
    clock_w = int(draw.textlength(clock, font=clock_font))
    draw.text((width - 16 * RENDER - clock_w, 8 * RENDER), clock, font=clock_font, fill=ink)

    title = "64% · 4d 19h"
    title_w = int(draw.textlength(title, font=meter_font))
    bolt_w = 16 * RENDER if bolt else 0
    icon_w = 14 * RENDER
    gap = 6 * RENDER
    cluster = icon_w + gap + bolt_w + title_w
    cluster_right = width - 16 * RENDER - clock_w - 18 * RENDER
    origin = cluster_right - cluster
    draw_bars(draw, origin, 10 * RENDER, RENDER)
    text_x = origin + icon_w + gap
    if bolt:
        draw_bolt(draw, text_x, 9 * RENDER, RENDER)
        text_x += bolt_w
    draw.text((text_x, 8 * RENDER), title, font=meter_font, fill=ink)
    return frame


def load_popover() -> Image.Image:
    src = Image.open(CAPTURE / "popover-codex.png").convert("RGBA")
    # Display width keeps the type readable inside an 800px-wide GIF.
    target_w = 440 * RENDER
    target_h = round(src.height * target_w / src.width)
    sized = src.resize((target_w, target_h), Image.Resampling.LANCZOS)
    radius = round(WINDOW_RADIUS_CSS * target_w / 388)
    sized.putalpha(rounded_mask(sized.size, radius))
    return sized


def place(scene: Image.Image, popover: Image.Image, progress: float, bolt: bool) -> Image.Image:
    frame = menu_bar(scene, bolt)
    width, _ = frame.size
    alpha = popover.split()[-1].point(lambda value: int(value * progress))
    shown = popover.copy()
    shown.putalpha(alpha)
    x = width - shown.width - 20 * RENDER
    start_y = 18 * RENDER
    end_y = 46 * RENDER
    y = int(start_y + (end_y - start_y) * ease(max(progress, 0)))
    shadow = Image.new("RGBA", (shown.width + 80 * RENDER, shown.height + 80 * RENDER), (0, 0, 0, 0))
    shadow_draw = ImageDraw.Draw(shadow)
    shadow_draw.rounded_rectangle(
        (20 * RENDER, 28 * RENDER, shown.width + 40 * RENDER, shown.height + 36 * RENDER),
        28 * RENDER,
        fill=(48, 36, 72, int(78 * progress)),
    )
    shadow = shadow.filter(ImageFilter.GaussianBlur(18 * RENDER))
    frame.alpha_composite(shadow, (x - 30 * RENDER, y - 8 * RENDER))
    frame.alpha_composite(shown, (x, y))
    return frame


def write_frames(scene: Image.Image, popover: Image.Image) -> None:
    if FRAMES.exists():
        shutil.rmtree(FRAMES)
    FRAMES.mkdir(parents=True)
    index = 0

    def save(image: Image.Image) -> None:
        nonlocal index
        image.convert("RGB").save(FRAMES / f"{index:04d}.png", "PNG")
        index += 1

    closed = menu_bar(scene, bolt=False)
    for _ in range(8):
        save(closed)
    steps = 14
    for step in range(1, steps + 1):
        progress = step / steps
        save(place(scene, popover, progress, bolt=progress > 0.45))
    held = place(scene, popover, 1, bolt=True)
    for _ in range(34):
        save(held)
    print(f"Wrote {index} frames")


def encode() -> None:
    pattern = str(FRAMES / "%04d.png")
    gif = DOCS / "usagebar-demo.gif"
    mp4 = DOCS / "usagebar-demo.mp4"
    palette = FRAMES / "palette.png"
    scaled = f"scale={GIF_W}:{GIF_H}:flags=lanczos"
    subprocess.run(
        [
            "ffmpeg", "-y", "-framerate", "12", "-i", pattern,
            "-vf", f"{scaled},palettegen=max_colors=160:stats_mode=single:reserve_transparent=0",
            "-update", "1", "-frames:v", "1",
            str(palette),
        ],
        check=True,
        capture_output=True,
    )
    subprocess.run(
        [
            "ffmpeg", "-y", "-framerate", "12", "-i", pattern, "-i", str(palette),
            "-lavfi", f"{scaled}[x];[x][1:v]paletteuse=dither=bayer:bayer_scale=3:diff_mode=rectangle",
            "-loop", "0",
            str(gif),
        ],
        check=True,
        capture_output=True,
    )
    subprocess.run(
        [
            "ffmpeg", "-y", "-framerate", "12", "-i", pattern,
            "-vf", scaled,
            "-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", "18", "-movflags", "+faststart",
            str(mp4),
        ],
        check=True,
        capture_output=True,
    )
    print(f"Wrote {gif.relative_to(ROOT)} ({gif.stat().st_size / 1_000_000:.1f} MB)")
    print(f"Wrote {mp4.relative_to(ROOT)} ({mp4.stat().st_size / 1_000_000:.1f} MB)")


def main() -> int:
    mapping = {
        "popover-codex.png": "popover-codex.png",
        "popover-claude.png": "popover-claude.png",
        "popover-cursor.png": "popover-cursor.png",
        "popover-opencode.png": "popover-opencode.png",
        "popover-devin.png": "popover-devin.png",
        "popover-antigravity.png": "popover-antigravity.png",
        "settings-tools.png": "settings-tools.png",
        "settings-layout.png": "settings-layout.png",
    }
    for src_name, dest_name in mapping.items():
        src = CAPTURE / src_name
        if not src.exists():
            print(f"missing {src}", file=sys.stderr)
            return 1
        export_still(src, SCREENS / dest_name)

    popover = load_popover()
    # Canvas height hugs the open popover instead of leaving a band of desktop.
    global GIF_H
    top = 46
    bottom = 22
    GIF_H = top + round(popover.height / RENDER) + bottom
    if GIF_H % 2:
        GIF_H += 1
    scene = wallpaper((GIF_W * RENDER, GIF_H * RENDER))
    write_frames(scene, popover)
    encode()
    shutil.rmtree(FRAMES, ignore_errors=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
