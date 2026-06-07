use std::collections::HashMap;
use std::io::Write;
use std::process::{Command, Stdio};

use crate::sequencer::Phrase;

const W: usize = 1280;
const H: usize = 720;

#[derive(Clone, Copy, Debug)]
struct Pt { x: f64, y: f64 }

impl Pt {
    fn add(self, o: Pt) -> Pt { Pt { x: self.x + o.x, y: self.y + o.y } }
    fn sub(self, o: Pt) -> Pt { Pt { x: self.x - o.x, y: self.y - o.y } }
    fn mul(self, k: f64) -> Pt { Pt { x: self.x * k, y: self.y * k } }
    fn len(self) -> f64 { self.x.hypot(self.y) }
}

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
            if d <= r { set_px(buf, xx, yy, rgb, a * (1.0 - d / r).powf(0.65)); }
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

fn bezier_points(a: Pt, c: Pt, b: Pt, steps: usize) -> Vec<Pt> {
    let mut pts = Vec::with_capacity(steps + 1);
    for i in 0..=steps {
        let t = i as f64 / steps as f64;
        let u = 1.0 - t;
        pts.push(Pt {
            x: u*u*a.x + 2.0*u*t*c.x + t*t*b.x,
            y: u*u*a.y + 2.0*u*t*c.y + t*t*b.y,
        });
    }
    pts
}

fn polyline(buf: &mut [[u8; 3]], pts: &[Pt], rgb: [u8; 3], alpha: f64, width: f64) {
    for pair in pts.windows(2) { line(buf, pair[0], pair[1], rgb, alpha, width); }
}

fn dots_along(buf: &mut [[u8; 3]], pts: &[Pt], rgb: [u8; 3], step: f64, radius: f64, alpha: f64) {
    let mut accum = 0.0;
    for pair in pts.windows(2) {
        let a = pair[0];
        let b = pair[1];
        let dist = b.sub(a).len().max(1.0);
        let pieces = (dist / 3.0).ceil() as usize;
        for j in 0..pieces {
            let u = j as f64 / pieces as f64;
            accum += dist / pieces as f64;
            if accum >= step {
                accum = 0.0;
                dot(buf, a.x * (1.0 - u) + b.x * u, a.y * (1.0 - u) + b.y * u, radius, rgb, alpha);
            }
        }
    }
}

fn color_for_phrase(p: &Phrase, ordinal: usize) -> [u8; 3] {
    let s = p.src.to_lowercase();
    if s.contains("hijaz") { [92, 22, 76] }
    else if s.contains("bayati") { [22, 94, 52] }
    else if s.contains("saba") { [17, 80, 104] }
    else if s.contains("rast") { [112, 84, 26] }
    else {
        match ordinal % 6 {
            0 => [74, 42, 112],
            1 => [22, 90, 96],
            2 => [100, 36, 46],
            3 => [30, 94, 58],
            4 => [88, 64, 24],
            _ => [90, 36, 84],
        }
    }
}

fn initial_anchor(i: usize, n: usize) -> Pt {
    if n == 3 {
        return match i {
            0 => Pt { x: 335.0, y: 250.0 },
            1 => Pt { x: 865.0, y: 255.0 },
            _ => Pt { x: 640.0, y: 515.0 },
        };
    }
    let a = -0.75 + std::f64::consts::TAU * i as f64 / n.max(1) as f64;
    Pt { x: W as f64 * 0.50 + a.cos() * W as f64 * 0.25, y: H as f64 * 0.48 + a.sin() * H as f64 * 0.24 }
}

