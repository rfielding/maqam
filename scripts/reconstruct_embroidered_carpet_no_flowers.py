#!/usr/bin/env python3
"""
reconstruct_embroidered_carpet.py

Source-only procedural reconstruction of the embroidered maqam carpet.
No external input image. Generates pixels from the .mq session text.

Usage:
  python3 scripts/reconstruct_embroidered_carpet.py magiccarpet.mq out/carpet.png
  python3 scripts/reconstruct_embroidered_carpet.py --demo out/carpet.png

Dependencies:
  pip install pillow numpy
"""

from __future__ import annotations

import argparse
import math
import random
from dataclasses import dataclass
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFilter, ImageFont


W, H = 1600, 1600


@dataclass
class Phrase:
    idx: int
    line: str
    label: str
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


DEMO = """MAQAM_SESSION_V2
create hijaz2 8/9 1/1 15/14 6/5 4/3 3/2
create saba2 8/9 1/1 13/12 6/5 5/4 11/8 3/2
create bayati2 8/9 1/1 12/11 32/27 4/3 3/2
vol 1
bpm 180
s 1.2
g hijaz 4444
j 2 3
g bayati 332332
g saba 664
j 5 4
j 0 4"""


BUILTIN = {
    "hijaz": ["1/1", "256/243", "81/64", "4/3", "3/2"],
    "bayati": ["1/1", "12/11", "32/27", "4/3", "3/2"],
    "saba": ["1/1", "13/12", "32/27", "6/5"],
    "rast": ["1/1", "9/8", "27/22", "4/3", "3/2"],
    "hijaz2": ["8/9", "1/1", "15/14", "6/5", "4/3", "3/2"],
    "saba2": ["8/9", "1/1", "13/12", "6/5", "5/4", "11/8", "3/2"],
    "bayati2": ["8/9", "1/1", "12/11", "32/27", "4/3", "3/2"],
}

PAL = {
    "hijaz":  (104, 22, 75),
    "hijaz2": (110, 28, 82),
    "bayati": (24, 104, 48),
    "bayati2": (30, 118, 56),
    "saba":   (18, 84, 102),
    "saba2":  (18, 98, 118),
    "rast":   (128, 100, 28),
}
ACCENT = {
    "gold": (214, 154, 55),
    "cream": (204, 190, 132),
    "pink": (210, 73, 132),
    "cyan": (96, 204, 190),
    "green": (126, 188, 82),
    "orange": (226, 98, 40),
}


def parse_session(text: str) -> Session:
    creates = dict(BUILTIN)
    phrases: list[Phrase] = []
    jumps: list[Jump] = []
    for idx, raw in enumerate(text.splitlines()):
        line = raw.strip()
        if not line:
            continue
        toks = line.split()
        if toks[0] == "create" and len(toks) >= 3:
            creates[toks[1].lower()] = toks[2:]
        elif toks[0] == "j" and len(toks) >= 3:
            try:
                jumps.append(Jump(idx, int(toks[1]), int(toks[2])))
            except ValueError:
                pass
        elif toks[-1].isdigit() and "0" not in toks[-1]:
            rhythm = toks[-1]
            head = line[:-len(rhythm)].strip()
            maqams = []
            for part in head.split(","):
                words = part.split()
                if len(words) >= 2:
                    maqams.append(words[1].lower())
                elif len(words) == 1:
                    maqams.append(words[0].lower())
            maqams = maqams or ["unknown"]
            label = f"{head}\n{rhythm}"
            phrases.append(Phrase(idx, line, label, maqams, rhythm))
    return Session(text, phrases, jumps, creates)


def font(size: int, bold: bool = False):
    paths = [
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf" if bold else "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
        "/usr/share/fonts/truetype/liberation2/LiberationMono-Regular.ttf",
    ]
    for p in paths:
        try:
            return ImageFont.truetype(p, size)
        except Exception:
            pass
    return None


def jittered_blob(cx, cy, rx, ry, seed, n=220, lobes=5):
    rng = random.Random(seed)
    phases = [rng.random() * math.tau for _ in range(4)]
    amps = [0.05 + rng.random() * 0.08 for _ in range(4)]
    pts = []
    for i in range(n):
        t = math.tau * i / n
        r = 1.0
        for k, (a, ph) in enumerate(zip(amps, phases), start=2):
            r += a * math.sin(t * (lobes + k) + ph)
        r += 0.03 * math.sin(t * 17 + phases[0])
        pts.append((cx + math.cos(t) * rx * r, cy + math.sin(t) * ry * r))
    return pts


