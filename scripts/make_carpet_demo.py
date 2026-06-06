#!/usr/bin/env python3
"""Run the known-good guided-redraw carpet tour.

This script intentionally does NOT procedurally generate the still carpet.
That experiment produced a few big color cells and straight lines, which is
not the desired embroidered carpet language.

Working boundary:
    still PNG  ->  guided bright-redraw MP4

Default paths are repo-root aware, so this works from a normal checkout:

    cd /home/rfielding/code/maqam
    python3 scripts/make_carpet_demo.py

Required input:
    reference/carpet/embroidered_map_of_musical_territories.png

Output:
    out/carpet/embroidered_map_guided_180bpm_bright_redraw_fixed.mp4
"""

from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path


def repo_root() -> Path:
    here = Path(__file__).resolve()
    for parent in [here.parent, *here.parents]:
        if (parent / ".git").exists() or (parent / "Cargo.toml").exists():
            return parent
    return Path.cwd()


def main() -> int:
    root = repo_root()
    ap = argparse.ArgumentParser()
    ap.add_argument(
        "--source-png",
        type=Path,
        default=root / "reference" / "carpet" / "embroidered_map_of_musical_territories.png",
        help="existing still carpet PNG; this script does not generate the still",
    )
    ap.add_argument(
        "--output-mp4",
        type=Path,
        default=root / "out" / "carpet" / "embroidered_map_guided_180bpm_bright_redraw_fixed.mp4",
    )
    ap.add_argument("--bpm", type=float, default=180.0)
    ap.add_argument("--seconds", type=float, default=20.0)
    ap.add_argument("--fps", type=int, default=30)
    args = ap.parse_args()

    source = args.source_png if args.source_png.is_absolute() else (Path.cwd() / args.source_png)
    output = args.output_mp4 if args.output_mp4.is_absolute() else (Path.cwd() / args.output_mp4)

    if not source.exists():
        print("ERROR: missing still carpet PNG", file=sys.stderr)
        print(f"expected: {source}", file=sys.stderr)
        print("", file=sys.stderr)
        print("This script only does the part that currently works:", file=sys.stderr)
        print("    existing embroidered still PNG -> bright-redraw guided MP4", file=sys.stderr)
        print("", file=sys.stderr)
        print("The still-image generator is not solved in code yet.", file=sys.stderr)
        print("Do not trust the old procedural demo; it was visually wrong.", file=sys.stderr)
        return 2

    output.parent.mkdir(parents=True, exist_ok=True)
    script = root / "scripts" / "make_guided_redraw_mp4.py"
    cmd = [
        sys.executable,
        str(script),
        str(source),
        str(output),
        "--bpm", str(args.bpm),
        "--seconds", str(args.seconds),
        "--fps", str(args.fps),
    ]
    print("running:", " ".join(cmd))
    subprocess.run(cmd, check=True)
    print(output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
