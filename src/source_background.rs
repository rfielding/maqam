use std::collections::HashMap;
use std::io::Write;
use std::process::{Command, Stdio};

use crate::sequencer::Phrase;

const W: usize = 1280;
const H: usize = 720;

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
    if s.contains("hijaz") { [92, 24, 76] }
    else if s.contains("bayati") { [22, 96, 52] }
    else if s.contains("saba") { [18, 82, 106] }
    else if s.contains("rast") { [112, 86, 28] }
    else { [[72,42,110],[24,88,96],[96,36,48],[30,92,58]][i % 4] }
}

fn anchors(n: usize) -> Vec<Pt> {
    if n == 3 {
        return vec![Pt { x: 350.0, y: 265.0 }, Pt { x: 855.0, y: 265.0 }, Pt { x: 640.0, y: 500.0 }];
    }
    if n == 4 {
        return vec![
            Pt { x: 380.0, y: 250.0 },
            Pt { x: 880.0, y: 250.0 },
            Pt { x: 400.0, y: 500.0 },
            Pt { x: 865.0, y: 500.0 },
        ];
    }
    let mut out = Vec::new();
    for i in 0..n.max(1) {
        let a = -0.7 + std::f64::consts::TAU * i as f64 / n.max(1) as f64;
        out.push(Pt { x: W as f64 * 0.50 + a.cos() * W as f64 * 0.20, y: H as f64 * 0.50 + a.sin() * H as f64 * 0.15 });
    }
    out
}

fn warp(seed: u32, t: f64) -> f64 {
    let p1 = h01(seed ^ 11) * std::f64::consts::TAU;
    let p2 = h01(seed ^ 29) * std::f64::consts::TAU;
    1.0 + 0.045 * (2.0 * t + p1).sin() + 0.055 * (3.0 * t + p2).cos()
}

fn dist(x: f64, y: f64, c: Pt, rx: f64, ry: f64, seed: u32) -> f64 {
    let dx = (x - c.x) / rx;
    let dy = (y - c.y) / ry;
    let th = dy.atan2(dx);
    ((dx.abs().powf(2.35) + dy.abs().powf(2.10)).powf(1.0 / 2.25)) / warp(seed, th)
}

fn boundary(c: Pt, rx: f64, ry: f64, seed: u32, t: f64, sc: f64) -> Pt {
    let w = warp(seed, t) * sc;
    Pt { x: c.x + t.cos() * rx * w, y: c.y + t.sin() * ry * w }
}

fn draw_rhythm_code(buf: &mut [[u8; 3]], p: &Phrase, c: Pt, rx: f64, ry: f64, seed: u32) {
    let groups = if p.bar.groups.is_empty() { vec![3,3,2] } else { p.bar.groups.clone() };
    let total: usize = groups.iter().map(|&g| g as usize).sum::<usize>().max(1);
    let x0 = c.x - rx * 0.58;
    let w = rx * 1.16;
    let y0 = c.y + ry * 0.38;
    let mut acc = 0usize;

    // This is the readable score layer: group lengths become woven bead columns.
    for (gi, &g) in groups.iter().enumerate() {
        let center = acc as f64 + g as f64 * 0.5;
        let x = x0 + center / total as f64 * w;
        let height = 14.0 + g as f64 * 7.0;
        let y1 = y0 - height * 0.5;
        let y2 = y0 + height * 0.5;
        let col = if gi % 2 == 0 { [226, 184, 92] } else { [96, 210, 190] };
        line(buf, Pt { x, y: y1 }, Pt { x, y: y2 }, [42, 28, 24], 0.34, 1.7);
        line(buf, Pt { x, y: y1 }, Pt { x, y: y2 }, col, 0.34, 0.8);
        for k in 0..g.max(1) {
            let yy = y1 + (k as f64 + 0.5) / g.max(1) as f64 * (y2 - y1);
            dot(buf, x, yy, 2.1, col, 0.55);
        }
        acc += g as usize;
    }

    // A small asymmetric constellation makes phrase complexity visible without labels.
    let density = (p.bar.ratio_strs.len() + p.bar.frequencies.len()).clamp(5, 18);
    let base = Pt { x: c.x + rx * 0.38, y: c.y - ry * 0.34 };
    for k in 0..density {
        let a = std::f64::consts::TAU * h01(seed.wrapping_add(k as u32 * 101));
        let r = 10.0 + 34.0 * h01(seed ^ (k as u32 * 349));
        let x = base.x + a.cos() * r;
        let y = base.y + a.sin() * r * 0.55;
        if dist(x, y, c, rx, ry, seed) < 0.92 {
            let col = if k % 3 == 0 { [220, 160, 74] } else if k % 3 == 1 { [82, 190, 176] } else { [182, 76, 142] };
            dot(buf, x, y, 1.6, col, 0.42);
        }
    }
}