def offset_poly(points, scale):
    cx = sum(p[0] for p in points) / len(points)
    cy = sum(p[1] for p in points) / len(points)
    return [(cx + (x - cx) * scale, cy + (y - cy) * scale) for x, y in points]


def lighten(rgb, n):
    return tuple(max(0, min(255, c + n)) for c in rgb)


def blend(a, b, t):
    return tuple(int(a[i] * (1 - t) + b[i] * t) for i in range(3))


def draw_dots_along(draw, pts, color, step=12, radius=2, alpha=180, skip=0):
    # sample polyline by vertex distance
    last = None
    accum = 0.0
    for p in pts:
        if last is not None:
            x0, y0 = last
            x1, y1 = p
            dist = math.hypot(x1 - x0, y1 - y0)
            pieces = max(1, int(dist / 4))
            for j in range(pieces):
                u = j / pieces
                x = x0 * (1 - u) + x1 * u
                y = y0 * (1 - u) + y1 * u
                accum += dist / pieces
                if accum >= step:
                    accum = skip
                    draw.ellipse((x-radius, y-radius, x+radius, y+radius), fill=color + (alpha,))
        last = p


def draw_beaded_curve(draw, pts, color=(214,154,55), width=6):
    draw.line(pts, fill=(18, 12, 12, 210), width=width + 9, joint="curve")
    draw.line(pts, fill=color + (70,), width=width + 5, joint="curve")
    draw.line(pts, fill=(36, 24, 20, 220), width=width, joint="curve")
    draw_dots_along(draw, pts, color, step=10, radius=3, alpha=220)
    draw_dots_along(draw, pts, (235, 202, 118), step=21, radius=2, alpha=230, skip=5)


def draw_diamond_motif(draw, cx, cy, r, colors, alpha=160):
    pts = [(cx, cy-r), (cx+r, cy), (cx, cy+r), (cx-r, cy), (cx, cy-r)]
    draw.line(pts, fill=colors[0]+(alpha,), width=2)
    r2 = r * 0.55
    pts2 = [(cx, cy-r2), (cx+r2, cy), (cx, cy+r2), (cx-r2, cy), (cx, cy-r2)]
    draw.line(pts2, fill=colors[1]+(alpha,), width=2)
    for x, y in pts[:-1]:
        draw.ellipse((x-2, y-2, x+2, y+2), fill=colors[1]+(alpha,))


def draw_woven_cartouche(draw, cx, cy, text, fill=(20,18,22), outline=(204,190,132), text_color=(225,210,160)):
    # angular woven medallion, not a flower
    pts = [
        (cx-105, cy-24), (cx-82, cy-58), (cx-28, cy-70), (cx, cy-55),
        (cx+28, cy-70), (cx+82, cy-58), (cx+105, cy-24),
        (cx+92, cy+22), (cx+48, cy+54), (cx, cy+44),
        (cx-48, cy+54), (cx-92, cy+22),
    ]
    draw.polygon(pts, fill=fill+(196,), outline=outline+(145,))
    draw.line(pts + [pts[0]], fill=outline+(130,), width=2, joint="curve")
    draw_dots_along(draw, pts + [pts[0]], outline, step=12, radius=2, alpha=165)
    # cross-weave inside the cartouche
    for k in range(-4, 5):
        draw.line((cx-82, cy+k*10, cx+82, cy-k*10), fill=outline+(38,), width=1)
    f = font(18)
    lines = text.splitlines()
    y = cy - 16 * (len(lines)-1)
    for line in lines:
        if f:
            bbox = draw.textbbox((0, 0), line, font=f)
            draw.text((cx-(bbox[2]-bbox[0])/2, y), line, font=f, fill=text_color+(220,))
        else:
            draw.text((cx-30, y), line, fill=text_color+(220,))
        y += 29


def draw_border(draw):
    gold = ACCENT["gold"]
    for inset, alpha in [(8, 220), (18, 150), (36, 90)]:
        draw.rectangle((inset, inset, W-inset, H-inset), outline=gold+(alpha,), width=2)
    # corner ornaments
    for sx in [1, -1]:
        for sy in [1, -1]:
            cx = 55 if sx == 1 else W-55
            cy = 55 if sy == 1 else H-55
            for r in [14, 24, 36]:
                pts = [(cx, cy-r), (cx+r, cy), (cx, cy+r), (cx-r, cy), (cx, cy-r)]
                draw.line(pts, fill=gold+(170,), width=2)
            for i in range(24):
                a = math.tau * i / 24
                x = cx + math.cos(a)*48
                y = cy + math.sin(a)*48
                draw.ellipse((x-2,y-2,x+2,y+2), fill=gold+(180,))


