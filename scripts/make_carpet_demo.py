#!/usr/bin/env python3
from __future__ import annotations
import argparse, hashlib, math, subprocess, sys
from pathlib import Path
from dataclasses import dataclass
import numpy as np
from PIL import Image, ImageDraw, ImageFilter

# Works from any cwd. Default output is <repo>/out/carpet/...

@dataclass
class Phrase:
    idx: int
    src: str
    maqams: list[str]
    rhythm: str

@dataclass
class Jump:
    idx: int
    target: int
    count: int

@dataclass
class Session:
    text: str
    phrases: list[Phrase]
    jumps: list[Jump]
    creates: dict[str, list[str]]
    bpm: float

PALETTE = {
    'hijaz': (118, 34, 74), 'hijaz2': (128, 38, 82),
    'bayati': (34, 103, 72), 'bayati2': (38, 116, 80),
    'saba': (34, 92, 112), 'saba2': (38, 102, 126),
    'rast': (126, 96, 34), 'nahawand': (64, 56, 130),
    'kurd': (74, 58, 122), 'ajam': (128, 102, 44),
    'zaba': (58, 92, 126),
}

RATIO_COLORS = {
    '1/1': (215, 198, 135), '13/12': (210, 78, 132),
    '12/11': (214, 94, 166), '11/10': (220, 110, 145),
    '32/27': (88, 148, 215), '6/5': (116, 214, 132),
    '5/4': (96, 194, 116), '4/3': (86, 168, 218),
    '3/2': (194, 164, 224), '9/8': (144, 200, 92),
    '15/14': (230, 95, 90), '11/8': (220, 140, 74),
}

BUILTIN = {
    'hijaz': ['1/1', '256/243', '81/64', '4/3', '3/2'],
    'bayati': ['1/1', '12/11', '32/27', '4/3', '3/2'],
    'saba': ['1/1', '13/12', '32/27', '6/5'],
    'rast': ['1/1', '9/8', '27/22', '4/3', '3/2'],
}


def repo_root() -> Path:
    p = Path(__file__).resolve()
    for parent in [p.parent, *p.parents]:
        if (parent / 'Cargo.toml').exists() or (parent / '.git').exists():
            return parent
    return Path.cwd()


def parse_mq(path: Path) -> Session:
    text = path.read_text() if path.exists() else ''
    creates = dict(BUILTIN)
    phrases: list[Phrase] = []
    jumps: list[Jump] = []
    bpm = 180.0
    for idx, raw in enumerate(text.splitlines()):
        line = raw.strip()
        if not line:
            continue
        toks = line.split()
        if toks[0] == 'create' and len(toks) >= 3:
            creates[toks[1].lower()] = toks[2:]
        elif toks[0] == 'bpm' and len(toks) >= 2:
            try:
                bpm = float(toks[1])
            except ValueError:
                pass
        elif toks[0] == 'j' and len(toks) >= 3:
            try:
                jumps.append(Jump(idx, int(toks[1]), int(toks[2])))
            except ValueError:
                pass
        else:
            rhythm = None
            for tok in reversed(toks):
                if tok.isdigit() and '0' not in tok:
                    rhythm = tok
                    break
            if rhythm:
                head = line[:-len(rhythm)].strip()
                maqams: list[str] = []
                for part in head.split(','):
                    words = part.split()
                    if len(words) >= 2:
                        maqams.append(words[1].lower())
                    elif len(words) == 1:
                        maqams.append(words[0].lower())
                phrases.append(Phrase(idx, line, maqams or ['unknown'], rhythm))
    if not phrases:
        for i, name in enumerate(creates.keys()):
            phrases.append(Phrase(i, f'g {name} 4444', [name], '4444'))
    return Session(text, phrases, jumps, creates, bpm)


def color_for(name: str, salt: int = 0) -> tuple[int, int, int]:
    if name in PALETTE:
        return PALETTE[name]
    h = int(hashlib.sha256((name + str(salt)).encode()).hexdigest()[:6], 16)
    return (50 + (h & 63), 60 + ((h >> 8) & 75), 80 + ((h >> 16) & 70))


def ratio_pos(r: str, scale: float) -> tuple[float, float]:
    try:
        n, d = [int(x) for x in r.split('/')]
    except Exception:
        return (0.0, 0.0)
    primes = [2, 3, 5, 7, 11, 13]
    x = y = 0.0
    for i, p in enumerate(primes):
        e = 0
        while n and n % p == 0:
            n //= p
            e += 1
        while d and d % p == 0:
            d //= p
            e -= 1
        a = 2 * math.pi * i / len(primes)
        x += e * math.cos(a)
        y += e * math.sin(a)
    return x * scale, y * scale