fn draw_region(buf: &mut [[u8; 3]], p: &Phrase, c: Pt, rx: f64, ry: f64, rgb: [u8; 3], ord: usize) {
    let seed = (p.id as u32).wrapping_mul(977).wrapping_add(ord as u32 * 313).wrapping_add(1701);

    for y in (c.y - ry * 1.10) as i32..=(c.y + ry * 1.10) as i32 {
        for x in (c.x - rx * 1.10) as i32..=(c.x + rx * 1.10) as i32 {
            if x < 0 || y < 0 || x >= W as i32 || y >= H as i32 { continue; }
            let d = dist(x as f64 + 0.5, y as f64 + 0.5, c, rx, ry, seed);
            if d < 1.0 {
                let over = ((x as f64 * 0.085 + seed as f64 * 0.011).sin() * 0.5 + 0.5).powf(2.0);
                let under = ((y as f64 * 0.095 + seed as f64 * 0.017).cos() * 0.5 + 0.5).powf(2.0);
                let diagonal = ((x as f64 * 0.045 - y as f64 * 0.032 + seed as f64 * 0.013).sin() * 0.5 + 0.5).powf(3.0);
                let thread = 0.24 * over + 0.20 * under + 0.12 * diagonal;
                pix(buf, x, y, rgb, (1.0 - d).powf(0.20) * (0.26 + thread));
                if (x + (seed as i32 & 7)) % 9 == 0 { pix(buf, x, y, [210,185,120], 0.026); }
                if (y + (seed as i32 & 11)) % 11 == 0 { pix(buf, x, y, [32,20,28], 0.040); }
            }
        }
    }

    for (sc, alpha, step) in [(1.00, 0.22, 16), (0.88, 0.12, 24), (0.74, 0.08, 32)] {
        let mut last = boundary(c, rx, ry, seed, 0.0, sc);
        let mut acc = 0usize;
        for i in 1..=220 {
            let t = std::f64::consts::TAU * i as f64 / 220.0;
            let q = boundary(c, rx, ry, seed, t, sc);
            line(buf, last, q, [185, 128, 54], alpha * 0.10, 0.55);
            acc += 1;
            if acc >= step { acc = 0; dot(buf, q.x, q.y, 0.9, [215, 158, 68], alpha); }
            last = q;
        }
    }

    let groups = if p.bar.groups.is_empty() { vec![3,3,2] } else { p.bar.groups.clone() };
    for (gi, g) in groups.iter().enumerate() {
        let yy = c.y - ry * 0.48 + (gi as f64 + 0.5) * ry * 0.96 / groups.len().max(1) as f64;
        let mut last: Option<Pt> = None;
        for k in 0..160 {
            let u = k as f64 / 159.0;
            let x = c.x - rx * 0.76 + u * rx * 1.52;
            let y = yy + (u * std::f64::consts::TAU * (1.0 + *g as f64 * 0.22) + seed as f64 * 0.01).sin() * (5.0 + *g as f64 * 0.9);
            if dist(x, y, c, rx, ry, seed) < 0.94 {
                let q = Pt { x, y };
                if let Some(prev) = last { line(buf, prev, q, [210,195,130], 0.09, 0.45); }
                last = Some(q);
            } else { last = None; }
        }
    }

    draw_rhythm_code(buf, p, c, rx, ry, seed);
}

