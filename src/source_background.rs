#![allow(dead_code)]

use std::io::Write;
use std::process::Command;

use crate::{sequencer::Phrase, tuning::Maqam};

const W: usize = 1024;
const H: usize = 1024;
const BORDER: usize = 44;
const MAX_HO: u32 = 8;

#[derive(Clone, Copy, Debug)]
struct Pt {
    x: f64,
    y: f64,
}

impl Pt {
    fn lerp(self, other: Pt, t: f64) -> Pt {
        Pt {
            x: self.x * (1.0 - t) + other.x * t,
            y: self.y * (1.0 - t) + other.y * t,
        }
    }

    fn sub(self, other: Pt) -> Pt {
        Pt {
            x: self.x - other.x,
            y: self.y - other.y,
        }
    }

    fn len(self) -> f64 {
        self.x.hypot(self.y)
    }
}

#[derive(Clone)]
struct PhraseBand {
    id: usize,
    src: String,
    color: [u8; 3],
    start: u32,
    end: u32,
    ratio_tokens: Vec<String>,
    ratio_cloud: RatioCloud,
    groups: Vec<u8>,
    jins_boundaries: Vec<usize>,
    scale_len: usize,
}

#[derive(Clone, Debug)]
struct RatioCloud {
    prime_weights: [u32; 6],
    positive_bias: [i32; 6],
    density: u32,
}

fn clamp(x: f64) -> u8 {
    x.round().clamp(0.0, 255.0) as u8
}

fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^ (x >> 16)
}

fn h01(x: u32) -> f64 {
    hash(x) as f64 / u32::MAX as f64
}

fn blend(px: &mut [u8; 3], rgb: [u8; 3], a: f64) {
    let a = a.clamp(0.0, 1.0);
    px[0] = clamp(px[0] as f64 * (1.0 - a) + rgb[0] as f64 * a);
    px[1] = clamp(px[1] as f64 * (1.0 - a) + rgb[1] as f64 * a);
    px[2] = clamp(px[2] as f64 * (1.0 - a) + rgb[2] as f64 * a);
}

fn pix(buf: &mut [[u8; 3]], x: i32, y: i32, rgb: [u8; 3], a: f64) {
    if x < 0 || y < 0 || x >= W as i32 || y >= H as i32 {
        return;
    }
    blend(&mut buf[y as usize * W + x as usize], rgb, a);
}

fn dot(buf: &mut [[u8; 3]], x: f64, y: f64, r: f64, rgb: [u8; 3], a: f64) {
    let rr = r.ceil() as i32;
    for yy in y as i32 - rr..=y as i32 + rr {
        for xx in x as i32 - rr..=x as i32 + rr {
            let dx = xx as f64 + 0.5 - x;
            let dy = yy as f64 + 0.5 - y;
            let d = (dx * dx + dy * dy).sqrt();
            if d <= r {
                pix(buf, xx, yy, rgb, a * (1.0 - d / r));
            }
        }
    }
}

fn line(buf: &mut [[u8; 3]], a: Pt, b: Pt, rgb: [u8; 3], alpha: f64, width: f64) {
    let n = a.sub(b).len().max(1.0) as usize;
    for i in 0..=n {
        let t = i as f64 / n as f64;
        let p = a.lerp(b, t);
        dot(buf, p.x, p.y, width, rgb, alpha);
    }
}

