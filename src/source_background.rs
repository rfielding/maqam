use std::io::Write;
use std::process::{Command, Stdio};

use crate::sequencer::Phrase;

const W: usize = 1280;
const H: usize = 720;
const BORDER: usize = 44;
const HO: u32 = 8;
const HS: u32 = 1 << HO;
const HA: u32 = HS * HS;

#[derive(Clone, Copy)]
struct Pt { x: f64, y: f64 }

impl Pt {
    fn sub(self, o: Pt) -> Pt { Pt { x: self.x - o.x, y: self.y - o.y } }
    fn len(self) -> f64 { self.x.hypot(self.y) }
}

fn clamp(x: f64) -> u8 { x.round().clamp(0.0, 255.0) as u8 }

fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^ (x >> 16)
}

fn h01(x: u32) -> f64 { hash(x) as f64 / u32::MAX as f64 }

fn blend(px: &mut [u8; 3], rgb: [u8; 3], a: f64) {
    let a = a.clamp(0.0, 1.0);
    px[0] = clamp(px[0] as f64 * (1.0 - a) + rgb[0] as f64 * a);
    px[1] = clamp(px[1] as f64 * (1.0 - a) + rgb[1] as f64 * a);
    px[2] = clamp(px[2] as f64 * (1.0 - a) + rgb[2] as f64 * a);
}

fn pix(buf: &mut [[u8; 3]], x: i32, y: i32, rgb: [u8; 3], a: f64) {
    if x < 0 || y < 0 || x >= W as i32 || y >= H as i32 { return; }
    blend(&mut buf[y as usize * W + x as usize], rgb, a);
}

fn dot(buf: &mut [[u8; 3]], x: f64, y: f64, r: f64, rgb: [u8; 3], a: f64) {
    let rr = r.ceil() as i32;
    for yy in y as i32 - rr..=y as i32 + rr {
        for xx in x as i32 - rr..=x as i32 + rr {
            let dx = xx as f64 + 0.5 - x;
            let dy = yy as f64 + 0.5 - y;
            let d = (dx * dx + dy * dy).sqrt();
            if d <= r { pix(buf, xx, yy, rgb, a * (1.0 - d / r)); }
        }
    }
}

fn line(buf: &mut [[u8; 3]], a: Pt, b: Pt, rgb: [u8; 3], alpha: f64, width: f64) {
    let n = a.sub(b).len().max(1.0) as usize;
    for i in 0..=n {
        let t = i as f64 / n as f64;
        dot(buf, a.x * (1.0 - t) + b.x * t, a.y * (1.0 - t) + b.y * t, width, rgb, alpha);
    }
}

fn color(p: &Phrase, i: usize) -> [u8; 3] {
    let s = p.src.to_lowercase();
    if s.contains("hijaz") { [96, 25, 80] }
    else if s.contains("bayati") { [24, 102, 54] }
    else if s.contains("saba") { [18, 86, 112] }
    else if s.contains("rast") { [118, 90, 30] }
    else { [[72,42,110],[24,88,96],[96,36,48],[30,92,58],[90,50,82]][i % 5] }
}

fn anchors(n: usize) -> Vec<Pt> {
    if n == 3 { return vec![Pt{x:350.0,y:265.0}, Pt{x:855.0,y:265.0}, Pt{x:640.0,y:500.0}]; }
    if n == 4 { return vec![Pt{x:352.0,y:240.0}, Pt{x:885.0,y:250.0}, Pt{x:410.0,y:510.0}, Pt{x:850.0,y:505.0}]; }
    (0..n.max(1)).map(|i| {
        let a = -0.7 + std::f64::consts::TAU * i as f64 / n.max(1) as f64;
        Pt { x: W as f64 * 0.50 + a.cos() * W as f64 * 0.25, y: H as f64 * 0.50 + a.sin() * H as f64 * 0.18 }
    }).collect()
}

fn radii(n: usize, i: usize) -> (f64, f64) {
    if n == 3 { return match i { 0 => (330.0,235.0), 1 => (350.0,230.0), _ => (410.0,195.0) }; }
    if n == 4 { return match i { 0 => (300.0,205.0), 1 => (320.0,205.0), 2 => (320.0,185.0), _ => (315.0,180.0) }; }
    ((300.0 - n as f64 * 4.0).clamp(210.0, 285.0), (190.0 - n as f64 * 2.0).clamp(135.0, 180.0))
}