def ratio_value(r: str) -> float:
    try:
        a, b = r.split('/')
        return float(a) / float(b)
    except Exception:
        return 1.0


def generate_still(session: Session, out: Path, width: int = 1920, height: int = 1080, art: bool = False) -> None:
    seed = int(hashlib.sha256(session.text.encode()).hexdigest()[:16] or '1234', 16)
    rng = np.random.default_rng(seed)
    n = max(1, len(session.phrases))
    centers = []
    for i, ph in enumerate(session.phrases):
        theta = -0.7 + 2 * math.pi * i / n
        cx = width * 0.50 + width * 0.29 * math.cos(theta) + rng.normal(0, width * 0.035)
        cy = height * 0.50 + height * 0.27 * math.sin(theta) + rng.normal(0, height * 0.035)
        beats = sum(int(c) for c in ph.rhythm)
        centers.append((cx, cy, width * (0.15 + 0.006 * beats), height * (0.12 + 0.004 * beats), color_for(ph.maqams[0], i), ph))

    yy, xx = np.mgrid[0:height, 0:width]
    wx = 18 * np.sin(yy * 0.018) + 9 * np.cos((xx + yy) * 0.008)
    wy = 15 * np.cos(xx * 0.014) + 7 * np.sin((xx - yy) * 0.011)
    dists = []
    for i, (cx, cy, rx, ry, col, ph) in enumerate(centers):
        dx = (xx + wx - cx) / rx
        dy = (yy + wy - cy) / ry
        d = dx * dx + dy * dy + 0.10 * np.sin(dx * 6 + dy * 3 + i)
        dists.append(d)
    stack = np.stack(dists)
    labels = np.argmin(stack, axis=0)
    part = np.partition(stack, 1, axis=0)
    seam = np.clip((part[1] - part[0]) * 7, 0, 1)
    img = np.zeros((height, width, 3), dtype=np.float32)

    for i, (cx, cy, rx, ry, col, ph) in enumerate(centers):
        mask = labels == i
        base = np.array(col, dtype=np.float32)
        period = 9 + max(1, sum(int(c) for c in ph.rhythm)) * 0.8
        t1 = np.sin((xx * 0.88 + yy * 0.33 + i * 17) / period)
        t2 = np.sin((xx * -0.43 + yy * 1.04 + i * 11) / (period * 0.72))
        micro = ((xx.astype(np.int32) ^ (yy.astype(np.int32) * 31) ^ (i * 911)) & 15) - 7
        light = (11 * t1 + 8 * t2 + micro).astype(np.float32)
        vals = np.clip(base[None, None, :] + light[:, :, None], 0, 255)
        img[mask] = vals[mask]

    seam_mix = np.array([160, 120, 60], dtype=np.float32)
    boundary = seam < 0.18
    img[boundary] = img[boundary] * 0.62 + seam_mix * 0.38
    img *= 0.78 if art else 0.56
    im = Image.fromarray(np.clip(img, 0, 255).astype(np.uint8), 'RGB')
    draw = ImageDraw.Draw(im, 'RGBA')

    for y in range(0, height, 7):
        draw.line((0, y, width, y), fill=(230, 210, 155, 16 if art else 10), width=1)
    for x in range(0, width, 9):
        draw.line((x, 0, x, height), fill=(25, 18, 32, 24 if art else 18), width=1)

    for i, (cx, cy, rx, ry, col, ph) in enumerate(centers):
        groups = [int(c) for c in ph.rhythm]
        total = max(1, sum(groups))
        acc = 0
        for gi, g in enumerate(groups):
            mid = (acc + g * 0.5) / total
            acc += g
            y = cy - ry * 0.46 + (gi + 0.5) * (ry * 0.92 / max(1, len(groups)))
            length = rx * (0.72 + g * 0.05)
            ang = -0.55 + mid * 1.9
            x0 = cx - length * math.cos(ang) * 0.5
            y0 = y - length * math.sin(ang) * 0.2
            x1 = cx + length * math.cos(ang) * 0.5
            y1 = y + length * math.sin(ang) * 0.2
            band = tuple(min(255, int(c + 70)) for c in col) + (68 if art else 42,)
            draw.line((x0, y0, x1, y1), fill=band, width=max(3, 3 + g))
            for k in range(g):
                u = (k + 0.5) / g
                px = x0 * (1 - u) + x1 * u
                py = y0 * (1 - u) + y1 * u
                draw.ellipse((px - 3, py - 3, px + 3, py + 3), fill=(225, 208, 146, 75 if art else 48))

        for mi, name in enumerate(ph.maqams):
            ratios = session.creates.get(name, BUILTIN.get(name, ['1/1', '4/3', '3/2']))
            rcx = cx + (mi - (len(ph.maqams) - 1) * 0.5) * rx * 0.42
            rcy = cy + ry * 0.20
            pts = []
            for r in ratios:
                ox, oy = ratio_pos(r, min(rx, ry) * 0.20)
                px = rcx + ox
                py = rcy + oy
                pts.append((r, px, py))
                rr = 5 if r == '1/1' else 4
                draw.ellipse((px - rr, py - rr, px + rr, py + rr), fill=RATIO_COLORS.get(r, (178, 148, 194)) + (105 if art else 68,))
            for a in range(len(pts)):
                for b in range(a + 1, len(pts)):
                    r1, x1, y1 = pts[a]
                    r2, x2, y2 = pts[b]
                    qv = ratio_value(r1) / ratio_value(r2)
                    if qv < 1:
                        qv = 1 / qv
                    if abs(qv - 81 / 80) < 0.003:
                        draw.line((x1, y1, x2, y2), fill=(245, 230, 135, 120 if art else 72), width=2)

        local_rng = np.random.default_rng(seed + i * 1009)
        for _ in range(850 if art else 550):
            a = local_rng.random() * 2 * math.pi
            r = math.sqrt(local_rng.random())
            px = cx + math.cos(a) * r * rx * 0.92
            py = cy + math.sin(a) * r * ry * 0.92
            length = 4 + local_rng.random() * 16
            th = a + (local_rng.random() - 0.5) * 1.3
            st = tuple(min(255, int(c + 50)) for c in col) + (34 if art else 22,)
            draw.line((px - math.cos(th) * length * 0.5, py - math.sin(th) * length * 0.5, px + math.cos(th) * length * 0.5, py + math.sin(th) * length * 0.5), fill=st, width=1)

    seam_col = (200, 165, 75, 68 if art else 42)

    def curve(a, b, width_line=6):
        ax, ay = a
        bx, by = b
        dx = bx - ax
        dy = by - ay
        L = max(1, math.hypot(dx, dy))
        cx = (ax + bx) / 2 - dy / L * L * 0.18
        cy = (ay + by) / 2 + dx / L * L * 0.18
        pts = []
        for j in range(49):
            t = j / 48
            u = 1 - t
            pts.append((u * u * ax + 2 * u * t * cx + t * t * bx, u * u * ay + 2 * u * t * cy + t * t * by))
        draw.line(pts, fill=seam_col, width=width_line, joint='curve')

    pos = {ph.idx: (cx, cy) for cx, cy, rx, ry, col, ph in centers}
    phrase_points = [(cx, cy) for cx, cy, rx, ry, col, ph in centers]
    for a, b in zip(phrase_points, phrase_points[1:]):
        curve(a, b, 5)
    for j in session.jumps:
        if j.idx in pos and j.target in pos:
            curve(pos[j.idx], pos[j.target], 8)
            x, y = pos[j.idx]
            for k in range(min(9, j.count)):
                rr = 12 + k * 5
                draw.ellipse((x - rr, y - rr * 0.62, x + rr, y + rr * 0.62), outline=seam_col, width=2)

    im = im.filter(ImageFilter.UnsharpMask(radius=1.4, percent=120, threshold=2))
    out.parent.mkdir(parents=True, exist_ok=True)
    im.save(out)


