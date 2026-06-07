use std::collections::HashMap;
use std::io::Write;
use std::process::{Command, Stdio};

use crate::sequencer::Phrase;

const W: usize = 1280;
const H: usize = 720;

#[derive(Clone, Copy)]
struct Pt { x: f64, y: f64 }

fn clamp_u8(x: f64) -> u8 { x.round().clamp(0.0, 255.0) as u8 }

fn mix(a: [u8; 3], b: [u8; 3], t: f64) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        clamp_u8(a[0] as f64 * (1.0 - t) + b[0] as f64 * t),
        clamp_u8(a[1] as f64 * (1.0 - t) + b[1] as f64 * t),
        clamp_u8(a[2] as f64 * (1.0 - t) + b[2] as f64 * t),
    ]
}

fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^ (x >> 16)
}
fn h01(x: u32) -> f64 { hash(x) as f64 / u32::MAX as f64 }

fn set_px(buf: &mut [[u8; 3]], x: i32, y: i32, rgb: [u8; 3], a: f64) {
    if x < 0 || y < 0 || x >= W as i32 || y >= H as i32 { return; }
    let idx = y as usize * W + x as usize;
    buf[idx] = mix(buf[idx], rgb, a);
}

fn dot(buf: &mut [[u8; 3]], cx: f64, cy: f64, r: f64, rgb: [u8; 3], a: f64) {
    let ri = r.ceil() as i32;
    for yy in (cy as i32 - ri)..=(cy as i32 + ri) {
        for xx in (cx as i32 - ri)..=(cx as i32 + ri) {
            let dx = xx as f64 + 0.5 - cx;
            let dy = yy as f64 + 0.5 - cy;
            let d = (dx * dx + dy * dy).sqrt();
            if d <= r { set_px(buf, xx, yy, rgb, a * (1.0 - d / r)); }
        }
    }
}

fn line(buf: &mut [[u8; 3]], a: Pt, b: Pt, rgb: [u8; 3], alpha: f64, width: f64) {
    let steps = ((b.x - a.x).hypot(b.y - a.y) as usize).max(1);
    for i in 0..=steps {
        let t = i as f64 / steps as f64;
        dot(buf, a.x * (1.0 - t) + b.x * t, a.y * (1.0 - t) + b.y * t, width, rgb, alpha);
    }
}

fn bezier(buf: &mut [[u8; 3]], a: Pt, c: Pt, b: Pt, rgb: [u8; 3], alpha: f64, width: f64, beads: bool) {
    let mut prev = a;
    for i in 1..=140 {
        let t = i as f64 / 140.0;
        let u = 1.0 - t;
        let p = Pt {
            x: u*u*a.x + 2.0*u*t*c.x + t*t*b.x,
            y: u*u*a.y + 2.0*u*t*c.y + t*t*b.y,
        };
        line(buf, prev, p, rgb, alpha, width);
        if beads && i % 6 == 0 { dot(buf, p.x, p.y, width + 1.6, [226, 172, 72], 0.62); }
        prev = p;
    }
}

fn hilbert_xy(mut d: u32, order: u32) -> (u32, u32) {
    let n = 1u32 << order;
    let mut x = 0u32;
    let mut y = 0u32;
    let mut s = 1u32;
    while s < n {
        let rx = (d / 2) & 1;
        let ry = (d ^ rx) & 1;
        if ry == 0 {
            if rx == 1 { x = s - 1 - x; y = s - 1 - y; }
            std::mem::swap(&mut x, &mut y);
        }
        x += s * rx;
        y += s * ry;
        d /= 4;
        s *= 2;
    }
    (x, y)
}

fn hilbert_anchor(i: usize, count: usize) -> Pt {
    let count = count.max(1);
    let mut order = 1u32;
    while (1usize << (2 * order)) < count { order += 1; }
    let n = (1u32 << order) as f64;
    let max_d = (1usize << (2 * order)) - 1;
    let d = if count <= 1 { 0 } else { i * max_d / (count - 1) } as u32;
    let (x, y) = hilbert_xy(d, order);
    Pt {
        x: 120.0 + (x as f64 + 0.5) / n * (W as f64 - 240.0),
        y:  95.0 + (y as f64 + 0.5) / n * (H as f64 - 190.0),
    }
}

fn color_for_phrase(p: &Phrase, ordinal: usize) -> [u8; 3] {
    let s = p.src.to_lowercase();
    if s.contains("hijaz") { [104, 28, 82] }
    else if s.contains("bayati") { [24, 102, 58] }
    else if s.contains("saba") { [18, 86, 108] }
    else if s.contains("rast") { [122, 92, 28] }
    else {
        match ordinal % 5 {
            0 => [82, 48, 118],
            1 => [24, 96, 100],
            2 => [108, 40, 48],
            3 => [34, 104, 62],
            _ => [92, 70, 26],
        }
    }
}