fn field_texture(buf: &mut [[u8; 3]]) {
    for y in 0..H {
        for x in 0..W {
            let idx = y * W + x;
            let xf = x as f64;
            let yf = y as f64;
            let warp = 0.7 * (yf * 0.018).sin() + 0.5 * (xf * 0.011).cos();
            let vertical = ((xf * 0.70 + warp).sin() * 0.5 + 0.5).powf(7.0);
            let horizontal = ((yf * 0.74 - warp).cos() * 0.5 + 0.5).powf(7.0);
            let diagonal = (((xf + yf) * 0.36).sin() * 0.5 + 0.5).powf(9.0);
            let knot = h01((x as u32).wrapping_mul(97) ^ (y as u32).wrapping_mul(193));
            let light = 0.065 * vertical + 0.060 * horizontal + 0.035 * diagonal;
            if light > 0.01 { blend(&mut buf[idx], [156,132,86], light); }
            if knot > 0.985 { blend(&mut buf[idx], [7,6,10], 0.085); }
        }
    }
}

fn border(buf: &mut [[u8; 3]]) {
    let gold = [194,135,54];
    for inset in [8,18,36] {
        let a = if inset == 8 { 0.55 } else if inset == 18 { 0.32 } else { 0.18 };
        let x0 = inset as f64; let x1 = (W - inset) as f64; let y0 = inset as f64; let y1 = (H - inset) as f64;
        line(buf, Pt{x:x0,y:y0}, Pt{x:x1,y:y0}, gold, a, 1.2);
        line(buf, Pt{x:x1,y:y0}, Pt{x:x1,y:y1}, gold, a, 1.2);
        line(buf, Pt{x:x1,y:y1}, Pt{x:x0,y:y1}, gold, a, 1.2);
        line(buf, Pt{x:x0,y:y1}, Pt{x:x0,y:y0}, gold, a, 1.2);
    }
}

fn wobble(x: f64, y: f64, seed: u32) -> f64 {
    1.0 + 0.080 * (x * 0.014 + seed as f64 * 0.011).sin()
        + 0.060 * (y * 0.018 + seed as f64 * 0.017).cos()
        + 0.035 * ((x + y) * 0.010 + seed as f64 * 0.007).sin()
}

fn normalized_distance(x: f64, y: f64, a: Pt, rx: f64, ry: f64, seed: u32) -> f64 {
    let dx = (x - a.x) / rx;
    let dy = (y - a.y) / ry;
    let d = (dx.abs().powf(2.25) + dy.abs().powf(2.05)).powf(1.0 / 2.15);
    d / wobble(x, y, seed)
}

fn rot(n: u32, x: &mut u32, y: &mut u32, rx: u32, ry: u32) {
    if ry == 0 {
        if rx == 1 { *x = n - 1 - *x; *y = n - 1 - *y; }
        std::mem::swap(x, y);
    }
}

fn hilbert_index(mut x: u32, mut y: u32) -> u32 {
    let mut d = 0;
    let mut s = HS / 2;
    while s > 0 {
        let rx = if (x & s) > 0 { 1 } else { 0 };
        let ry = if (y & s) > 0 { 1 } else { 0 };
        d += s * s * ((3 * rx) ^ ry);
        rot(s, &mut x, &mut y, rx, ry);
        s /= 2;
    }
    d
}