fn quiet_seam(buf: &mut [[u8; 3]], a: Pt, b: Pt) {
    let mid = Pt { x: (a.x + b.x) * 0.5, y: (a.y + b.y) * 0.5 };
    for i in 0..36 {
        let t = i as f64 / 35.0;
        let q = Pt { x: a.x * (1.0 - t) + b.x * t, y: a.y * (1.0 - t) + b.y * t };
        if i % 7 == 0 { dot(buf, (q.x + mid.x) * 0.5, (q.y + mid.y) * 0.5, 0.8, [196,138,56], 0.08); }
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
            let light = 0.055 * vertical + 0.050 * horizontal + 0.028 * diagonal;
            let dark = if knot > 0.985 { 0.085 } else { 0.0 };
            if light > 0.01 { blend(&mut buf[idx], [150,130,88], light); }
            if dark > 0.0 { blend(&mut buf[idx], [7,6,10], dark); }
        }
    }
}

fn write_rug_carpet_ppm(path: &str, phrases: &[Phrase]) -> anyhow::Result<()> {
    let mut buf = vec![[0u8; 3]; W * H];
    for y in 0..H {
        for x in 0..W {
            let xf = x as f64; let yf = y as f64;
            let base = 7.0 + 3.0 * (xf * 0.015).sin() + 3.0 * (yf * 0.021).cos() + 2.0 * ((xf + yf) * 0.010).sin();
            let n = (hash((x as u32).wrapping_mul(31) ^ (y as u32).wrapping_mul(131)) % 18) as f64;
            buf[y * W + x] = [clamp(base + n * 0.18), clamp(base + 2.0 + n * 0.14), clamp(base + 12.0 + n * 0.42)];
        }
    }
    field_texture(&mut buf);

    let playable: Vec<&Phrase> = phrases.iter().filter(|p| p.jump.is_none() && p.control.is_none()).collect();
    let n = playable.len().max(1);
    let pts = anchors(n);
    let mut by_id = HashMap::new();
    for (i, p) in playable.iter().enumerate() { by_id.insert(p.id, pts[i]); }

    for (i, p) in playable.iter().enumerate() {
        let (rx, ry) = if n == 3 {
            match i { 0 => (285.0,195.0), 1 => (292.0,190.0), _ => (340.0,155.0) }
        } else if n == 4 {
            match i { 0 => (232.0,140.0), 1 => (235.0,140.0), 2 => (245.0,135.0), _ => (242.0,135.0) }
        } else {
            ((250.0 - n as f64 * 3.0).clamp(190.0,238.0), (160.0 - n as f64 * 2.0).clamp(120.0,152.0))
        };
        draw_region(&mut buf, p, pts[i], rx, ry, color(p, i), i);
    }
    field_texture(&mut buf);

    for (idx, jline) in phrases.iter().enumerate() {
        let Some(j) = &jline.jump else { continue; };
        let Some(&target) = by_id.get(&j.target_id) else { continue; };
        let source = phrases[..idx].iter().rev().find(|p| p.jump.is_none() && p.control.is_none()).and_then(|p| by_id.get(&p.id)).copied().unwrap_or(target);
        if source.sub(target).len() > 4.0 { quiet_seam(&mut buf, source, target); }
    }

    for i in 0..5600u32 {
        let x = 18.0 + h01(i * 17) * (W as f64 - 36.0);
        let y = 18.0 + h01(i * 43) * (H as f64 - 36.0);
        let col = [[214,154,55],[86,190,178],[180,64,124],[200,78,44],[116,168,78],[196,184,128]][i as usize % 6];
        if i % 3 == 0 { line(&mut buf, Pt{x:x-1.4,y}, Pt{x:x+1.4,y}, col, 0.11, 0.34); }
        else { dot(&mut buf, x, y, 0.7, col, 0.11); }
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
    src.push("maqam-spring-rug-source.ppm");
    let src = src.to_string_lossy().replace('\\', "/");
    write_rug_carpet_ppm(&src, phrases)?;
    let tmp = format!("{path}.source-background.mp4");
    let status = Command::new("ffmpeg")
        .args(["-y", "-loop", "1", "-framerate", "30", "-i", &src, "-i", path])
        .args(["-map", "0:v", "-map", "1:a"])
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