fn layout_spring(phrases: &[Phrase], playable: &[&Phrase]) -> Vec<Pt> {
    let n = playable.len();
    let mut pos: Vec<Pt> = (0..n).map(|i| initial_anchor(i, n)).collect();
    if n <= 1 { return pos; }
    let index: HashMap<usize, usize> = playable.iter().enumerate().map(|(i, p)| (p.id, i)).collect();
    let mut vel = vec![Pt { x: 0.0, y: 0.0 }; n];
    let radius = if n == 3 { 255.0 } else { (185.0 - n as f64 * 5.0).clamp(110.0, 185.0) };

    for _ in 0..90 {
        let mut force = vec![Pt { x: 0.0, y: 0.0 }; n];
        for i in 0..n {
            for j in (i + 1)..n {
                let d = pos[i].sub(pos[j]);
                let len = d.len().max(1.0);
                let want = radius * 1.45;
                let push = ((want - len) / want).max(0.0) * 2.8 + 1800.0 / (len * len);
                let f = d.mul(push / len);
                force[i] = force[i].add(f);
                force[j] = force[j].sub(f);
            }
        }
        // Gentle attraction along playback order: topology informs pressure, but no edge will be drawn.
        for i in 0..(n - 1) {
            let d = pos[i + 1].sub(pos[i]);
            let len = d.len().max(1.0);
            let want = radius * 1.70;
            let pull = (len - want) * 0.010;
            let f = d.mul(pull / len);
            force[i] = force[i].add(f);
            force[i + 1] = force[i + 1].sub(f);
        }
        // Jump targets attract the previous playable region toward the target; this packs related sections.
        for (idx, jline) in phrases.iter().enumerate().filter(|(_, p)| p.jump.is_some()) {
            let Some(j) = &jline.jump else { continue; };
            let Some(&to) = index.get(&j.target_id) else { continue; };
            let from = phrases[..idx].iter().rev().find(|p| p.jump.is_none() && p.control.is_none()).and_then(|p| index.get(&p.id)).copied();
            let Some(from) = from else { continue; };
            if from == to { continue; }
            let d = pos[to].sub(pos[from]);
            let len = d.len().max(1.0);
            let want = radius * 1.42;
            let pull = (len - want) * 0.020 * (j.times as f64).sqrt().min(2.4);
            let f = d.mul(pull / len);
            force[from] = force[from].add(f);
            force[to] = force[to].sub(f);
        }
        // Border squishes the rug inward.  This is the frame pressure.
        for i in 0..n {
            let margin_x = radius * 0.72 + 48.0;
            let margin_y = radius * 0.52 + 42.0;
            if pos[i].x < margin_x { force[i].x += (margin_x - pos[i].x) * 0.060; }
            if pos[i].x > W as f64 - margin_x { force[i].x -= (pos[i].x - (W as f64 - margin_x)) * 0.060; }
            if pos[i].y < margin_y { force[i].y += (margin_y - pos[i].y) * 0.070; }
            if pos[i].y > H as f64 - margin_y { force[i].y -= (pos[i].y - (H as f64 - margin_y)) * 0.070; }
        }
        for i in 0..n {
            vel[i] = vel[i].mul(0.72).add(force[i]);
            pos[i] = pos[i].add(vel[i]);
        }
    }
    pos
}

fn blob_point(c: Pt, rx: f64, ry: f64, seed: u32, t: f64, scale: f64) -> Pt {
    let warp = 1.0
        + 0.11 * (t * 5.0 + seed as f64 * 0.011).sin()
        + 0.07 * (t * 9.0 + seed as f64 * 0.019).cos()
        + 0.04 * (t * 17.0 + seed as f64 * 0.007).sin();
    Pt { x: c.x + t.cos() * rx * warp * scale, y: c.y + t.sin() * ry * warp * scale }
}