fn hilbert_xy(mut d: u32) -> (u32, u32) {
    let mut x = 0u32;
    let mut y = 0u32;
    let mut s = 1u32;
    while s < HS {
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

fn phrase_weights(playable: &[&Phrase]) -> Vec<usize> {
    playable.iter().map(|p| {
        let group_sum = p.bar.groups.iter().map(|&g| g as usize).sum::<usize>();
        group_sum.max(1) * p.repeat.max(1)
    }).collect()
}

fn owner_from_hilbert(h: u32, weights: &[usize]) -> usize {
    let total = weights.iter().copied().sum::<usize>().max(1);
    let target = (h as usize * total) / HA as usize;
    let mut acc = 0usize;
    for (i, &w) in weights.iter().enumerate() {
        acc += w;
        if target < acc { return i; }
    }
    weights.len().saturating_sub(1)
}

fn hilbert_to_canvas(gx: u32, gy: u32) -> Pt {
    let x = BORDER as f64 + gx as f64 / (HS - 1) as f64 * (W - 2 * BORDER) as f64;
    let y = BORDER as f64 + gy as f64 / (HS - 1) as f64 * (H - 2 * BORDER) as f64;
    let cx = W as f64 * 0.5;
    let cy = H as f64 * 0.5;
    let dx = (x - cx) * 0.86;
    let dy = (y - cy) * 0.86;
    let a = std::f64::consts::FRAC_PI_4;
    Pt { x: cx + dx * a.cos() - dy * a.sin(), y: cy + dx * a.sin() + dy * a.cos() * 0.82 }
}

fn token_hash(p: &Phrase, k: usize) -> u32 {
    if !p.bar.ratio_strs.is_empty() {
        let s = &p.bar.ratio_strs[k % p.bar.ratio_strs.len()];
        let mut h = 2166136261u32;
        for b in s.as_bytes() { h = h.wrapping_mul(16777619) ^ *b as u32; }
        h
    } else if !p.bar.groups.is_empty() {
        (p.bar.groups[k % p.bar.groups.len()] as u32).wrapping_mul(2654435761)
    } else {
        (p.id as u32).wrapping_mul(7919).wrapping_add(k as u32)
    }
}

fn draw_ratio_stitch(buf: &mut [[u8; 3]], p0: Pt, p1: Pt, rgb: [u8; 3], energy: f64, phase: u32) {
    let v = p1.sub(p0);
    let l = v.len();
    if l < 0.5 { return; }
    let ux = v.x / l;
    let uy = v.y / l;
    let px = -uy;
    let py = ux;
    let mid = Pt { x: (p0.x + p1.x) * 0.5, y: (p0.y + p1.y) * 0.5 };
    let off = ((phase as f64 * 0.031).sin()) * (0.7 + 2.8 * energy);
    let a = Pt { x: p0.x + px * off, y: p0.y + py * off };
    let b = Pt { x: p1.x + px * off, y: p1.y + py * off };
    line(buf, a, b, rgb, 0.20 + 0.18 * energy, 0.45 + 0.35 * energy);

    let h = 3.0 + 10.0 * energy;
    let c1 = Pt { x: mid.x - px * h, y: mid.y - py * h };
    let c2 = Pt { x: mid.x + px * h, y: mid.y + py * h };
    line(buf, c1, c2, [224, 176, 92], 0.18 + 0.16 * energy, 0.32);
    if phase % 7 == 0 { dot(buf, mid.x, mid.y, 1.4 + 1.8 * energy, [235, 196, 120], 0.32); }
}

fn draw_hilbert_ratio_hatching(buf: &mut [[u8; 3]], playable: &[&Phrase], weights: &[usize], inside: &[bool], owner: &[usize], colors: &[[u8; 3]]) {
    if playable.is_empty() { return; }
    let step = 5u32;
    let mut d = 0u32;
    while d + 1 < HA {
        let who = owner_from_hilbert(d, weights);
        let (x0, y0) = hilbert_xy(d);
        let (x1, y1) = hilbert_xy(d + 1);
        let p0 = hilbert_to_canvas(x0, y0);
        let p1 = hilbert_to_canvas(x1, y1);
        let mid = Pt { x: (p0.x + p1.x) * 0.5, y: (p0.y + p1.y) * 0.5 };
        let ix = mid.x.round() as i32;
        let iy = mid.y.round() as i32;
        if ix > 1 && iy > 1 && ix < (W - 2) as i32 && iy < (H - 2) as i32 {
            let idx = iy as usize * W + ix as usize;
            if inside[idx] && owner[idx] == who {
                let th = token_hash(playable[who], d as usize / step as usize);
                let energy = 0.25 + 0.75 * h01(th ^ d.wrapping_mul(17));
                let mut rgb = colors[who];
                if th & 1 == 0 { rgb = [214, 156, 72]; }
                if th & 7 == 3 { rgb = [82, 194, 178]; }
                draw_ratio_stitch(buf, p0, p1, rgb, energy, th ^ d);
            }
        }
        d += step;
    }
}

fn draw_rhythm_code(buf: &mut [[u8; 3]], p: &Phrase, c: Pt, idx: usize) {
    let groups = if p.bar.groups.is_empty() { vec![3,3,2] } else { p.bar.groups.clone() };
    let total = groups.iter().map(|&g| g as usize).sum::<usize>().max(1);
    let y0 = c.y + if idx < 2 { 72.0 } else { -70.0 };
    let x0 = c.x - 85.0;
    let w = 170.0;
    let mut acc = 0usize;
    for (gi, &g) in groups.iter().enumerate() {
        let center = acc as f64 + g as f64 * 0.5;
        let x = x0 + center / total as f64 * w;
        let height = 12.0 + g as f64 * 8.0;
        let y1 = y0 - height * 0.5;
        let y2 = y0 + height * 0.5;
        let col = if gi % 2 == 0 { [226, 184, 92] } else { [96, 210, 190] };
        line(buf, Pt { x, y: y1 }, Pt { x, y: y2 }, [38, 24, 22], 0.46, 1.8);
        line(buf, Pt { x, y: y1 }, Pt { x, y: y2 }, col, 0.44, 0.8);
        for k in 0..g.max(1) {
            let yy = y1 + (k as f64 + 0.5) / g.max(1) as f64 * (y2 - y1);
            dot(buf, x, yy, 2.2, col, 0.62);
        }
        acc += g as usize;
    }
}

fn draw_jump_knot(buf: &mut [[u8; 3]], from: Pt, to: Pt, times: usize) {
    let mid = Pt { x: (from.x + to.x) * 0.5, y: (from.y + to.y) * 0.5 };
    let r = 6.0 + times.min(6) as f64 * 1.7;
    dot(buf, mid.x, mid.y, r + 2.5, [26, 16, 20], 0.36);
    for i in 0..times.max(1).min(12) {
        let a = std::f64::consts::TAU * i as f64 / times.max(1).min(12) as f64;
        dot(buf, mid.x + a.cos() * r, mid.y + a.sin() * r * 0.72, 1.7, [216, 158, 66], 0.48);
    }
}

fn write_rug_carpet_ppm(path: &str, phrases: &[Phrase]) -> anyhow::Result<()> {
    let mut buf = vec![[0u8; 3]; W * H];
    for y in 0..H {
        for x in 0..W {
            let xf = x as f64;
            let yf = y as f64;
            let base = 7.0 + 3.0 * (xf * 0.015).sin() + 3.0 * (yf * 0.021).cos() + 2.0 * ((xf + yf) * 0.010).sin();
            let n = (hash((x as u32).wrapping_mul(31) ^ (y as u32).wrapping_mul(131)) % 18) as f64;
            buf[y * W + x] = [clamp(base + n * 0.18), clamp(base + 2.0 + n * 0.14), clamp(base + 12.0 + n * 0.42)];
        }
    }
    field_texture(&mut buf);

    let playable: Vec<&Phrase> = phrases.iter().filter(|p| p.jump.is_none() && p.control.is_none()).collect();
    let n = playable.len();
    if n > 0 {
        let pts = anchors(n);
        let weights = phrase_weights(&playable);
        let colors: Vec<[u8; 3]> = playable.iter().enumerate().map(|(i, p)| color(p, i)).collect();
        let mut owner = vec![usize::MAX; W * H];
        let mut inside = vec![false; W * H];
        for y in BORDER..(H - BORDER) {
            for x in BORDER..(W - BORDER) {
                let xf = x as f64 + 0.5;
                let yf = y as f64 + 0.5;
                let mut union = f64::INFINITY;
                for (i, a) in pts.iter().enumerate() {
                    let (rx, ry) = radii(n, i);
                    let nd = normalized_distance(xf, yf, *a, rx, ry, 1009 + i as u32 * 733);
                    if nd < union { union = nd; }
                }
                if union < 1.0 {
                    let gx = (((x - BORDER) as u32 * HS) / (W - 2 * BORDER) as u32).min(HS - 1);
                    let gy = (((y - BORDER) as u32 * HS) / (H - 2 * BORDER) as u32).min(HS - 1);
                    let h = hilbert_index(gx, gy);
                    let who = owner_from_hilbert(h, &weights);
                    let idx = y * W + x;
                    owner[idx] = who;
                    inside[idx] = true;
                    let edge = ((1.0 - union) / 0.20).clamp(0.0, 1.0);
                    let rib = 0.5 + 0.5 * ((xf * 0.060 + yf * 0.021 + who as f64).sin());
                    blend(&mut buf[idx], colors[who], edge * (0.34 + 0.12 * rib));
                }
            }
        }

        draw_hilbert_ratio_hatching(&mut buf, &playable, &weights, &inside, &owner, &colors);

        for y in (BORDER + 1)..(H - BORDER - 1) {
            for x in (BORDER + 1)..(W - BORDER - 1) {
                let idx = y * W + x;
                if !inside[idx] { continue; }
                let here = owner[idx];
                let right_i = y * W + x + 1;
                let down_i = (y + 1) * W + x;
                let outer = !inside[right_i] || !inside[down_i] || !inside[y * W + x - 1] || !inside[(y - 1) * W + x];
                let seam = inside[right_i] && owner[right_i] != here || inside[down_i] && owner[down_i] != here;
                if outer || seam {
                    let noise = h01((x as u32).wrapping_mul(37) ^ (y as u32).wrapping_mul(149));
                    blend(&mut buf[idx], [24, 14, 17], if outer { 0.46 } else { 0.31 });
                    if noise > 0.45 { blend(&mut buf[idx], [204, 146, 58], if outer { 0.42 } else { 0.28 }); }
                    if noise > 0.82 { dot(&mut buf, x as f64, y as f64, if outer { 1.6 } else { 1.2 }, [228, 174, 76], 0.46); }
                }
            }
        }

        field_texture(&mut buf);
        for (i, p) in playable.iter().enumerate() { draw_rhythm_code(&mut buf, p, pts[i], i); }
        for (idx, jline) in phrases.iter().enumerate() {
            let Some(j) = &jline.jump else { continue; };
            let target = playable.iter().position(|p| p.id == j.target_id);
            let source = phrases[..idx].iter().rev()
                .find(|p| p.jump.is_none() && p.control.is_none())
                .and_then(|p| playable.iter().position(|q| q.id == p.id));
            if let (Some(s), Some(t)) = (source, target) { if s != t { draw_jump_knot(&mut buf, pts[s], pts[t], j.times); } }
        }
    }

    for i in 0..5200u32 {
        let x = 18.0 + h01(i * 17) * (W as f64 - 36.0);
        let y = 18.0 + h01(i * 43) * (H as f64 - 36.0);
        let col = [[214,154,55],[86,190,178],[180,64,124],[200,78,44],[116,168,78],[196,184,128]][i as usize % 6];
        if i % 3 == 0 { line(&mut buf, Pt{x:x-1.4,y}, Pt{x:x+1.4,y}, col, 0.10, 0.32); }
        else { dot(&mut buf, x, y, 0.65, col, 0.10); }
    }

    border(&mut buf);
    let mut f = std::fs::File::create(path)?;
    write!(f, "P6\n{} {}\n255\n", W, H)?;
    for px in buf { f.write_all(&px)?; }
    f.flush()?;
    Ok(())
}

pub fn replace_video_with_generated_source_for_phrases(path: &str, phrases: &[Phrase]) -> anyhow::Result<bool> {
    let mut src = std::env::temp_dir();
    src.push("maqam-hilbert-ratio-hatched-rug-source.ppm");
    let src = src.to_string_lossy().replace('\\', "/");
    write_rug_carpet_ppm(&src, phrases)?;
    let tmp = format!("{path}.source-background.mp4");
    let status = Command::new("ffmpeg")
        .args(["-y", "-loop", "1", "-framerate", "30", "-i", &src, "-i", path])
        .args(["-filter_complex", "[0:v][1:v]blend=all_mode=screen[v]"])
        .args(["-map", "[v]", "-map", "1:a?"])
        .args(["-c:v", "libx264", "-crf", "18", "-pix_fmt", "yuv420p"])
        .args(["-c:a", "copy", "-shortest", &tmp])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match status { Ok(s) if s.success() => { std::fs::rename(&tmp, path)?; Ok(true) }, _ => Ok(false) }
}

#[allow(dead_code)]
pub fn replace_video_with_generated_source(path: &str) -> anyhow::Result<bool> {
    replace_video_with_generated_source_for_phrases(path, &[])
}
