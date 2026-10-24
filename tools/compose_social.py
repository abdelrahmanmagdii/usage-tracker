#!/usr/bin/env python3
"""Place a cropped UsageBar recording on a brand gradient.

shot.txt keys:
  kind=card|pair
  crop=W:H:X:Y          (kind=card)
  main=W:H:X:Y          (kind=pair, the dashboard)
  widget=W:H:X:Y        (kind=pair, the floating meter)

Cards are scaled with lanczos, masked to rounded corners, and given a shadow
rendered once. The desktop, taskbar, and Test Mode watermark are never part
of the crop.
"""

from __future__ import annotations

import argparse
import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter


def parse_rect(value: str) -> tuple[int, int, int, int]:
    width, height, x, y = (int(float(part)) for part in value.split(":"))
    return width - width % 2, height - height % 2, x, y


def load_shot(path: Path) -> dict[str, str]:
    shot: dict[str, str] = {}
    for line in path.read_text().splitlines():
        if "=" not in line:
            continue
        key, value = line.split("=", 1)
        shot[key.strip()] = value.strip()
    return shot


def even(value: int) -> int:
    return max(2, value - value % 2)


def fit_scale(group_w: int, group_h: int, canvas_w: int, canvas_h: int) -> float:
    return min((canvas_w * 0.84) / group_w, (canvas_h * 0.86) / group_h)


def gradient(width: int, height: int) -> Image.Image:
    image = Image.new("RGB", (width, height))
    pixels = image.load()
    for y in range(height):
        for x in range(width):
            blend = 0.18 * x / max(1, width - 1) + 0.82 * y / max(1, height - 1)
            pixels[x, y] = (
                int(26 + 84 * blend),
                int(20 + 68 * blend),
                int(51 + 178 * blend),
            )
    return image


def rounded_mask(width: int, height: int, radius: int) -> Image.Image:
    mask = Image.new("L", (width, height), 0)
    draw = ImageDraw.Draw(mask)
    draw.rounded_rectangle((0, 0, width - 1, height - 1), radius=radius, fill=255)
    return mask


def shadow_image(mask: Image.Image) -> tuple[Image.Image, int]:
    blur = 18
    pad = blur * 2
    canvas = Image.new("L", (mask.width + pad * 2, mask.height + pad * 2), 0)
    canvas.paste(mask, (pad, pad))
    blurred = canvas.filter(ImageFilter.GaussianBlur(blur))
    # A soft black shadow. Alpha is the blurred mask, pulled down so it
    # does not look like a second copy of the window.
    alpha = blurred.point(lambda value: int(value * 0.45))
    image = Image.new("RGBA", alpha.size, (0, 0, 0, 0))
    image.putalpha(alpha)
    return image, pad


def run(cmd: list[str]) -> None:
    subprocess.run(cmd, check=True)


