#!/usr/bin/env python3
"""Build the README demo GIF and framed stills from docs/_capture PNGs."""

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

CANVAS = (1280, 720)
BAR_H = 36
STILL_CANVAS = (980, 1324)


def font(size: int, bold: bool = False) -> ImageFont.FreeTypeFont | ImageFont.ImageFont:
    names = (
        ("/System/Library/Fonts/SFNS.ttf", "/System/Library/Fonts/SFNSRounded.ttf")
        if bold
        else ("/System/Library/Fonts/SFNS.ttf",)
    )
    candidates = [
        *names,
        "/System/Library/Fonts/SFNSMono.ttf",
        "/Library/Fonts/SF-Pro-Text-Regular.otf",
        "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
        "/System/Library/Fonts/Helvetica.ttc",
    ]
    for path in candidates:
        try:
            return ImageFont.truetype(path, size)
        except OSError:
            continue
    return ImageFont.load_default()


def clean_alpha(im: Image.Image, cutoff: int = 18) -> Image.Image:
    im = im.convert("RGBA")
    pixels = im.getdata()
    im.putdata([(0, 0, 0, 0) if px[3] < cutoff else px for px in pixels])
    return im


def desktop() -> Image.Image:
    w, h = CANVAS
    base = Image.new("RGB", CANVAS, (18, 12, 36))
    overlay = Image.new("RGB", CANVAS, (18, 12, 36))
    draw = ImageDraw.Draw(overlay)
    cx, cy = int(w * 0.45), int(h * 0.08)
    for r in range(max(w, h), 0, -8):
        t = 1 - r / max(w, h)
        color = (
            int(18 + 72 * t),
            int(12 + 28 * t),
            int(36 + 90 * t),
        )
        draw.ellipse((cx - r, cy - r, cx + r, cy + r), fill=color)
    return Image.blend(base, overlay, 0.92).convert("RGBA")


def draw_meter_mark(draw: ImageDraw.ImageDraw, x: int, y: int) -> None:
    draw.rounded_rectangle((x, y - 5, x + 5, y + 7), 2, fill=(255, 255, 255))
    draw.rounded_rectangle((x + 8, y + 1, x + 13, y + 7), 2, fill=(232, 168, 140))


def draw_zap(draw: ImageDraw.ImageDraw, x: int, y: int) -> None:
    draw.polygon(
        [(x + 5, y - 7), (x, y + 0), (x + 3, y + 0), (x - 2, y + 8), (x + 4, y + 1), (x + 1, y + 1)],
        fill=(255, 214, 92),
    )


def menubar(base: Image.Image) -> Image.Image:
    im = base.copy()
    w, _ = CANVAS
    bar = Image.new("RGBA", (w, BAR_H), (12, 8, 22, 168))
    im.alpha_composite(bar, (0, 0))
    draw = ImageDraw.Draw(im)
    ui = font(13, bold=True)
    small = font(12, bold=True)
    draw_meter_mark(draw, 16, 18)
    draw.text((36, 9), "UsageBar", font=ui, fill=(246, 243, 255))

    pill = "64%  ·  37%"
    pw = int(draw.textlength(pill, font=small)) + 48
    px = w - pw - 168
    draw.rounded_rectangle((px, 6, px + pw, 30), 12, fill=(16, 14, 22, 230))
    draw_zap(draw, px + 12, 18)
    draw.text((px + 26, 9), pill, font=small, fill=(255, 255, 255))
    draw.text((w - 150, 9), "Fri  9:41", font=small, fill=(236, 232, 246))
    return im


def ease(t: float) -> float:
    return 1 - (1 - t) ** 3


def place_popover(scene: Image.Image, popover: Image.Image, progress: float) -> Image.Image:
    frame = scene.copy()
    w, h = CANVAS
    target_w = 460
    scale = target_w / popover.width
    sized = popover.resize((target_w, int(popover.height * scale)), Image.Resampling.LANCZOS)
    alpha = sized.split()[-1].point(lambda a: int(a * progress))
    sized.putalpha(alpha)
    x = w - sized.width - 28
    start_y = BAR_H - 18
    end_y = BAR_H + 14
    y = int(start_y + (end_y - start_y) * ease(progress))
    shadow = Image.new("RGBA", (sized.width + 40, sized.height + 40), (0, 0, 0, 0))
    sdraw = ImageDraw.Draw(shadow)
    sdraw.rounded_rectangle((10, 16, sized.width + 30, sized.height + 28), 28, fill=(20, 10, 40, int(90 * progress)))
    shadow = shadow.filter(ImageFilter.GaussianBlur(16))
    frame.alpha_composite(shadow, (x - 20, y - 10))
    frame.alpha_composite(sized, (x, y))
    return frame