fn region(buf: &mut [[u8; 3]], p: &Phrase, c: Pt, rx: f64, ry: f64, color: [u8; 3], ordinal: usize) {
    let seed = (p.id as u32).wrapping_mul(977).wrapping_add(ordinal as u32 * 313);
    let y0 = (c.y - ry * 1.22) as i32;
    let y1 = (c.y + ry * 1.22) as i32;
    let x0 = (c.x - rx * 1.22) as i32;
    let x1 = (c.x + rx * 1.22) as i32;
    for y in y0..=y1 {
        for x in x0..=x1 {
            if x < 0 || y < 0 || x >= W as i32 || y >= H as i32 { continue; }
            let dx = (x as f64 + 0.5 - c.x) / rx;
            let dy = (y as f64 + 0.5 - c.y) / ry;
            let th = dy.atan2(dx);
            let warp = 1.0
                + 0.12 * (th * 5.0 + seed as f64 * 0.011).sin()
                + 0.07 * (th * 9.0 + seed as f64 * 0.019).cos()
                + 0.04 * (th * 17.0 + seed as f64 * 0.007).sin();
            let d = (dx * dx + dy * dy).sqrt() / warp;
            if d < 1.0 {
                let stripe = 0.5 + 0.5 * ((x as f64 * 0.055 + y as f64 * 0.021 + seed as f64).sin());
                let alpha = (1.0 - d).powf(0.26) * (0.32 + 0.18 * stripe);
                set_px(buf, x, y, color, alpha);
            }
        }
    }

    let mut prev = None;
    for i in 0..=260 {
        let th = std::f64::consts::TAU * i as f64 / 260.0;
        let warp = 1.0
            + 0.12 * (th * 5.0 + seed as f64 * 0.011).sin()
            + 0.07 * (th * 9.0 + seed as f64 * 0.019).cos();
        let q = Pt { x: c.x + th.cos() * rx * warp, y: c.y + th.sin() * ry * warp };
        if let Some(last) = prev { line(buf, last, q, [190, 135, 58], 0.29, 1.4); }
        if i % 5 == 0 { dot(buf, q.x, q.y, 2.2, [232, 178, 78], 0.55); }
        prev = Some(q);
    }

    // Rhythm bands inside territory: phrase.bar.groups drives cadence.
    let groups = if p.bar.groups.is_empty() { vec![3, 3, 2] } else { p.bar.groups.clone() };
    for (gi, g) in groups.iter().enumerate() {
        let yy = c.y - ry * 0.50 + (gi as f64 + 0.5) * ry / groups.len().max(1) as f64;
        let mut last = None;
        for k in 0..150 {
            let u = k as f64 / 149.0;
            let x = c.x - rx * 0.72 + u * rx * 1.44;
            let y = yy + (u * std::f64::consts::TAU * (1.0 + *g as f64 * 0.20) + seed as f64 * 0.01).sin() * (5.0 + *g as f64 * 1.3);
            let q = Pt { x, y };
            if let Some(last) = last { line(buf, last, q, [210, 195, 130], 0.16, 0.8); }
            if k % ((*g as usize).max(2) + 5) == 0 { dot(buf, x, y, 1.6, [230, 214, 145], 0.34); }
            last = Some(q);
        }
    }

    // Harmonic constellations: ratio count and frequency count affect density.
    let density = (p.bar.ratio_strs.len() + p.bar.frequencies.len()).clamp(8, 42);
    for k in 0..density {
        let kk = k as u32;
        let rr = h01(seed ^ kk.wrapping_mul(7919)).sqrt();
        let th = std::f64::consts::TAU * h01(seed.wrapping_add(kk * 313));
        let x = c.x + th.cos() * rx * 0.70 * rr;
        let y = c.y + th.sin() * ry * 0.66 * rr;
        let col = match k % 5 {
            0 => [220, 160, 70],
            1 => [96, 204, 190],
            2 => [190, 80, 170],
            3 => [210, 80, 70],
            _ => [130, 190, 80],
        };
        dot(buf, x, y, 2.4 + 1.8 * h01(seed ^ kk), col, 0.48);
        line(buf, Pt { x: x - 4.0, y }, Pt { x: x + 4.0, y }, col, 0.22, 0.6);
        line(buf, Pt { x, y: y - 4.0 }, Pt { x, y: y + 4.0 }, col, 0.22, 0.6);
    }
}