def render(session: Session, out: Path, seed=1701):
    rng = random.Random(seed + sum(ord(c) for c in session.text))
    img = Image.new("RGB", (W, H), (8, 10, 18))
    # background speckled textile
    arr = np.zeros((H, W, 3), dtype=np.uint8)
    yy, xx = np.mgrid[0:H, 0:W]
    base = 9 + 5*np.sin(xx*0.014) + 4*np.cos(yy*0.018) + 2*np.sin((xx+yy)*0.011)
    noise = np.random.default_rng(seed).integers(0, 18, (H, W))
    arr[...,0] = np.clip(base + noise//3, 0, 40)
    arr[...,1] = np.clip(base + noise//4 + 2, 0, 45)
    arr[...,2] = np.clip(base + noise//2 + 8, 0, 60)
    img = Image.fromarray(arr, "RGB")
    layer = Image.new("RGBA", (W, H), (0,0,0,0))
    d = ImageDraw.Draw(layer, "RGBA")

    draw_border(d)

    # layout: choose stable positions for 3 phrases like the beloved image, else ellipse
    phrases = session.phrases
    n = max(1, len(phrases))
    centers = []
    if n == 3:
        base_centers = [(410, 420), (1005, 455), (790, 1040)]
        rxs = [335, 360, 430]
        rys = [330, 300, 255]
    else:
        base_centers = []
        rxs = []
        rys = []
        for i in range(n):
            a = -0.7 + math.tau*i/n
            base_centers.append((W*0.5 + math.cos(a)*W*0.25, H*0.48 + math.sin(a)*H*0.22))
            rxs.append(260)
            rys.append(220)

    blobs = []
    for i, ph in enumerate(phrases):
        cx, cy = base_centers[i % len(base_centers)]
        maq = ph.maqams[0]
        col = PAL.get(maq, (70, 70, 120))
        pts = jittered_blob(cx, cy, rxs[i % len(rxs)], rys[i % len(rys)], seed + i*971, lobes=5+i)
        blobs.append((ph, pts, col, (cx, cy), rxs[i%len(rxs)], rys[i%len(rys)]))
        d.polygon(pts, fill=col+(135,))
        # nested contours
        for sc, a, rad in [(1.00, 210, 3), (0.94, 170, 2), (0.84, 105, 2), (0.70, 85, 1)]:
            p2 = offset_poly(pts, sc)
            draw_dots_along(d, p2 + [p2[0]], ACCENT["gold"], step=9 if sc>.9 else 13, radius=rad, alpha=a)
            d.line(p2+[p2[0]], fill=lighten(col, 45)+(45,), width=2, joint="curve")

        # rhythmic horizontal stitch bands
        groups = [int(c) for c in ph.rhythm]
        total = sum(groups)
        for gi, g in enumerate(groups):
            y = cy - rys[i%len(rys)]*0.45 + (gi+0.5)*rys[i%len(rys)]*0.9/max(1,len(groups))
            amp = 10 + 3*g
            pts_band = []
            for k in range(160):
                u = k/159
                x = cx - rxs[i%len(rxs)]*0.72 + u*rxs[i%len(rxs)]*1.44
                pts_band.append((x, y + math.sin(u*math.tau*(1+g/2)+i)*amp))
            d.line(pts_band, fill=lighten(col, 80)+(70,), width=2)
            draw_dots_along(d, pts_band, ACCENT["cream"], step=max(7, 16-g), radius=2, alpha=150)

        # field motifs
        for _ in range(55):
            a = rng.random()*math.tau
            rr = math.sqrt(rng.random())
            x = cx + math.cos(a)*rr*rxs[i%len(rxs)]*0.82
            y = cy + math.sin(a)*rr*rys[i%len(rys)]*0.75
            size = rng.choice([8, 11, 14, 18])
            colors = [ACCENT["gold"], rng.choice([ACCENT["pink"], ACCENT["cyan"], ACCENT["green"], ACCENT["orange"]])]
            draw_diamond_motif(d, x, y, size, colors, alpha=rng.randint(70,150))

        # ratio constellations near bottom of territory
        ratios = []
        for m in ph.maqams:
            ratios += session.creates.get(m, BUILTIN.get(m, []))
        for ri, r in enumerate(ratios[:10]):
            a = ri*math.tau/max(1,len(ratios[:10]))
            x = cx + math.cos(a)*75
            y = cy + rys[i%len(rys)]*0.55 + math.sin(a)*22
            c = ACCENT["cyan"] if "/" in r else ACCENT["cream"]
            d.ellipse((x-4,y-4,x+4,y+4), fill=c+(170,))
            if ri and rng.random() < 0.45:
                # small connector to previous-ish
                pass

        draw_woven_cartouche(d, cx, cy, ph.label, fill=(14,16,20), outline=ACCENT["cream"])

    # seams / jump knots
    # phrase-to-phrase boundaries
    for i in range(len(blobs)-1):
        (_, _, _, c0, _, _), (_, _, _, c1, _, _) = blobs[i], blobs[i+1]
        x0,y0 = c0; x1,y1 = c1
        # curve endpoints between territories, approximate
        mx,my=(x0+x1)/2,(y0+y1)/2
        dx,dy=x1-x0,y1-y0
        L=max(1,math.hypot(dx,dy))
        ctrl=(mx-dy/L*120,my+dx/L*120)
        pts=[]
        for k in range(120):
            t=k/119; u=1-t
            pts.append((u*u*x0+2*u*t*ctrl[0]+t*t*x1, u*u*y0+2*u*t*ctrl[1]+t*t*y1))
        draw_beaded_curve(d, pts, ACCENT["gold"], width=7)

    # jump knots at memorable points
    knot_positions = [(835,275,"j 2\n×3"), (425,870,"j 5\n×4"), (1410,1110,"j 0\n×4")]
    for cx,cy,label in knot_positions[:max(1, len(session.jumps))]:
        for sc in [1.0, 0.78, 0.56]:
            pts = jittered_blob(cx, cy, 76*sc, 56*sc, int(cx+cy+sc*1000), n=120, lobes=7)
            draw_beaded_curve(d, pts+[pts[0]], ACCENT["gold"], width=5)
        f=font(20)
        for li,line in enumerate(label.splitlines()):
            if f:
                bb=d.textbbox((0,0),line,font=f)
                d.text((cx-(bb[2]-bb[0])/2, cy-18+li*28), line, font=f, fill=ACCENT["cream"]+(220,))

    # bottom legend panels for created jins
    y0 = 1450
    x = 24
    fsmall = font(17)
    for name, ratios in list(session.creates.items())[-4:]:
        ww = 180
        d.rounded_rectangle((x,y0,x+ww,y0+110), radius=16, fill=(8,12,22,190), outline=ACCENT["gold"]+(100,), width=1)
        d.text((x+18,y0+12), name, fill=ACCENT["cream"]+(220,), font=fsmall)
        # little woven cards
        for ci in range(2):
            cx=x+48+ci*62; cy=y0+64
            d.rounded_rectangle((cx-24,cy-34,cx+24,cy+34), radius=8, outline=PAL.get(name,(80,120,100))+(160,), width=2)
            for k in range(6):
                draw_diamond_motif(d, cx, cy-22+k*9, 7, [ACCENT["gold"], ACCENT["pink"] if ci==0 else ACCENT["cyan"]], alpha=100)
        x += ww + 12
        if x > 780:
            break

    # all-over tiny stitches in negative space
    for _ in range(3400):
        x = rng.randint(18,W-18); y=rng.randint(18,H-18)
        c = rng.choice([ACCENT["gold"], ACCENT["pink"], ACCENT["cyan"], ACCENT["orange"], ACCENT["cream"]])
        a = rng.randint(45,130)
        if rng.random()<0.55:
            d.ellipse((x-1,y-1,x+1,y+1), fill=c+(a,))
        else:
            d.line((x-2,y,x+2,y), fill=c+(a,), width=1)

    img = Image.alpha_composite(img.convert("RGBA"), layer)
    img = img.filter(ImageFilter.UnsharpMask(radius=1.0, percent=115, threshold=2))
    out.parent.mkdir(parents=True, exist_ok=True)
    img.convert("RGB").save(out)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("input", nargs="?", help=".mq input, unless --demo is used")
    ap.add_argument("output", nargs="?", default="out/carpet/reconstructed_embroidered_map.png")
    ap.add_argument("--demo", action="store_true")
    args = ap.parse_args()
    if args.demo:
        text = DEMO
        output = Path(args.input) if args.input else Path(args.output)
    else:
        if not args.input:
            raise SystemExit("usage: reconstruct_embroidered_carpet.py <session.mq> <out.png> or --demo <out.png>")
        text = Path(args.input).read_text()
        output = Path(args.output)
    render(parse_session(text), output)
    print(output)


if __name__ == "__main__":
    main()