fn fill_polygon(buf: &mut [[u8; 3]], pts: &[Pt], rgb: [u8; 3], alpha: f64) {
    if pts.len() < 3 {
        return;
    }
    let min_y = pts
        .iter()
        .map(|p| p.y.floor() as i32)
        .min()
        .unwrap_or(0)
        .max(0);
    let max_y = pts
        .iter()
        .map(|p| p.y.ceil() as i32)
        .max()
        .unwrap_or(0)
        .min(H as i32 - 1);
    for y in min_y..=max_y {
        let scan = y as f64 + 0.5;
        let mut xs = Vec::new();
        for i in 0..pts.len() {
            let a = pts[i];
            let b = pts[(i + 1) % pts.len()];
            let (y0, y1, x0, x1) = if a.y <= b.y {
                (a.y, b.y, a.x, b.x)
            } else {
                (b.y, a.y, b.x, a.x)
            };
            if scan >= y0 && scan < y1 && (y1 - y0).abs() > 1e-9 {
                let t = (scan - y0) / (y1 - y0);
                xs.push(x0 + (x1 - x0) * t);
            }
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        for pair in xs.chunks(2) {
            if let [xa, xb] = pair {
                for x in xa.floor() as i32..=xb.ceil() as i32 {
                    pix(buf, x, y, rgb, alpha);
                }
            }
        }
    }
}

fn stroke_polygon(buf: &mut [[u8; 3]], pts: &[Pt], rgb: [u8; 3], alpha: f64, width: f64) {
    if pts.len() < 2 {
        return;
    }
    for i in 0..pts.len() {
        line(buf, pts[i], pts[(i + 1) % pts.len()], rgb, alpha, width);
    }
}

fn border(buf: &mut [[u8; 3]]) {
    let gold = [188, 132, 56];
    for inset in [8.0, 18.0, 34.0] {
        let a = if inset < 10.0 {
            0.54
        } else if inset < 20.0 {
            0.30
        } else {
            0.16
        };
        let x0 = inset;
        let x1 = W as f64 - inset;
        let y0 = inset;
        let y1 = H as f64 - inset;
        line(buf, Pt { x: x0, y: y0 }, Pt { x: x1, y: y0 }, gold, a, 1.1);
        line(buf, Pt { x: x1, y: y0 }, Pt { x: x1, y: y1 }, gold, a, 1.1);
        line(buf, Pt { x: x1, y: y1 }, Pt { x: x0, y: y1 }, gold, a, 1.1);
        line(buf, Pt { x: x0, y: y1 }, Pt { x: x0, y: y0 }, gold, a, 1.1);
    }
}

fn fill_background(buf: &mut [[u8; 3]], seed: u32) {
    for y in 0..H {
        for x in 0..W {
            let xf = x as f64;
            let yf = y as f64;
            let warp = 0.70 * (yf * 0.018).sin() + 0.50 * (xf * 0.011).cos();
            let base = 7.0
                + 3.2 * (xf * 0.015).sin()
                + 2.8 * (yf * 0.021).cos()
                + 1.7 * ((xf + yf) * 0.010).sin();
            let vertical = ((xf * 0.72 + warp).sin() * 0.5 + 0.5).powf(7.0);
            let horizontal = ((yf * 0.74 - warp).cos() * 0.5 + 0.5).powf(7.0);
            let diagonal = (((xf + yf) * 0.33).sin() * 0.5 + 0.5).powf(8.0);
            let noise = (hash((x as u32).wrapping_mul(31) ^ (y as u32).wrapping_mul(131) ^ seed)
                % 18) as f64;
            let mut px = [
                clamp((base + noise * 0.20).clamp(0.0, 36.0)),
                clamp((base + 2.0 + noise * 0.16).clamp(0.0, 42.0)),
                clamp((base + 10.0 + noise * 0.36).clamp(0.0, 56.0)),
            ];
            let sheen = 0.052 * vertical + 0.045 * horizontal + 0.028 * diagonal;
            blend(&mut px, [150, 128, 84], sheen);
            if h01((x as u32).wrapping_mul(97) ^ (y as u32).wrapping_mul(193) ^ seed) > 0.985 {
                blend(&mut px, [8, 7, 12], 0.10);
            }
            buf[y * W + x] = px;
        }
    }
}

fn rot(n: u32, x: &mut u32, y: &mut u32, rx: u32, ry: u32) {
    if ry == 0 {
        if rx == 1 {
            *x = n - 1 - *x;
            *y = n - 1 - *y;
        }
        std::mem::swap(x, y);
    }
}

fn hs_for_order(order: u32) -> u32 {
    1 << order
}

fn ha_for_order(order: u32) -> u32 {
    let hs = hs_for_order(order);
    hs * hs
}

fn hilbert_order_for_phrases(phrases: &[Phrase]) -> u32 {
    let total_span: u32 = phrases
        .iter()
        .filter(|p| p.jump.is_none() && p.control.is_none())
        .map(|p| (p.bar.total_subdivs.max(1) * p.repeat.max(1)) as u32)
        .sum();
    let needed = total_span.max(1);
    let mut order = 0u32;
    while order < MAX_HO && ha_for_order(order) < needed {
        order += 1;
    }
    order
}

fn hilbert_xy(mut d: u32, order: u32) -> (u32, u32) {
    let mut x = 0u32;
    let mut y = 0u32;
    let mut s = 1u32;
    let hs = hs_for_order(order);
    while s < hs {
        let rx = (d / 2) & 1;
        let ry = (d ^ rx) & 1;
        rot(s, &mut x, &mut y, rx, ry);
        x += s * rx;
        y += s * ry;
        d /= 4;
        s *= 2;
    }
    (x, y)
}

fn hilbert_to_canvas(gx: u32, gy: u32, order: u32) -> Pt {
    let hs = hs_for_order(order);
    if hs <= 1 {
        return Pt {
            x: W as f64 * 0.5,
            y: H as f64 * 0.5,
        };
    }
    Pt {
        x: BORDER as f64 + gx as f64 / (hs - 1) as f64 * (W - 2 * BORDER) as f64,
        y: BORDER as f64 + gy as f64 / (hs - 1) as f64 * (H - 2 * BORDER) as f64,
    }
}

fn hilbert_cell_size(order: u32) -> f64 {
    let hs = hs_for_order(order);
    let sx = (W - 2 * BORDER) as f64 / hs.max(1) as f64;
    let sy = (H - 2 * BORDER) as f64 / hs.max(1) as f64;
    sx.min(sy)
}

fn control_color(src: &str) -> [u8; 3] {
    if src.starts_with("bpm ") {
        [188, 132, 56]
    } else if src.starts_with("s ") {
        [86, 170, 196]
    } else {
        [126, 126, 150]
    }
}

fn ratio_tokens(p: &Phrase) -> Vec<String> {
    p.bar
        .ratio_strs
        .iter()
        .flat_map(|s| {
            s.split_whitespace()
                .map(|t| t.to_string())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn parse_ratio_token(token: &str) -> Option<(u32, u32)> {
    let (p, q) = token.split_once('/')?;
    let p = p.parse::<u32>().ok()?;
    let q = q.parse::<u32>().ok()?;
    if p == 0 || q == 0 {
        return None;
    }
    Some((p, q))
}

fn gcd_u32(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a.max(1)
}

fn factor_ratio(mut p: u32, mut q: u32) -> [i32; 6] {
    let g = gcd_u32(p, q);
    p /= g;
    q /= g;
    let primes = [2u32, 3, 5, 7, 11, 13];
    let mut out = [0i32; 6];
    for (i, prime) in primes.iter().copied().enumerate() {
        while p % prime == 0 {
            out[i] += 1;
            p /= prime;
        }
        while q % prime == 0 {
            out[i] -= 1;
            q /= prime;
        }
    }
    out
}

fn ratio_cloud(tokens: &[String]) -> RatioCloud {
    let mut cloud = RatioCloud {
        prime_weights: [0; 6],
        positive_bias: [0; 6],
        density: 0,
    };
    for token in tokens {
        let Some((p, q)) = parse_ratio_token(token) else {
            continue;
        };
        let exps = factor_ratio(p, q);
        for (i, exp) in exps.into_iter().enumerate() {
            cloud.prime_weights[i] += exp.unsigned_abs();
            cloud.positive_bias[i] += exp.signum();
            if exp != 0 {
                cloud.density += 1;
            }
        }
    }
    cloud
}

fn phrase_bands(phrases: &[Phrase], ha: u32) -> Vec<PhraseBand> {
    let playable: Vec<&Phrase> = phrases
        .iter()
        .filter(|p| p.jump.is_none() && p.control.is_none())
        .collect();
    let spans: Vec<u32> = playable
        .iter()
        .map(|p| (p.bar.total_subdivs.max(1) * p.repeat.max(1)) as u32)
        .collect();
    let total_span: u32 = spans.iter().copied().sum();
    let margin = if total_span < ha {
        (ha - total_span) / 2
    } else {
        0
    };
    let mut cursor = margin;
    let mut out = Vec::new();
    for (i, p) in playable.iter().enumerate() {
        let tokens = ratio_tokens(p);
        let span = spans[i].max(1);
        let start = cursor;
        let end = (cursor + span.saturating_sub(1)).min(ha - 1);
        cursor = end.saturating_add(1);
        out.push(PhraseBand {
            id: p.id,
            src: p.src.clone(),
            color: Maqam::color_for_ratio_strs(&p.bar.ratio_strs),
            start,
            end,
            ratio_tokens: tokens.clone(),
            ratio_cloud: ratio_cloud(&tokens),
            groups: if p.bar.groups.is_empty() {
                vec![3, 3, 2]
            } else {
                p.bar.groups.clone()
            },
            jins_boundaries: p.bar.jins_boundaries.clone(),
            scale_len: p.bar.frequencies.len().max(1),
        });
    }
    out
}

fn accent_for_token(tokens: &[String], idx: usize) -> [u8; 3] {
    if tokens.is_empty() {
        return [214, 154, 55];
    }
    let t = &tokens[idx % tokens.len()];
    let mut h = 2166136261u32;
    for b in t.as_bytes() {
        h = h.wrapping_mul(16777619) ^ *b as u32;
    }
    match h % 5 {
        0 => [214, 154, 55],
        1 => [96, 204, 190],
        2 => [210, 73, 132],
        3 => [126, 188, 82],
        _ => [226, 98, 40],
    }
}

fn draw_stitch(
    buf: &mut [[u8; 3]],
    p0: Pt,
    p1: Pt,
    color: [u8; 3],
    over: bool,
    energy: f64,
    phase: u32,
) {
    let v = p1.sub(p0);
    let l = v.len();
    if l < 0.5 {
        return;
    }
    let ux = v.x / l;
    let uy = v.y / l;
    let px = -uy;
    let py = ux;
    let mid = Pt {
        x: (p0.x + p1.x) * 0.5,
        y: (p0.y + p1.y) * 0.5,
    };
    let offset = ((phase as f64 * 0.031).sin()) * (0.8 + 2.4 * energy);
    let a = Pt {
        x: p0.x + px * offset,
        y: p0.y + py * offset,
    };
    let b = Pt {
        x: p1.x + px * offset,
        y: p1.y + py * offset,
    };

    if !over {
        line(
            buf,
            a,
            b,
            [16, 10, 12],
            0.34 + 0.14 * energy,
            0.80 + 0.30 * energy,
        );
        line(buf, a, b, color, 0.16 + 0.08 * energy, 0.42 + 0.18 * energy);
        return;
    }

    line(buf, a, b, [22, 14, 18], 0.58, 1.30 + 0.20 * energy);
    line(buf, a, b, color, 0.34 + 0.14 * energy, 0.72 + 0.20 * energy);
    let h = 2.0 + 6.0 * energy;
    line(
        buf,
        Pt {
            x: mid.x - px * h,
            y: mid.y - py * h,
        },
        Pt {
            x: mid.x + px * h,
            y: mid.y + py * h,
        },
        [235, 196, 120],
        0.18 + 0.12 * energy,
        0.30,
    );
}

fn draw_beads(buf: &mut [[u8; 3]], pts: &[Pt], color: [u8; 3], step: usize, r: f64, alpha: f64) {
    for (i, p) in pts.iter().enumerate() {
        if i % step == 0 {
            dot(buf, p.x, p.y, r, color, alpha);
        }
    }
}

fn draw_region_underpaint(buf: &mut [[u8; 3]], pts: &[Pt], color: [u8; 3]) {
    for (i, p) in pts.iter().enumerate() {
        let r = 18.0 + 5.0 * ((i as f64 * 0.17).sin() + 1.0);
        dot(buf, p.x, p.y, r, color, 0.050);
        dot(buf, p.x, p.y, r * 0.62, color, 0.050);
    }
}

fn draw_jins_border(buf: &mut [[u8; 3]], pts: &[Pt], idx: usize, accent: [u8; 3]) {
    if pts.len() < 3 {
        return;
    }
    let i = idx.clamp(1, pts.len().saturating_sub(2));
    let p = pts[i];
    let prev = pts[i - 1];
    let next = pts[i + 1];
    let v = next.sub(prev);
    let l = v.len().max(1.0);
    let px = -v.y / l;
    let py = v.x / l;

    let seam_half = 11.0;
    let a = Pt {
        x: p.x - px * seam_half,
        y: p.y - py * seam_half,
    };
    let b = Pt {
        x: p.x + px * seam_half,
        y: p.y + py * seam_half,
    };
    line(buf, a, b, [16, 12, 16], 0.62, 2.8);
    line(buf, a, b, [214, 154, 55], 0.34, 1.4);
    line(buf, a, b, accent, 0.24, 0.7);

    for k in -2..=2 {
        let off = k as f64 * 4.0;
        let m = Pt {
            x: p.x + px * off,
            y: p.y + py * off,
        };
        dot(
            buf,
            m.x,
            m.y,
            if k == 0 { 1.9 } else { 1.2 },
            [235, 202, 118],
            if k == 0 { 0.82 } else { 0.52 },
        );
    }
}

fn draw_phrase_band(buf: &mut [[u8; 3]], band: &PhraseBand, order: u32) -> Vec<Pt> {
    let step = 3u32;
    let mut pts = Vec::new();
    let mut d = band.start;
    while d <= band.end {
        let (gx, gy) = hilbert_xy(d, order);
        pts.push(hilbert_to_canvas(gx, gy, order));
        if band.end - d < step {
            break;
        }
        d += step;
    }
    draw_region_underpaint(buf, &pts, band.color);

    for (i, seg) in pts.windows(2).enumerate() {
        let energy = 0.25 + 0.75 * h01(hash((band.id as u32).wrapping_mul(911) ^ i as u32 * 17));
        let over = ((i / 2) + band.id) % 2 == 0;
        let accent = accent_for_token(&band.ratio_tokens, i);
        let color = if i % 7 == 0 { accent } else { band.color };
        draw_stitch(
            buf,
            seg[0],
            seg[1],
            color,
            over,
            energy,
            i as u32 + band.id as u32 * 13,
        );
        if i % 19 == 0 {
            dot(buf, seg[0].x, seg[0].y, 1.6 + energy, accent, 0.36);
        }
    }

    if !band.jins_boundaries.is_empty() && band.scale_len > 1 {
        for (j, boundary) in band.jins_boundaries.iter().copied().enumerate() {
            let frac = boundary as f64 / (band.scale_len - 1) as f64;
            let idx = ((pts.len().saturating_sub(1)) as f64 * frac).round() as usize;
            let accent = accent_for_token(&band.ratio_tokens, j * 3 + boundary);
            draw_jins_border(buf, &pts, idx, accent);
        }
    }

    let mut cursor = 0usize;
    for &g in &band.groups {
        let idx = cursor.min(pts.len().saturating_sub(1));
        if let Some(p) = pts.get(idx) {
            dot(buf, p.x, p.y, 3.0 + g as f64 * 0.35, [214, 154, 55], 0.36);
            dot(buf, p.x, p.y, 1.4, [236, 202, 118], 0.56);
        }
        cursor += g as usize * 2;
    }
    draw_beads(buf, &pts, [204, 190, 132], 11, 1.4, 0.22);
    pts
}

fn draw_jump_knots(buf: &mut [[u8; 3]], phrases: &[Phrase], regions: &[(PhraseBand, Vec<Pt>)]) {
    for (idx, p) in phrases.iter().enumerate() {
        let Some(jump) = &p.jump else {
            continue;
        };
        let source_phrase = phrases[..idx]
            .iter()
            .rev()
            .find(|q| q.jump.is_none() && q.control.is_none())
            .map(|q| q.id);
        let Some(src_id) = source_phrase else {
            continue;
        };
        let src = regions
            .iter()
            .find(|(b, _)| b.id == src_id)
            .and_then(|(_, pts)| pts.get(pts.len() / 2))
            .copied();
        let dst = regions
            .iter()
            .find(|(b, _)| b.id == jump.target_id)
            .and_then(|(_, pts)| pts.get(pts.len() / 2))
            .copied();
        if let (Some(a), Some(b)) = (src, dst) {
            let curve = jump_thread_curve(a, b);
            for (i, seg) in curve.windows(2).enumerate() {
                let color = if i % 6 < 3 {
                    [226, 98, 40]
                } else {
                    [214, 154, 55]
                };
                draw_thread_segment(buf, seg[0], seg[1], color, i % 2 == 0);
            }
            let m = Pt {
                x: (a.x + b.x) * 0.5,
                y: (a.y + b.y) * 0.5,
            };
            let loops = jump.times.clamp(1, 12);
            for k in 0..loops {
                let ang = std::f64::consts::TAU * k as f64 / loops as f64;
                dot(
                    buf,
                    m.x + ang.cos() * 18.0,
                    m.y + ang.sin() * 13.0,
                    2.2,
                    [226, 98, 40],
                    0.58,
                );
            }
        }
    }
}

fn draw_global_weave(buf: &mut [[u8; 3]], seed: u32) {
    for i in 0..2600u32 {
        let x = BORDER as f64 + h01(i.wrapping_mul(17) ^ seed) * (W - 2 * BORDER) as f64;
        let y =
            BORDER as f64 + h01(i.wrapping_mul(43) ^ seed.rotate_left(7)) * (H - 2 * BORDER) as f64;
        let a = h01(i.wrapping_mul(97) ^ seed.rotate_left(11)) * std::f64::consts::TAU;
        let l = 1.4 + 2.2 * h01(i.wrapping_mul(131) ^ seed);
        let p0 = Pt {
            x: x - a.cos() * l,
            y: y - a.sin() * l,
        };
        let p1 = Pt {
            x: x + a.cos() * l,
            y: y + a.sin() * l,
        };
        let color = if i % 5 == 0 {
            [214, 154, 55]
        } else {
            [96, 204, 190]
        };
        line(buf, p0, p1, color, 0.08, 0.28);
    }
}

fn draw_phrase_medallion(buf: &mut [[u8; 3]], band: &PhraseBand, pts: &[Pt]) {
    if pts.is_empty() {
        return;
    }
    let c = pts[pts.len() / 2];
    let r = 10.0 + band.groups.len() as f64 * 1.8;
    dot(buf, c.x, c.y, r + 2.0, [14, 12, 18], 0.42);
    dot(buf, c.x, c.y, r, [214, 154, 55], 0.18);
    let loops = band.groups.len().max(3);
    for i in 0..loops {
        let a = std::f64::consts::TAU * i as f64 / loops as f64;
        dot(
            buf,
            c.x + a.cos() * r,
            c.y + a.sin() * r * 0.76,
            1.6,
            [235, 202, 118],
            0.72,
        );
    }
    let id_hash = hash(band.id as u32 * 911);
    for i in 0..(band.id % 5 + 3) {
        let a = std::f64::consts::TAU * i as f64 / ((band.id % 5 + 3) as f64);
        let rr = 3.5 + 4.5 * ((id_hash >> (i % 8)) & 1) as f64;
        dot(
            buf,
            c.x + a.cos() * rr,
            c.y + a.sin() * rr,
            1.1,
            accent_for_token(&band.ratio_tokens, i),
            0.84,
        );
    }
}

fn draw_timeline_band(buf: &mut [[u8; 3]], phrases: &[Phrase], regions: &[(PhraseBand, Vec<Pt>)]) {
    if phrases.is_empty() {
        return;
    }
    let left = BORDER as f64 + 28.0;
    let right = W as f64 - BORDER as f64 - 28.0;
    let y = BORDER as f64 + 26.0;
    line(
        buf,
        Pt { x: left, y },
        Pt { x: right, y },
        [38, 34, 52],
        0.58,
        2.4,
    );

    for (i, p) in phrases.iter().enumerate() {
        let t = if phrases.len() == 1 {
            0.5
        } else {
            i as f64 / (phrases.len() - 1) as f64
        };
        let x = left + (right - left) * t;
        if p.jump.is_some() {
            dot(buf, x, y, 6.0, [18, 12, 18], 0.48);
            for k in 0..6 {
                let a = std::f64::consts::TAU * k as f64 / 6.0;
                dot(
                    buf,
                    x + a.cos() * 6.5,
                    y + a.sin() * 4.6,
                    1.5,
                    [226, 98, 40],
                    0.86,
                );
            }
            continue;
        }
        if p.control.is_some() {
            let c = control_color(&p.src);
            line(
                buf,
                Pt { x, y: y - 9.0 },
                Pt { x, y: y + 9.0 },
                [16, 12, 16],
                0.54,
                2.2,
            );
            line(
                buf,
                Pt { x, y: y - 8.0 },
                Pt { x, y: y + 8.0 },
                c,
                0.66,
                1.0,
            );
            dot(buf, x, y, 2.0, [235, 202, 118], 0.72);
            continue;
        }
        let region = regions.iter().find(|(b, _)| b.id == p.id);
        let c = region
            .map(|(b, _)| b.color)
            .unwrap_or_else(|| Maqam::color_for_ratio_strs(&p.bar.ratio_strs));
        let width = 8.0 + p.bar.groups.len() as f64 * 2.0;
        line(
            buf,
            Pt { x: x - width, y },
            Pt { x: x + width, y },
            [14, 12, 18],
            0.60,
            2.8,
        );
        line(
            buf,
            Pt { x: x - width, y },
            Pt { x: x + width, y },
            c,
            0.70,
            1.1,
        );
        for (gi, g) in p.bar.groups.iter().enumerate() {
            let u = if p.bar.groups.len() == 1 {
                0.5
            } else {
                gi as f64 / (p.bar.groups.len() - 1) as f64
            };
            let bx = x - width + 2.0 + u * (width * 2.0 - 4.0);
            dot(buf, bx, y, 1.0 + *g as f64 * 0.20, [214, 154, 55], 0.76);
        }
    }
}

fn mix_rgb(a: [u8; 3], b: [u8; 3], t: f64) -> [u8; 3] {
    [
        clamp(a[0] as f64 * (1.0 - t) + b[0] as f64 * t),
        clamp(a[1] as f64 * (1.0 - t) + b[1] as f64 * t),
        clamp(a[2] as f64 * (1.0 - t) + b[2] as f64 * t),
    ]
}

fn dominant_prime_color(cloud: &RatioCloud) -> [u8; 3] {
    match cloud
        .prime_weights
        .iter()
        .enumerate()
        .max_by_key(|(_, w)| **w)
        .map(|(i, _)| i)
        .unwrap_or(0)
    {
        0 => [214, 154, 55],
        1 => [96, 204, 190],
        2 => [210, 73, 132],
        3 => [126, 188, 82],
        4 => [226, 98, 40],
        _ => [188, 132, 56],
    }
}

fn maqam_texture_kind(src: &str) -> u8 {
    let s = src.to_lowercase();
    if s.contains("rast") {
        0
    } else if s.contains("bayati") {
        1
    } else if s.contains("saba") {
        2
    } else if s.contains("hijaz") {
        3
    } else {
        4
    }
}

fn countdown_color(base: [u8; 3], remaining: usize, group_len: usize) -> [u8; 3] {
    let rem = remaining.max(1);
    let gl = group_len.max(1);
    let t = if gl <= 1 {
        1.0
    } else {
        (gl.saturating_sub(rem)) as f64 / (gl - 1) as f64
    };
    let hi = mix_rgb(base, [245, 226, 180], 0.34);
    let lo = mix_rgb(base, [20, 16, 24], 0.28);
    mix_rgb(hi, lo, t)
}

fn fill_rect(buf: &mut [[u8; 3]], x0: i32, y0: i32, x1: i32, y1: i32, rgb: [u8; 3], alpha: f64) {
    let xa = x0.max(0).min(W as i32 - 1);
    let xb = x1.max(0).min(W as i32 - 1);
    let ya = y0.max(0).min(H as i32 - 1);
    let yb = y1.max(0).min(H as i32 - 1);
    for y in ya..=yb {
        for x in xa..=xb {
            pix(buf, x, y, rgb, alpha);
        }
    }
}

fn draw_cell_texture(buf: &mut [[u8; 3]], c: Pt, size: i32, cloud: &RatioCloud, base: [u8; 3]) {
    let x0 = c.x.round() as i32 - size / 2;
    let y0 = c.y.round() as i32 - size / 2;
    let x1 = x0 + size - 1;
    let y1 = y0 + size - 1;
    let density = cloud.density.max(1) as i32;
    let warp_gap = (8 - ((density / 4).min(4))).max(3);
    let weft_gap = (7 - ((density / 5).min(3))).max(3);
    let warp_w = if cloud.positive_bias[1] + cloud.positive_bias[4] >= 0 {
        2
    } else {
        1
    };
    let weft_w = if cloud.positive_bias[2] + cloud.positive_bias[5] >= 0 {
        2
    } else {
        1
    };
    let warp = mix_rgb(
        base,
        [228, 208, 160],
        0.18 + (cloud.prime_weights[1] + cloud.prime_weights[4]).min(6) as f64 * 0.02,
    );
    let weft = mix_rgb(
        base,
        [24, 20, 28],
        0.12 + (cloud.prime_weights[2] + cloud.prime_weights[5]).min(6) as f64 * 0.02,
    );
    let accent = dominant_prime_color(cloud);

    let mut vx = x0 + 1;
    let mut col = 0;
    while vx < x1 {
        let over = (col + density) % 2 == 0;
        let strand = if over {
            warp
        } else {
            mix_rgb(warp, [10, 8, 12], 0.34)
        };
        fill_rect(
            buf,
            vx,
            y0 + 1,
            (vx + warp_w).min(x1 - 1),
            y1 - 1,
            strand,
            if over { 0.94 } else { 0.74 },
        );
        vx += warp_gap;
        col += 1;
    }

    let mut hy = y0 + 1;
    let mut row = 0;
    while hy < y1 {
        let over = (row + density + 1) % 2 == 0;
        let strand = if over {
            mix_rgb(weft, accent, 0.12)
        } else {
            weft
        };
        fill_rect(
            buf,
            x0 + 1,
            hy,
            x1 - 1,
            (hy + weft_w).min(y1 - 1),
            strand,
            if over { 0.92 } else { 0.68 },
        );
        hy += weft_gap;
        row += 1;
    }

    let mut kx = x0 + 2;
    while kx < x1 - 2 {
        let ky = y0 + 2 + ((kx - x0) / warp_gap % 3) * ((size / 4).max(2));
        fill_rect(
            buf,
            kx,
            ky,
            (kx + 1).min(x1 - 1),
            (ky + 1).min(y1 - 1),
            accent,
            0.72,
        );
        kx += warp_gap * 2;
    }
}

fn draw_maqam_texture(buf: &mut [[u8; 3]], c: Pt, size: f64, kind: u8, color: [u8; 3]) {
    let x0 = c.x - size * 0.42;
    let x1 = c.x + size * 0.42;
    let y0 = c.y - size * 0.42;
    let y1 = c.y + size * 0.42;
    let ink = mix_rgb(color, [245, 232, 190], 0.24);
    match kind {
        0 => {
            line(
                buf,
                Pt { x: c.x, y: y0 },
                Pt { x: c.x, y: y1 },
                ink,
                0.44,
                0.38,
            );
            line(
                buf,
                Pt { x: x0, y: c.y },
                Pt { x: x1, y: c.y },
                ink,
                0.28,
                0.28,
            );
        }
        1 => {
            line(
                buf,
                Pt { x: x0, y: y1 },
                Pt { x: x1, y: y0 },
                ink,
                0.42,
                0.34,
            );
            line(
                buf,
                Pt { x: x0, y: c.y },
                Pt { x: c.x, y: y0 },
                ink,
                0.26,
                0.26,
            );
        }
        2 => {
            dot(buf, c.x, c.y, size * 0.12, ink, 0.52);
            dot(
                buf,
                c.x - size * 0.18,
                c.y + size * 0.10,
                size * 0.06,
                ink,
                0.28,
            );
            dot(
                buf,
                c.x + size * 0.18,
                c.y - size * 0.10,
                size * 0.06,
                ink,
                0.28,
            );
        }
        3 => {
            line(
                buf,
                Pt { x: x0, y: y0 },
                Pt { x: c.x, y: c.y },
                ink,
                0.42,
                0.34,
            );
            line(
                buf,
                Pt { x: c.x, y: c.y },
                Pt { x: x1, y: y0 },
                ink,
                0.42,
                0.34,
            );
        }
        _ => {
            line(
                buf,
                Pt { x: x0, y: c.y },
                Pt { x: x1, y: c.y },
                ink,
                0.34,
                0.28,
            );
        }
    }
}

fn draw_countdown_mark(
    buf: &mut [[u8; 3]],
    c: Pt,
    size: f64,
    remaining: usize,
    _group_len: usize,
    color: [u8; 3],
) {
    let rem = remaining.max(1);
    let ink = mix_rgb(color, [250, 240, 204], 0.58);
    let off = size * 0.5;
    let r = (size * 0.08).max(1.0);
    let corners = [
        Pt {
            x: c.x - off,
            y: c.y - off,
        },
        Pt {
            x: c.x + off,
            y: c.y - off,
        },
        Pt {
            x: c.x + off,
            y: c.y + off,
        },
        Pt {
            x: c.x - off,
            y: c.y + off,
        },
    ];
    let corner_count = rem.min(4);
    for p in corners.iter().take(corner_count) {
        dot(buf, p.x, p.y, r, ink, 0.86);
    }
    if rem > 4 {
        let extra = rem - 4;
        let step = (size * 0.10).max(1.0);
        let x0 = c.x - step * (extra as f64 - 1.0) * 0.5;
        for i in 0..extra {
            dot(buf, x0 + i as f64 * step, c.y, r * 0.84, ink, 0.74);
        }
    }
}

fn draw_vertex_marker(buf: &mut [[u8; 3]], center: Pt, next: Option<Pt>, size: f64) {
    let tri_r = (size * 0.34).max(6.0);
    dot(
        buf,
        center.x,
        center.y,
        (size * 0.14).max(3.5),
        [248, 248, 248],
        0.98,
    );

    let Some(next) = next else {
        return;
    };

    let v = next.sub(center);
    let len = v.len().max(1.0);
    let ux = v.x / len;
    let uy = v.y / len;
    let px = -uy;
    let py = ux;
    let tip = Pt {
        x: center.x + ux * tri_r * 1.20,
        y: center.y + uy * tri_r * 1.20,
    };
    let base = Pt {
        x: center.x - ux * tri_r * 0.55,
        y: center.y - uy * tri_r * 0.55,
    };
    let left = Pt {
        x: base.x + px * tri_r * 0.92,
        y: base.y + py * tri_r * 0.92,
    };
    let right = Pt {
        x: base.x - px * tri_r * 0.92,
        y: base.y - py * tri_r * 0.92,
    };
    fill_polygon(buf, &[left, tip, right], [248, 248, 248], 0.98);
}

fn draw_hilbert_square(buf: &mut [[u8; 3]], c: Pt, fill: [u8; 3], size: f64, cloud: &RatioCloud) {
    let h = size * 0.5;
    let x0 = (c.x - h).round() as i32;
    let y0 = (c.y - h).round() as i32;
    let x1 = (c.x + h).round() as i32;
    let y1 = (c.y + h).round() as i32;
    fill_rect(buf, x0, y0, x1, y1, fill, 0.98);
    draw_cell_texture(
        buf,
        c,
        ((x1 - x0).abs().min((y1 - y0).abs())).max(4),
        cloud,
        fill,
    );
    let outline = mix_rgb(fill, [12, 10, 16], 0.72);
    line(
        buf,
        Pt {
            x: x0 as f64,
            y: y0 as f64,
        },
        Pt {
            x: x1 as f64,
            y: y0 as f64,
        },
        outline,
        0.96,
        0.32,
    );
    line(
        buf,
        Pt {
            x: x1 as f64,
            y: y0 as f64,
        },
        Pt {
            x: x1 as f64,
            y: y1 as f64,
        },
        outline,
        0.96,
        0.32,
    );
    line(
        buf,
        Pt {
            x: x1 as f64,
            y: y1 as f64,
        },
        Pt {
            x: x0 as f64,
            y: y1 as f64,
        },
        outline,
        0.96,
        0.32,
    );
    line(
        buf,
        Pt {
            x: x0 as f64,
            y: y1 as f64,
        },
        Pt {
            x: x0 as f64,
            y: y0 as f64,
        },
        outline,
        0.96,
        0.32,
    );
}

fn draw_thread_segment(buf: &mut [[u8; 3]], a: Pt, b: Pt, color: [u8; 3], over: bool) {
    let v = b.sub(a);
    let l = v.len().max(1.0);
    let ux = v.x / l;
    let uy = v.y / l;
    let inset = 3.6;
    let p0 = if over {
        a
    } else {
        Pt {
            x: a.x + ux * inset,
            y: a.y + uy * inset,
        }
    };
    let p1 = if over {
        b
    } else {
        Pt {
            x: b.x - ux * inset,
            y: b.y - uy * inset,
        }
    };

    if over {
        line(buf, p0, p1, [20, 14, 18], 0.76, 2.8);
        line(buf, p0, p1, color, 0.76, 1.45);
        line(buf, p0, p1, [235, 202, 118], 0.32, 0.42);
    } else {
        line(buf, p0, p1, [10, 8, 12], 0.86, 3.0);
        line(buf, p0, p1, mix_rgb(color, [14, 10, 16], 0.50), 0.44, 1.0);
    }
}

fn draw_weave_ribbon_segment(
    buf: &mut [[u8; 3]],
    a: Pt,
    b: Pt,
    color: [u8; 3],
    accent: [u8; 3],
    over: bool,
) {
    let v = b.sub(a);
    let l = v.len().max(1.0);
    let ux = v.x / l;
    let uy = v.y / l;
    let px = -uy;
    let py = ux;

    let base0 = Pt {
        x: a.x - px * 3.8,
        y: a.y - py * 3.8,
    };
    let base1 = Pt {
        x: b.x - px * 3.8,
        y: b.y - py * 3.8,
    };
    let base2 = Pt {
        x: b.x + px * 3.8,
        y: b.y + py * 3.8,
    };
    let base3 = Pt {
        x: a.x + px * 3.8,
        y: a.y + py * 3.8,
    };

    let shadow = if over { [14, 10, 16] } else { [8, 6, 10] };
    fill_polygon(
        buf,
        &[base0, base1, base2, base3],
        shadow,
        if over { 0.82 } else { 0.62 },
    );
    fill_polygon(
        buf,
        &[
            Pt {
                x: a.x - px * 3.0,
                y: a.y - py * 3.0,
            },
            Pt {
                x: b.x - px * 3.0,
                y: b.y - py * 3.0,
            },
            Pt {
                x: b.x + px * 3.0,
                y: b.y + py * 3.0,
            },
            Pt {
                x: a.x + px * 3.0,
                y: a.y + py * 3.0,
            },
        ],
        color,
        if over { 0.90 } else { 0.70 },
    );

    let mut t = 0.08;
    while t < 0.92 {
        let c = a.lerp(b, t);
        let bead = if ((t * 100.0) as usize) % 2 == 0 {
            accent
        } else {
            [235, 202, 118]
        };
        line(
            buf,
            Pt {
                x: c.x - px * 2.7,
                y: c.y - py * 2.7,
            },
            Pt {
                x: c.x + px * 2.7,
                y: c.y + py * 2.7,
            },
            bead,
            if over { 0.34 } else { 0.20 },
            0.34,
        );
        t += 0.18;
    }

    line(
        buf,
        Pt {
            x: a.x - px * 1.25 + ux * 0.8,
            y: a.y - py * 1.25 + uy * 0.8,
        },
        Pt {
            x: b.x - px * 1.25 - ux * 0.8,
            y: b.y - py * 1.25 - uy * 0.8,
        },
        [245, 226, 180],
        if over { 0.22 } else { 0.10 },
        0.48,
    );
    line(
        buf,
        Pt {
            x: a.x + px * 1.35 + ux * 0.5,
            y: a.y + py * 1.35 + uy * 0.5,
        },
        Pt {
            x: b.x + px * 1.35 - ux * 0.5,
            y: b.y + py * 1.35 - uy * 0.5,
        },
        mix_rgb(color, [18, 14, 20], 0.45),
        if over { 0.22 } else { 0.12 },
        0.42,
    );
}

fn jump_thread_curve(a: Pt, b: Pt) -> Vec<Pt> {
    let mx = (a.x + b.x) * 0.5;
    let my = (a.y + b.y) * 0.5;
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let l = dx.hypot(dy).max(1.0);
    let ctrl = Pt {
        x: mx - dy / l * 74.0,
        y: my + dx / l * 74.0,
    };
    let mut curve = Vec::new();
    for k in 0..72 {
        let t = k as f64 / 71.0;
        let u = 1.0 - t;
        curve.push(Pt {
            x: u * u * a.x + 2.0 * u * t * ctrl.x + t * t * b.x,
            y: u * u * a.y + 2.0 * u * t * ctrl.y + t * t * b.y,
        });
    }
    curve
}

fn draw_plain_hilbert(buf: &mut [[u8; 3]], bands: &[PhraseBand], order: u32) {
    let step = 1u32;
    let ha = ha_for_order(order);
    let tile_size = hilbert_cell_size(order) * 1.08;
    let mut pts = Vec::with_capacity(ha as usize);
    let mut vertex_markers: Vec<(Pt, Option<Pt>)> = Vec::new();
    let mut d = 0u32;
    while d < ha {
        let (gx, gy) = hilbert_xy(d, order);
        pts.push(hilbert_to_canvas(gx, gy, order));
        d = d.saturating_add(step);
    }

    for &p in &pts {
        fill_rect(
            buf,
            (p.x - 3.0).round() as i32,
            (p.y - 3.0).round() as i32,
            (p.x + 3.0).round() as i32,
            (p.y + 3.0).round() as i32,
            [28, 24, 34],
            0.38,
        );
    }

    for band in bands {
        let start = band.start.min(ha.saturating_sub(1)) as usize;
        let end = band.end.min(ha.saturating_sub(1)) as usize;
        if start >= pts.len() || end >= pts.len() || start >= end {
            continue;
        }

        let texture_kind = maqam_texture_kind(&band.src);
        let mut group_ranges = Vec::new();
        let mut cursor = start;
        for &g in &band.groups {
            let len = (g as usize).max(1);
            let next = (cursor + len).min(end + 1);
            group_ranges.push((cursor, next, len));
            cursor = next;
            if cursor > end {
                break;
            }
        }

        for &(gs, ge, _group_len) in &group_ranges {
            for (local_idx, &p) in pts[gs..ge].iter().enumerate() {
                draw_hilbert_square(buf, p, band.color, tile_size, &band.ratio_cloud);
                draw_maqam_texture(buf, p, tile_size, texture_kind, band.color);
                if local_idx == 0 {
                    let next = pts
                        .get(gs + 1)
                        .copied()
                        .or_else(|| pts.get(gs.saturating_sub(1)).copied());
                    vertex_markers.push((p, next));
                }
            }
        }

        for &idx in &band.jins_boundaries {
            if band.scale_len <= 1 {
                break;
            }
            let frac = idx as f64 / (band.scale_len - 1) as f64;
            let pos = start + (((end - start) as f64) * frac).round() as usize;
            if let Some(&p) = pts.get(pos.min(end)) {
                let accent = dominant_prime_color(&band.ratio_cloud);
                let x = p.x.round() as i32;
                let y = p.y.round() as i32;
                fill_rect(buf, x - 4, y - 1, x + 4, y + 1, accent, 0.82);
            }
        }
    }

    for seg in pts.windows(2) {
        line(buf, seg[0], seg[1], [8, 6, 10], 0.96, 4.2);
        line(buf, seg[0], seg[1], [228, 218, 180], 0.42, 0.62);
    }

    for seg in pts.windows(2) {
        line(buf, seg[0], seg[1], [14, 10, 16], 0.74, 1.35);
    }

    for band in bands {
        let start = band.start.min(ha.saturating_sub(1)) as usize;
        let end = band.end.min(ha.saturating_sub(1)) as usize;
        if start >= pts.len() || end >= pts.len() || start >= end {
            continue;
        }
        for seg in pts[start..=end].windows(2) {
            line(buf, seg[0], seg[1], band.color, 0.58, 0.82);
        }
    }

    for (center, next) in vertex_markers {
        draw_vertex_marker(buf, center, next, tile_size);
    }

    for (i, p) in pts.iter().enumerate().step_by(32) {
        let color = if (i / 32) % 2 == 0 {
            [66, 58, 76]
        } else {
            [54, 48, 66]
        };
        dot(buf, p.x, p.y, 0.8, color, 0.28);
    }
}

fn write_rug_carpet_ppm(path: &str, phrases: &[Phrase]) -> anyhow::Result<()> {
    let mut buf = vec![[0u8; 3]; W * H];
    let seed = 1701u32.wrapping_add(phrases.iter().fold(0u32, |acc, p| {
        acc.wrapping_add(p.id as u32 * 17 + p.src.len() as u32 * 29)
    }));
    fill_background(&mut buf, seed);
    border(&mut buf);
    let order = hilbert_order_for_phrases(phrases);
    let bands = phrase_bands(phrases, ha_for_order(order));
    draw_plain_hilbert(&mut buf, &bands, order);
    border(&mut buf);

    let mut f = std::fs::File::create(path)?;
    write!(f, "P6\n{} {}\n255\n", W, H)?;
    for px in buf {
        f.write_all(&px)?;
    }
    f.flush()?;
    Ok(())
}

pub fn write_generated_source_image_for_phrases(
    path: &str,
    phrases: &[Phrase],
) -> anyhow::Result<()> {
    write_rug_carpet_ppm(path, phrases)
}

pub fn replace_video_with_generated_source_for_phrases(
    path: &str,
    phrases: &[Phrase],
) -> anyhow::Result<()> {
    let mut src = std::env::current_dir()?;
    src.push("carpet.ppm");
    let src = src.to_string_lossy().replace('\\', "/");
    write_rug_carpet_ppm(&src, phrases)?;
    let tmp = format!("{path}.source-background.mp4");
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-loop",
            "1",
            "-framerate",
            "30",
            "-i",
            &src,
            "-i",
            path,
        ])
        .args(["-filter_complex", "[0:v][1:v]blend=all_mode=screen[v]"])
        .args(["-map", "[v]", "-map", "1:a?"])
        .args(["-c:v", "libx264", "-crf", "18", "-pix_fmt", "yuv420p"])
        .args(["-c:a", "copy", "-shortest", &tmp])
        .output();
    match status {
        Ok(output) if output.status.success() => {
            std::fs::rename(&tmp, path)?;
            Ok(())
        }
        Ok(output) => {
            let _ = std::fs::remove_file(&tmp);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let detail = stderr.trim();
            if detail.is_empty() {
                anyhow::bail!(
                    "generated source background failed with status {}",
                    output.status
                );
            }
            anyhow::bail!("generated source background failed: {detail}");
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!("generated source background requires ffmpeg on your PATH");
        }
        Err(error) => Err(error.into()),
    }
}

#[allow(dead_code)]
pub fn replace_video_with_generated_source(path: &str) -> anyhow::Result<()> {
    replace_video_with_generated_source_for_phrases(path, &[])
}