def encode_mp4(
    raw: Path,
    shot: dict[str, str],
    dest: Path,
    canvas_w: int,
    canvas_h: int,
    ss: float,
    duration: float,
    work: Path,
) -> None:
    gradient(canvas_w, canvas_h).save(work / "bg.png")
    kind = shot.get("kind", "card")
    if kind == "pair":
        main = parse_rect(shot["main"])
        widget = parse_rect(shot["widget"])
        gap = 36
        scale = fit_scale(main[0] + gap + widget[0], max(main[1], widget[1]), canvas_w, canvas_h)
        layers = [("main", main), ("widget", widget)]
    else:
        crop = parse_rect(shot["crop"])
        scale = fit_scale(crop[0], crop[1], canvas_w, canvas_h)
        gap = 0
        layers = [("card", crop)]

    sized: list[tuple[str, tuple[int, int, int, int], int, int, int]] = []
    for name, rect in layers:
        out_w = even(round(rect[0] * scale))
        out_h = even(round(rect[1] * scale))
        radius = max(16, round(min(out_w, out_h) * 0.05))
        mask = rounded_mask(out_w, out_h, radius)
        mask.save(work / f"{name}-mask.png")
        shade, pad = shadow_image(mask)
        shade.save(work / f"{name}-shadow.png")
        sized.append((name, rect, out_w, out_h, pad))

    if kind == "pair":
        (_, _, mw, mh, _), (_, _, ww, wh, _) = sized
        scaled_gap = even(round(gap * scale))
        group_w = mw + scaled_gap + ww
        group_h = max(mh, wh)
        origin_x = even((canvas_w - group_w) // 2)
        origin_y = even((canvas_h - group_h) // 2)
        positions = {
            "main": (origin_x, origin_y),
            "widget": (origin_x + mw + scaled_gap, origin_y + (group_h - wh)),
        }
    else:
        _, _, cw, ch, _ = sized[0]
        positions = {"card": (even((canvas_w - cw) // 2), even((canvas_h - ch) // 2))}

    inputs = [
        "ffmpeg",
        "-y",
        "-v",
        "error",
        "-ss",
        f"{ss:.3f}",
        "-t",
        f"{duration:.3f}",
        "-i",
        str(raw),
        "-f",
        "lavfi",
        "-t",
        f"{duration:.3f}",
        "-i",
        "anullsrc=channel_layout=stereo:sample_rate=48000",
        "-loop",
        "1",
        "-t",
        f"{duration:.3f}",
        "-i",
        str(work / "bg.png"),
    ]
    graph: list[str] = []
    current = "2:v"
    for index, (name, rect, out_w, out_h, pad) in enumerate(sized):
        crop_w, crop_h, crop_x, crop_y = rect
        inputs.extend(["-loop", "1", "-t", f"{duration:.3f}", "-i", str(work / f"{name}-mask.png")])
        inputs.extend(["-loop", "1", "-t", f"{duration:.3f}", "-i", str(work / f"{name}-shadow.png")])
        mask_input = 3 + index * 2
        shadow_input = mask_input + 1
        x, y = positions[name]
        graph.append(
            f"[0:v]crop={crop_w}:{crop_h}:{crop_x}:{crop_y},"
            f"scale={out_w}:{out_h}:flags=lanczos,format=rgba[src{index}]"
        )
        graph.append(f"[src{index}][{mask_input}:v]alphamerge[card{index}]")
        nxt = f"lay{index}"
        graph.append(
            f"[{current}][{shadow_input}:v]overlay=x={x - pad + 8}:y={y - pad + 16}:format=auto[sh{index}]"
        )
        graph.append(f"[sh{index}][card{index}]overlay=x={x}:y={y}:format=auto[{nxt}]")
        current = nxt
    graph.append(f"[{current}]format=yuv420p[pix]")
    inputs.extend(
        [
            "-filter_complex",
            ";".join(graph),
            "-map",
            "[pix]",
            "-map",
            "1:a",
            "-c:v",
            "libx264",
            "-profile:v",
            "high",
            "-preset",
            "veryfast",
            "-crf",
            "18",
            "-pix_fmt",
            "yuv420p",
            "-r",
            "30",
            "-movflags",
            "+faststart",
            "-c:a",
            "aac",
            "-b:a",
            "128k",
            "-shortest",
        ]
    )
    dest.parent.mkdir(parents=True, exist_ok=True)
    run(inputs + [str(dest)])


def encode_gif(raw: Path, shot: dict[str, str], dest: Path, ss: float, duration: float, width: int) -> None:
    crop = shot["main"] if shot.get("kind") == "pair" else shot["crop"]
    crop_w, crop_h, _, _ = parse_rect(crop)
    out_w = width - width % 2
    out_h = even(round(crop_h * (out_w / crop_w)))
    if out_h > 1050:
        out_h = 1050
        out_w = even(round(crop_w * (out_h / crop_h)))
    vf = (
        f"crop={crop},fps=10,scale={out_w}:{out_h}:flags=lanczos,split[a][b];"
        "[a]palettegen=max_colors=128:stats_mode=diff[p];"
        "[b][p]paletteuse=dither=bayer:bayer_scale=3:diff_mode=rectangle"
    )
    dest.parent.mkdir(parents=True, exist_ok=True)
    run(
        [
            "ffmpeg",
            "-y",
            "-v",
            "error",
            "-ss",
            f"{ss:.3f}",
            "-t",
            f"{duration:.3f}",
            "-i",
            str(raw),
            "-filter_complex",
            vf,
            "-loop",
            "0",
            str(dest),
        ]
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--raw", type=Path, required=True)
    parser.add_argument("--shot", type=Path, required=True)
    parser.add_argument("--dest", type=Path, required=True)
    parser.add_argument("--mode", choices=("mp4", "gif"), required=True)
    parser.add_argument("--width", type=int, default=1920)
    parser.add_argument("--height", type=int, default=1080)
    parser.add_argument("--ss", type=float, default=0)
    parser.add_argument("--t", type=float, default=26)
    args = parser.parse_args()
    shot = load_shot(args.shot)
    if args.mode == "gif":
        encode_gif(args.raw, shot, args.dest, args.ss, args.t, args.width)
    else:
        with tempfile.TemporaryDirectory(prefix="usagebar-compose-") as temp:
            encode_mp4(
                args.raw,
                shot,
                args.dest,
                args.width,
                args.height,
                args.ss,
                args.t,
                Path(temp),
            )
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except subprocess.CalledProcessError as error:
        print(error, file=sys.stderr)
        sys.exit(error.returncode)
