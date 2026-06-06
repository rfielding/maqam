#!/usr/bin/env python3
"""Reference guided-reading MP4 renderer for the embroidered carpet image.

This preserves the behavior that looked right:
- static centered carpet
- darkened background
- current location shown as a brighter re-draw of the carpet itself
- no ring cursor, no dot cursor, no camera movement

Usage:
    python3 scripts/make_guided_redraw_mp4.py \
        reference/carpet/embroidered_map_of_musical_territories.png \
        carpet_guided_redraw.mp4

Dependencies:
    pip install pillow imageio imageio-ffmpeg numpy
    # ffmpeg/ffprobe should also be on PATH
"""

from __future__ import annotations

import argparse
import json
import math
import subprocess
from pathlib import Path

import imageio.v2 as imageio
import numpy as np
from PIL import Image, ImageDraw, ImageEnhance, ImageFilter


def fit_center(src: Image.Image, width: int, height: int, margin: int = 40):
    iw, ih = src.size
    scale = min((width - margin) / iw, (height - margin) / ih)
    rw, rh = int(iw * scale), int(ih * scale)
    rw -= rw % 2
    rh -= rh % 2
    resized = src.resize((rw, rh), Image.Resampling.LANCZOS)
    base = Image.new("RGB", (width, height), (6, 6, 10))
    x0 = (width - rw) // 2
    y0 = (height - rh) // 2
    base.paste(resized, (x0, y0))
    return base, x0, y0, rw, rh


def build_default_path(x0: int, y0: int, rw: int, rh: int):
    norm_points = [
        (0.18, 0.32), (0.31, 0.28), (0.45, 0.34),
        (0.60, 0.28), (0.72, 0.30), (0.79, 0.45),
        (0.69, 0.58), (0.56, 0.62), (0.43, 0.56),
        (0.32, 0.64), (0.21, 0.56), (0.15, 0.42),
        (0.18, 0.32),
    ]
    return [(x0 + nx * rw, y0 + ny * rh) for nx, ny in norm_points]


def build_beat_positions(points):
    # growl-ish symbolic sections: phrase, jump, phrase, phrase, jump, return jump.
    beats_per_section = [16, 4, 16, 16, 4, 4]
    section_ranges = [(0, 2), (2, 3), (3, 6), (6, 10), (10, 11), (11, 12)]
    beat_positions = []
    for (a, b), nbeats in zip(section_ranges, beats_per_section):
        p0 = np.array(points[a], dtype=float)
        p1 = np.array(points[b], dtype=float)
        for i in range(nbeats):
            t = i / max(1, nbeats)
            tt = t * t * (3 - 2 * t)
            p = p0 * (1 - tt) + p1 * tt
            beat_positions.append((float(p[0]), float(p[1])))
    beat_positions.append(points[-1])
    return beat_positions


def pos_at_beat(beat_positions, beatf: float):
    bi = int(math.floor(beatf)) % len(beat_positions)
    bj = (bi + 1) % len(beat_positions)
    frac = beatf - math.floor(beatf)
    p0 = np.array(beat_positions[bi], dtype=float)
    p1 = np.array(beat_positions[bj], dtype=float)
    p = p0 * (1 - frac) + p1 * frac
    return float(p[0]), float(p[1])


def make_video(src_path: Path, out_path: Path, bpm: float, seconds: float, fps: int, width: int, height: int):
    src = Image.open(src_path).convert("RGB")
    base, x0, y0, rw, rh = fit_center(src, width, height)

    dark = base.convert("RGBA")
    dark = Image.alpha_composite(dark, Image.new("RGBA", (width, height), (0, 0, 0, 105)))

    vig = Image.new("L", (width, height), 0)
    vd = ImageDraw.Draw(vig)
    for r in range(0, max(width, height), 10):
        alpha = int(120 * (r / max(width, height)))
        vd.ellipse((width // 2 - r, height // 2 - r, width // 2 + r, height // 2 + r), outline=alpha, width=14)
    vig = vig.filter(ImageFilter.GaussianBlur(55))
    black = Image.new("RGBA", (width, height), (0, 0, 0, 0))
    black.putalpha(vig)
    dark = Image.alpha_composite(dark, black)

    bright = base.convert("RGBA")
    bright = ImageEnhance.Brightness(bright).enhance(1.28)
    bright = ImageEnhance.Contrast(bright).enhance(1.10)
    bright = bright.filter(ImageFilter.UnsharpMask(radius=2, percent=140, threshold=2))

    points = build_default_path(x0, y0, rw, rh)
    beat_positions = build_beat_positions(points)
    beats_per_sec = bpm / 60.0
    frame_count = int(seconds * fps)

    writer = imageio.get_writer(
        str(out_path),
        fps=fps,
        codec="libx264",
        quality=8,
        macro_block_size=16,
        ffmpeg_params=[
            "-pix_fmt", "yuv420p",
            "-profile:v", "baseline",
            "-level", "3.1",
            "-movflags", "+faststart",
            "-an",
        ],
    )

    for f in range(frame_count):
        tsec = f / fps
        beatf = tsec * beats_per_sec
        cx, cy = pos_at_beat(beat_positions, beatf)
        phase = beatf % 1.0
        pulse = math.exp(-10.0 * phase)
        radius = 58 + 10 * pulse

        frame = dark.copy()

        mask = Image.new("L", (width, height), 0)
        md = ImageDraw.Draw(mask)
        for k in range(10, 0, -1):
            rr = radius * (k / 3.2)
            alpha = int(10 + 16 * k + 20 * pulse)
            md.ellipse((cx - rr, cy - rr, cx + rr, cy + rr), fill=alpha)
        mask = mask.filter(ImageFilter.GaussianBlur(20))
        frame = Image.composite(bright, frame, mask)

        trail = Image.new("L", (width, height), 0)
        td = ImageDraw.Draw(trail)
        history = []
        for k in range(10):
            hb = beatf - k * 0.6
            if hb < 0:
                continue
            history.append(pos_at_beat(beat_positions, hb))
        for i in range(len(history) - 1):
            td.line((*history[i], *history[i + 1]), fill=max(10, 50 - i * 4), width=18)
        trail = trail.filter(ImageFilter.GaussianBlur(12))
        frame = Image.composite(bright, frame, trail)

        writer.append_data(np.asarray(frame.convert("RGB"), dtype=np.uint8))

    writer.close()

    try:
        probe = subprocess.run(
            [
                "ffprobe", "-v", "error", "-select_streams", "v:0",
                "-show_entries", "stream=codec_name,pix_fmt,width,height,avg_frame_rate,nb_frames,duration",
                "-of", "json", str(out_path),
            ],
            capture_output=True,
            text=True,
            timeout=20,
        )
        if probe.returncode == 0 and probe.stdout.strip():
            print(json.dumps(json.loads(probe.stdout), indent=2))
    except Exception:
        pass


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("source_png", type=Path)
    ap.add_argument("output_mp4", type=Path)
    ap.add_argument("--bpm", type=float, default=180.0)
    ap.add_argument("--seconds", type=float, default=20.0)
    ap.add_argument("--fps", type=int, default=30)
    ap.add_argument("--width", type=int, default=1280)
    ap.add_argument("--height", type=int, default=720)
    args = ap.parse_args()

    make_video(args.source_png, args.output_mp4, args.bpm, args.seconds, args.fps, args.width, args.height)
    print(f"saved {args.output_mp4}")


if __name__ == "__main__":
    main()