fn draw_region(buf: &mut [[u8; 3]], p: &Phrase, c: Pt, rx: f64, ry: f64, color: [u8; 3], ordinal: usize) {
    let seed = (p.id as u32).wrapping_mul(977).wrapping_add(ordinal as u32 * 313).wrapping_add(1701);
    let y0 = (c.y - ry * 1.25) as i32;
    let y1 = (c.y + ry * 1.25) as i32;
    let x0 = (c.x - rx * 1.25) as i32;
    let x1 = (c.x + rx * 1.25) as i32;
    for y in y0..=y1 {
        for x in x0..=x1 {
            if x < 0 || y < 0 || x >= W as i32 || y >= H as i32 { continue; }
            let dx = (x as f64 + 0.5 - c.x) / rx;
            let dy = (y as f64 + 0.5 - c.y) / ry;
            let th = dy.atan2(dx);
            let warp = 1.0
                + 0.11 * (th * 5.0 + seed as f64 * 0.011).sin()
                + 0.07 * (th * 9.0 + seed as f64 * 0.019).cos()
                + 0.04 * (th * 17.0 + seed as f64 * 0.007).sin();
            let d = (dx * dx + dy * dy).sqrt() / warp;
            if d < 1.0 {
                let weave = 0.5 + 0.5 * ((x as f64 * 0.060 + y as f64 * 0.020 + seed as f64).sin());
                let cross = 0.5 + 0.5 * ((x as f64 * 0.019 - y as f64 * 0.050 + seed as f64 * 0.3).cos());
                let alpha = (1.0 - d).powf(0.22) * (0.38 + 0.18 * weave + 0.08 * cross);
                set_px(buf, x, y, color, alpha);
            }
        }
    }

    // Nested dotted contours: embroidered boundaries, not petals.
    for (scale, a, bead) in [(1.00, 0.50, 2.0), (0.93, 0.32, 1.6), (0.82, 0.22, 1.25), (0.68, 0.16, 1.0)] {
        let mut pts = Vec::with_capacity(241);
        for i in 0..=240 {
            let t = std::f64::consts::TAU * i as f64 / 240.0;
            pts.push(blob_point(c, rx, ry, seed, t, scale));
        }
        polyline(buf, &pts, [190, 135, 58], a * 0.34, 1.0);
        dots_along(buf, &pts, [224, 170, 76], if scale > 0.9 { 8.5 } else { 13.0 }, bead, a);
    }

    // Rhythmic woven bands: grouped cadence, quiet enough to read as textile fill.
    let groups = if p.bar.groups.is_empty() { vec![3, 3, 2] } else { p.bar.groups.clone() };
    for (gi, g) in groups.iter().enumerate() {
        let yy = c.y - ry * 0.48 + (gi as f64 + 0.5) * ry * 0.96 / groups.len().max(1) as f64;
        let mut pts = Vec::with_capacity(160);
        for k in 0..160 {
            let u = k as f64 / 159.0;
            let x = c.x - rx * 0.76 + u * rx * 1.52;
            let y = yy + (u * std::f64::consts::TAU * (1.0 + *g as f64 * 0.22) + seed as f64 * 0.01).sin() * (6.0 + *g as f64 * 1.15);
            pts.push(Pt { x, y });
        }
        polyline(buf, &pts, [210, 195, 130], 0.12, 0.65);
        dots_along(buf, &pts, [230, 215, 145], (15.0 - *g as f64).max(7.0), 1.15, 0.28);
    }

    // Lozenge stitch fragments, not rosettes.
    for k in 0..70u32 {
        let rr = h01(seed ^ k.wrapping_mul(7919)).sqrt();
        let th = std::f64::consts::TAU * h01(seed.wrapping_add(k * 313));
        let x = c.x + th.cos() * rx * 0.78 * rr;
        let y = c.y + th.sin() * ry * 0.70 * rr;
        let s = 3.5 + 5.0 * h01(seed ^ k.wrapping_mul(19));
        let col = match k % 5 {
            0 => [215, 155, 65],
            1 => [82, 190, 176],
            2 => [184, 70, 140],
            3 => [196, 76, 52],
            _ => [116, 170, 78],
        };
        line(buf, Pt { x, y: y - s }, Pt { x: x + s, y }, col, 0.20, 0.55);
        line(buf, Pt { x: x + s, y }, Pt { x, y: y + s }, col, 0.20, 0.55);
        line(buf, Pt { x, y: y + s }, Pt { x: x - s, y }, col, 0.20, 0.55);
        line(buf, Pt { x: x - s, y }, Pt { x, y: y - s }, col, 0.20, 0.55);
    }

    // Ratio constellations: small asymmetric clusters.
    let density = (p.bar.ratio_strs.len() + p.bar.frequencies.len()).clamp(6, 22);
    let base = Pt { x: c.x + rx * 0.10, y: c.y + ry * 0.45 };
    for k in 0..density {
        let kk = k as u32;
        let a = std::f64::consts::TAU * k as f64 / density as f64;
        let r = 18.0 + 42.0 * h01(seed ^ kk.wrapping_mul(331));
        let col = if k % 2 == 0 { [92, 202, 188] } else { [205, 190, 132] };
        dot(buf, base.x + a.cos() * r, base.y + a.sin() * r * 0.45, 1.9, col, 0.45);
    }
}

fn draw_seam(buf: &mut [[u8; 3]], a: Pt, b: Pt, strength: f64) {
    let d = b.sub(a);
    let len = d.len().max(1.0);
    let ctrl = Pt { x: (a.x + b.x) * 0.5 - d.y / len * 70.0, y: (a.y + b.y) * 0.5 + d.x / len * 70.0 };
    let pts = bezier_points(a, ctrl, b, 130);
    // Dark cord plus sparse beads: seam/crossing, not a graph edge.
    polyline(buf, &pts, [16, 10, 13], 0.34 * strength, 3.2);
    polyline(buf, &pts, [104, 72, 42], 0.12 * strength, 1.5);
    dots_along(buf, &pts, [202, 145, 58], 18.0, 1.7, 0.34 * strength);
}

fn draw_border(buf: &mut [[u8; 3]]) {
    let gold = [194, 135, 54];
    for inset in [8, 18, 36] {
        let a = if inset == 8 { 0.55 } else if inset == 18 { 0.32 } else { 0.18 };
        line(buf, Pt { x: inset as f64, y: inset as f64 }, Pt { x: (W - inset) as f64, y: inset as f64 }, gold, a, 1.2);
        line(buf, Pt { x: (W - inset) as f64, y: inset as f64 }, Pt { x: (W - inset) as f64, y: (H - inset) as f64 }, gold, a, 1.2);
        line(buf, Pt { x: (W - inset) as f64, y: (H - inset) as f64 }, Pt { x: inset as f64, y: (H - inset) as f64 }, gold, a, 1.2);
        line(buf, Pt { x: inset as f64, y: (H - inset) as f64 }, Pt { x: inset as f64, y: inset as f64 }, gold, a, 1.2);
    }
    // Corner lozenges: rug frame, not flowers.
    for &(cx, cy) in &[(48.0, 48.0), (W as f64 - 48.0, 48.0), (48.0, H as f64 - 48.0), (W as f64 - 48.0, H as f64 - 48.0)] {
        for r in [13.0, 24.0, 36.0] {
            let pts = [Pt { x: cx, y: cy - r }, Pt { x: cx + r, y: cy }, Pt { x: cx, y: cy + r }, Pt { x: cx - r, y: cy }, Pt { x: cx, y: cy - r }];
            polyline(buf, &pts, gold, 0.38, 0.9);
        }
    }
}