def write_frames(popover: Image.Image) -> list[Path]:
    if FRAMES.exists():
        shutil.rmtree(FRAMES)
    FRAMES.mkdir(parents=True)
    scene = menubar(desktop())
    paths: list[Path] = []
    index = 0

    def save(im: Image.Image) -> None:
        nonlocal index
        path = FRAMES / f"{index:04d}.png"
        im.convert("RGB").save(path, "PNG")
        paths.append(path)
        index += 1

    for _ in range(10):
        save(scene)
    steps = 14
    for i in range(steps):
        save(place_popover(scene, popover, (i + 1) / steps))
    held = place_popover(scene, popover, 1)
    for _ in range(46):
        save(held)
    return paths


def encode(paths: list[Path]) -> None:
    pattern = str(FRAMES / "%04d.png")
    gif = DOCS / "usagebar-demo.gif"
    mp4 = DOCS / "usagebar-demo.mp4"
    palette = FRAMES / "palette.png"
    subprocess.run(
        [
            "ffmpeg", "-y", "-framerate", "16", "-i", pattern,
            "-vf", "palettegen=max_colors=160:stats_mode=diff",
            str(palette),
        ],
        check=True,
        capture_output=True,
    )
    subprocess.run(
        [
            "ffmpeg", "-y", "-framerate", "16", "-i", pattern, "-i", str(palette),
            "-lavfi", "paletteuse=dither=sierra2_4a:diff_mode=rectangle",
            "-loop", "0",
            str(gif),
        ],
        check=True,
        capture_output=True,
    )
    subprocess.run(
        [
            "ffmpeg", "-y", "-framerate", "16", "-i", pattern,
            "-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", "18",
            str(mp4),
        ],
        check=True,
        capture_output=True,
    )
    print(f"Wrote {gif} ({gif.stat().st_size / 1_000_000:.1f} MB)")
    print(f"Wrote {mp4} ({mp4.stat().st_size / 1_000_000:.1f} MB)")


def frame_still(src: Path, dest: Path, canvas: tuple[int, int] = STILL_CANVAS) -> None:
    card = clean_alpha(Image.open(src))
    pad = 56
    max_w, max_h = canvas[0] - pad * 2, canvas[1] - pad * 2
    scale = min(max_w / card.width, max_h / card.height)
    sized = card.resize((int(card.width * scale), int(card.height * scale)), Image.Resampling.LANCZOS)
    bg = Image.new("RGBA", canvas, (0, 0, 0, 0))
    x = (canvas[0] - sized.width) // 2
    y = (canvas[1] - sized.height) // 2
    shadow = Image.new("RGBA", canvas, (0, 0, 0, 0))
    sdraw = ImageDraw.Draw(shadow)
    sdraw.rounded_rectangle((x + 6, y + 18, x + sized.width - 6, y + sized.height + 10), 36, fill=(40, 28, 70, 70))
    bg.alpha_composite(shadow.filter(ImageFilter.GaussianBlur(22)))
    bg.alpha_composite(sized, (x, y))
    dest.parent.mkdir(parents=True, exist_ok=True)
    bg.save(dest, "PNG")
    print(f"Wrote {dest} {bg.size}")


def readme_still(src: Path, dest: Path) -> None:
    card = clean_alpha(Image.open(src))
    canvas = (888, 1600)
    scale = min((canvas[0] - 80) / card.width, (canvas[1] - 80) / card.height)
    sized = card.resize((int(card.width * scale), int(card.height * scale)), Image.Resampling.LANCZOS)
    bg = Image.new("RGB", canvas, (246, 243, 255))
    draw = ImageDraw.Draw(bg)
    draw.rectangle((0, 0, canvas[0], canvas[1]), fill=(246, 243, 255))
    overlay = Image.new("RGBA", canvas, (0, 0, 0, 0))
    x = (canvas[0] - sized.width) // 2
    y = (canvas[1] - sized.height) // 2
    overlay.alpha_composite(sized, (x, y))
    out = Image.alpha_composite(bg.convert("RGBA"), overlay)
    out.convert("RGB").save(dest, "PNG")
    print(f"Wrote {dest} {out.size}")


def main() -> int:
    mapping = {
        "popover-codex-transparent.png": "popover-codex.png",
        "popover-claude.png": "popover-claude.png",
        "popover-cursor.png": "popover-cursor.png",
        "popover-opencode.png": "popover-opencode.png",
        "settings-tools.png": "settings-tools.png",
        "settings-layout.png": "settings-layout.png",
    }
    for src_name, dest_name in mapping.items():
        src = CAPTURE / src_name
        if not src.exists() and src_name.startswith("popover-codex"):
            src = CAPTURE / "popover-codex.png"
        if not src.exists():
            print(f"missing {src}", file=sys.stderr)
            return 1
        frame_still(src, SCREENS / dest_name)

    readme_still(CAPTURE / "popover-codex-transparent.png", DOCS / "usagebar-popover.png")
    readme_still(CAPTURE / "popover-claude.png", DOCS / "usagebar-claude.png")

    popover = clean_alpha(Image.open(CAPTURE / "popover-codex-transparent.png"))
    encode(write_frames(popover))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