def run_mp4(source_png: Path, out_mp4: Path, bpm: float) -> None:
    root = repo_root()
    script = root / 'scripts' / 'make_guided_redraw_mp4.py'
    cmd = [sys.executable, str(script), str(source_png), str(out_mp4), '--bpm', str(bpm)]
    subprocess.run(cmd, check=True)


def main() -> None:
    root = repo_root()
    ap = argparse.ArgumentParser()
    ap.add_argument('--mq', type=Path, default=root / 'growl.mq')
    ap.add_argument('--out-dir', type=Path, default=root / 'out' / 'carpet')
    ap.add_argument('--width', type=int, default=1920)
    ap.add_argument('--height', type=int, default=1080)
    ap.add_argument('--art', action='store_true')
    ap.add_argument('--no-mp4', action='store_true')
    args = ap.parse_args()

    session = parse_mq(args.mq)
    png = args.out_dir / 'embroidered_map_of_musical_territories.png'
    mp4 = args.out_dir / 'embroidered_map_guided_180bpm_bright_redraw_fixed.mp4'
    print(f'generating {png}')
    generate_still(session, png, args.width, args.height, args.art)
    if not args.no_mp4:
        print(f'generating {mp4}')
        run_mp4(png, mp4, session.bpm)
    print('done')
    print(png)
    if not args.no_mp4:
        print(mp4)


if __name__ == '__main__':
    main()