fn write_rug_carpet_ppm(path: &str, phrases: &[Phrase]) -> anyhow::Result<()> {
    let mut buf = vec![[0_u8; 3]; W * H];

    // Whole-field textile base. The rug must not have blank background.
    for y in 0..H {
        for x in 0..W {
            let xf = x as f64;
            let yf = y as f64;
            let weave = 8.0 + 4.0 * (xf * 0.015).sin() + 3.0 * (yf * 0.021).cos() + 2.0 * ((xf + yf) * 0.010).sin();
            let n = (hash((x as u32).wrapping_mul(31) ^ (y as u32).wrapping_mul(131)) % 16) as f64;
            buf[y * W + x] = [clamp_u8(weave + n * 0.20), clamp_u8(weave + 2.0 + n * 0.16), clamp_u8(weave + 12.0 + n * 0.46)];
        }
    }

    let playable: Vec<&Phrase> = phrases.iter().filter(|p| p.jump.is_none() && p.control.is_none()).collect();
    let n = playable.len().max(1);
    let anchors = if playable.is_empty() { vec![Pt { x: W as f64 * 0.5, y: H as f64 * 0.5 }] } else { layout_spring(phrases, &playable) };
    let mut anchor_by_id = HashMap::new();
    for (i, p) in playable.iter().enumerate() { anchor_by_id.insert(p.id, anchors[i]); }

    // Large pressured territories. For the common 3-phrase case, recover the triangular rug composition.
    for (i, p) in playable.iter().enumerate() {
        let c = anchors[i];
        let (rx, ry) = if n == 3 {
            match i { 0 => (290.0, 205.0), 1 => (305.0, 195.0), _ => (360.0, 165.0) }
        } else {
            let rx = (235.0 - n as f64 * 7.0).clamp(125.0, 220.0);
            let ry = (155.0 - n as f64 * 4.0).clamp(86.0, 148.0);
            (rx, ry)
        };
        draw_region(&mut buf, p, c, rx, ry, color_for_phrase(p, i), i);
    }

    // Seams follow structure but are subordinate and mostly textile-like.
    for pair in playable.windows(2) {
        let Some(&a) = anchor_by_id.get(&pair[0].id) else { continue; };
        let Some(&b) = anchor_by_id.get(&pair[1].id) else { continue; };
        draw_seam(&mut buf, a, b, 0.50);
    }
    for (idx, jline) in phrases.iter().enumerate().filter(|(_, p)| p.jump.is_some()) {
        let Some(j) = &jline.jump else { continue; };
        let Some(&target) = anchor_by_id.get(&j.target_id) else { continue; };
        let source = phrases[..idx]
            .iter()
            .rev()
            .find(|p| p.jump.is_none() && p.control.is_none())
            .and_then(|p| anchor_by_id.get(&p.id))
            .copied()
            .unwrap_or(target);
        if source.sub(target).len() > 4.0 { draw_seam(&mut buf, source, target, 0.35); }
    }

    // All-over stitch dust binds regions and negative space into one rug.
    for i in 0..3200u32 {
        let x = 18.0 + h01(i * 17) * (W as f64 - 36.0);
        let y = 18.0 + h01(i * 43) * (H as f64 - 36.0);
        let col = match i % 6 {
            0 => [214, 154, 55],
            1 => [96, 204, 190],
            2 => [190, 70, 132],
            3 => [210, 85, 48],
            4 => [126, 180, 82],
            _ => [204, 190, 132],
        };
        if i % 3 == 0 {
            line(&mut buf, Pt { x: x - 1.6, y }, Pt { x: x + 1.6, y }, col, 0.16, 0.45);
        } else {
            dot(&mut buf, x, y, 0.9, col, 0.16);
        }
    }

    draw_border(&mut buf);

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
    // No foreground composite: the old animation is gone.  Keep only the rug video stream plus original audio.
    let status = Command::new("ffmpeg")
        .args(["-y", "-loop", "1", "-framerate", "30", "-i", &src, "-i", path])
        .args(["-map", "0:v", "-map", "1:a"])
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