fn write_hilbert_carpet_ppm(path: &str, phrases: &[Phrase]) -> anyhow::Result<()> {
    let mut buf = vec![[0_u8; 3]; W * H];
    for y in 0..H {
        for x in 0..W {
            let xf = x as f64;
            let yf = y as f64;
            let weave = 8.0 + 4.0 * (xf * 0.015).sin() + 3.0 * (yf * 0.021).cos() + 2.0 * ((xf + yf) * 0.010).sin();
            let n = (hash((x as u32).wrapping_mul(31) ^ (y as u32).wrapping_mul(131)) % 14) as f64;
            buf[y * W + x] = [clamp_u8(weave + n * 0.20), clamp_u8(weave + 2.0 + n * 0.16), clamp_u8(weave + 12.0 + n * 0.44)];
        }
    }

    let playable: Vec<&Phrase> = phrases.iter().filter(|p| p.jump.is_none() && p.control.is_none()).collect();
    let count = playable.len().max(1);
    let mut anchors: HashMap<usize, Pt> = HashMap::new();

    for (i, p) in playable.iter().enumerate() {
        let c = hilbert_anchor(i, count);
        anchors.insert(p.id, c);
    }

    // If all lines are controls/jumps, make a small fallback medallion.
    if playable.is_empty() {
        anchors.insert(0, Pt { x: W as f64 * 0.5, y: H as f64 * 0.5 });
    }

    // Hilbert ghost path: visible as woven geography, not a graph line.
    let mut last = None;
    for i in 0..count {
        let p = hilbert_anchor(i, count);
        if let Some(prev) = last {
            line(&mut buf, prev, p, [72, 52, 96], 0.22, 2.0);
            let mid = Pt { x: (prev.x + p.x) * 0.5, y: (prev.y + p.y) * 0.5 };
            dot(&mut buf, mid.x, mid.y, 2.2, [170, 125, 56], 0.38);
        }
        last = Some(p);
    }

    for (i, p) in playable.iter().enumerate() {
        let c = anchors[&p.id];
        let color = color_for_phrase(p, i);
        let rx = (170.0 - (count as f64 * 7.0).min(55.0)).max(92.0);
        let ry = (105.0 - (count as f64 * 3.0).min(25.0)).max(72.0);
        region(&mut buf, p, c, rx, ry, color, i);
    }

    // Jump entries become curved beaded seams from the jump location to the target territory.
    for (ji, jline) in phrases.iter().filter(|p| p.jump.is_some()).enumerate() {
        let Some(j) = &jline.jump else { continue; };
        let target = anchors.get(&j.target_id).copied().unwrap_or(Pt { x: 90.0, y: H as f64 - 90.0 });
        let source = if let Some(pos) = phrases.iter().position(|p| p.id == jline.id) {
            let prev_play = phrases[..pos].iter().rev().find(|p| p.jump.is_none() && p.control.is_none()).and_then(|p| anchors.get(&p.id)).copied();
            prev_play.unwrap_or(target)
        } else { target };
        let dx = target.x - source.x;
        let dy = target.y - source.y;
        let len = dx.hypot(dy).max(1.0);
        let bend = 52.0 + (j.times as f64 * 13.0).min(120.0);
        let ctrl = Pt {
            x: (source.x + target.x) * 0.5 - dy / len * bend,
            y: (source.y + target.y) * 0.5 + dx / len * bend,
        };
        bezier(&mut buf, source, ctrl, target, [205, 145, 56], 0.30, 2.0 + (j.times as f64).min(5.0) * 0.25, true);
        dot(&mut buf, ctrl.x, ctrl.y, 9.0 + j.times as f64 * 1.5, [212, 160, 70], 0.25);
        for k in 0..(j.times.max(1) * 5).min(45) {
            let a = std::f64::consts::TAU * k as f64 / ((j.times.max(1) * 5).min(45) as f64);
            dot(&mut buf, ctrl.x + a.cos() * 15.0, ctrl.y + a.sin() * 10.0, 1.7, [230, 188, 92], 0.48);
        }
        let _ = ji;
    }

    // Global stitch dust.
    for i in 0..1900_u32 {
        let x = 18.0 + h01(i * 17) * (W as f64 - 36.0);
        let y = 18.0 + h01(i * 43) * (H as f64 - 36.0);
        let col = match i % 5 {
            0 => [214, 154, 55],
            1 => [96, 204, 190],
            2 => [210, 73, 132],
            3 => [226, 98, 40],
            _ => [204, 190, 132],
        };
        dot(&mut buf, x, y, 1.0, col, 0.18);
    }

    let mut f = std::fs::File::create(path)?;
    write!(f, "P6\n{} {}\n255\n", W, H)?;
    for px in buf { f.write_all(&px)?; }
    f.flush()?;
    Ok(())
}

pub fn replace_video_with_generated_source_for_phrases(path: &str, phrases: &[Phrase]) -> anyhow::Result<bool> {
    let mut src = std::env::temp_dir();
    src.push("maqam-hilbert-carpet-source.ppm");
    let src = src.to_string_lossy().replace('\\', "/");
    write_hilbert_carpet_ppm(&src, phrases)?;

    let tmp = format!("{path}.source-background.mp4");
    let filter = "[1:v]format=rgba,lumakey=threshold=0.13:tolerance=0.10:softness=0.04[fg];[0:v][fg]overlay=format=auto[v]";
    let status = Command::new("ffmpeg")
        .args(["-y", "-loop", "1", "-framerate", "30", "-i", &src, "-i", path])
        .args(["-filter_complex", filter, "-map", "[v]", "-map", "1:a"])
        .args(["-c:v", "libx264", "-crf", "18", "-pix_fmt", "yuv420p"])
        .args(["-c:a", "copy", "-shortest", &tmp])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    match status {
        Ok(s) if s.success() => { std::fs::rename(&tmp, path)?; Ok(true) }
        _ => Ok(false),
    }
}

#[allow(dead_code)]
pub fn replace_video_with_generated_source(path: &str) -> anyhow::Result<bool> {
    replace_video_with_generated_source_for_phrases(path, &[])
}
