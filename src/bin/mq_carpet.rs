// mq_carpet.rs — deterministic symbolic carpet renderer for .mq sessions
//
// Dependency-free prototype.  It writes PPM stills directly and MP4 tours by
// streaming raw RGB frames to ffmpeg.
//
// Usage:
//   cargo run --release --bin mq_carpet -- magiccarpet.mq magiccarpet.ppm
//   ffmpeg -y -i magiccarpet.ppm magiccarpet.png
//   cargo run --release --bin mq_carpet -- magiccarpet.mq magiccarpet.mp4

use std::collections::HashMap;
use std::env;
use std::f64::consts::TAU;
use std::fs;
use std::io::{self, Write};
use std::process::{Command, Stdio};

const W: usize = 1280;
const H: usize = 720;
const FPS: usize = 30;
const TOUR_SECS: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Ratio { p: u32, q: u32 }

impl Ratio {
    fn parse(s: &str) -> Option<Self> {
        let (a, b) = s.split_once('/')?;
        let p: u32 = a.parse().ok()?;
        let q: u32 = b.parse().ok()?;
        if q == 0 { return None; }
        let g = gcd(p.max(1), q.max(1));
        Some(Self { p: p / g, q: q / g })
    }
    fn value(self) -> f64 { self.p as f64 / self.q as f64 }
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 { let t = b; b = a % b; a = t; }
    a.max(1)
}

#[derive(Clone, Debug)]
enum Kind {
    Header,
    Create { name: String, ratios: Vec<Ratio> },
    Setting,
    Phrase { maqams: Vec<String>, rhythm: String },
    Jump { target: usize, count: usize },
    Other,
}

#[derive(Clone, Debug)]
struct Node { id: usize, line: String, kind: Kind }

struct Session {
    nodes: Vec<Node>,
    maqams: HashMap<String, Vec<Ratio>>,
    bpm: f64,
}

#[derive(Clone, Copy, Debug)]
struct Pt { x: f64, y: f64 }

#[derive(Clone, Copy)]
struct Territory {
    node_idx: usize,
    c: Pt,
    rx: f64,
    ry: f64,
    color: [u8; 3],
}

struct Geometry {
    territories: Vec<Territory>,
    node_pos: Vec<Pt>,
    path: Vec<Pt>,
}

#[derive(Clone)]
struct Image { rgb: Vec<u8> }

impl Image {
    fn new(color: [u8; 3]) -> Self {
        let mut rgb = vec![0u8; W * H * 3];
        for px in rgb.chunks_exact_mut(3) { px.copy_from_slice(&color); }
        Self { rgb }
    }

    fn write_ppm(&self, path: &str) -> io::Result<()> {
        let mut f = fs::File::create(path)?;
        write!(f, "P6\n{W} {H}\n255\n")?;
        f.write_all(&self.rgb)
    }

    fn blend_px(&mut self, x: i32, y: i32, over: [u8; 3], alpha: f32) {
        if x < 0 || y < 0 || x >= W as i32 || y >= H as i32 { return; }
        let i = (y as usize * W + x as usize) * 3;
        let a = alpha.clamp(0.0, 1.0);
        for ch in 0..3 {
            let base = self.rgb[i + ch] as f32;
            self.rgb[i + ch] = (base * (1.0 - a) + over[ch] as f32 * a).round().clamp(0.0, 255.0) as u8;
        }
    }

    fn darkened(&self, k: f32) -> Self {
        let mut out = self.clone();
        for c in &mut out.rgb { *c = (*c as f32 * k).round().clamp(0.0, 255.0) as u8; }
        out
    }

    fn brightened(&self) -> Self {
        let mut out = self.clone();
        for c in &mut out.rgb { *c = (*c as f32 * 1.35 + 9.0).round().clamp(0.0, 255.0) as u8; }
        out
    }

    fn redraw_from(&mut self, src: &Image, cx: f64, cy: f64, r: f64, strength: f32) {
        let xmin = (cx - r * 2.8).floor().max(0.0) as i32;
        let xmax = (cx + r * 2.8).ceil().min((W - 1) as f64) as i32;
        let ymin = (cy - r * 2.8).floor().max(0.0) as i32;
        let ymax = (cy + r * 2.8).ceil().min((H - 1) as f64) as i32;
        for y in ymin..=ymax {
            for x in xmin..=xmax {
                let dx = (x as f64 - cx) / r;
                let dy = (y as f64 - cy) / r;
                let a = (-(dx * dx + dy * dy) * 0.70).exp() as f32 * strength;
                if a < 0.01 { continue; }
                let i = (y as usize * W + x as usize) * 3;
                self.blend_px(x, y, [src.rgb[i], src.rgb[i + 1], src.rgb[i + 2]], a);
            }
        }
    }
}

fn parse_session(text: &str) -> Session {
    let mut nodes = Vec::new();
    let mut maqams = builtin_maqams();
    let mut bpm = 120.0;

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() { continue; }
        let id = nodes.len();
        let kind = parse_line(line, &mut maqams, &mut bpm);
        nodes.push(Node { id, line: line.to_string(), kind });
    }

    Session { nodes, maqams, bpm }
}

fn parse_line(line: &str, maqams: &mut HashMap<String, Vec<Ratio>>, bpm: &mut f64) -> Kind {
    if line == "MAQAM_SESSION_V2" { return Kind::Header; }
    let mut toks = line.split_whitespace();
    let Some(first) = toks.next() else { return Kind::Other; };
    match first.to_ascii_lowercase().as_str() {
        "create" => {
            let Some(name) = toks.next() else { return Kind::Other; };
            let ratios: Vec<Ratio> = toks.filter_map(Ratio::parse).collect();
            if !ratios.is_empty() { maqams.insert(name.to_ascii_lowercase(), ratios.clone()); }
            Kind::Create { name: name.to_string(), ratios }
        }
        "bpm" => {
            if let Some(v) = toks.next().and_then(|s| s.parse::<f64>().ok()) { *bpm = v; }
            Kind::Setting
        }
        "s" | "sus" | "vol" => Kind::Setting,
        "j" => {
            let target = toks.next().and_then(|s| s.parse::<usize>().ok()).unwrap_or(0);
            let count = toks.next().and_then(|s| s.parse::<usize>().ok()).unwrap_or(1);
            Kind::Jump { target, count }
        }
        _ => parse_phrase(line),
    }
}

fn parse_phrase(line: &str) -> Kind {
    let rhythm = line.split_whitespace().rev()
        .find(|tok| tok.chars().all(|c| c.is_ascii_digit() && c != '0'))
        .unwrap_or("44")
        .to_string();
    let phrase_part = line.strip_suffix(&rhythm).unwrap_or(line).trim();
    let mut maqams = Vec::new();
    for part in phrase_part.split(',') {
        let words: Vec<&str> = part.split_whitespace().collect();
        if let Some(name) = words.get(1).or_else(|| words.get(0)) {
            maqams.push(name.to_ascii_lowercase());
        }
    }
    if maqams.is_empty() { maqams.push("unknown".to_string()); }
    Kind::Phrase { maqams, rhythm }
}

fn builtin_maqams() -> HashMap<String, Vec<Ratio>> {
    let mut out = HashMap::new();
    for (name, ratios) in [
        ("nahawand", "1/1 9/8 32/27 4/3 3/2"),
        ("bayati", "1/1 12/11 32/27 4/3 3/2"),
        ("hijaz", "1/1 256/243 81/64 4/3 3/2"),
        ("rast", "1/1 9/8 27/22 4/3 3/2"),
        ("kurd", "1/1 256/243 32/27 4/3 3/2"),
        ("saba", "1/1 13/12 32/27 6/5"),
        ("zaba", "1/1 12/11 32/27 11/8"),
        ("ajam", "1/1 9/8 5/4 4/3 3/2"),
    ] {
        out.insert(name.to_string(), ratios.split_whitespace().filter_map(Ratio::parse).collect());
    }
    out
}

fn build_geometry(s: &Session) -> Geometry {
    let phrase_idxs: Vec<usize> = s.nodes.iter().enumerate()
        .filter(|(_, n)| matches!(&n.kind, Kind::Phrase { .. }))
        .map(|(i, _)| i)
        .collect();
    let n = phrase_idxs.len().max(1);
    let mut node_pos = vec![Pt { x: W as f64 / 2.0, y: H as f64 / 2.0 }; s.nodes.len()];
    let mut territories = Vec::new();

    for (rank, idx) in phrase_idxs.iter().copied().enumerate() {
        let theta = -0.75 + TAU * rank as f64 / n as f64;
        let mut seed = hash(&s.nodes[idx].line);
        let c = Pt {
            x: W as f64 * 0.50 + W as f64 * 0.27 * theta.cos() + (rand01(&mut seed) - 0.5) * 80.0,
            y: H as f64 * 0.50 + H as f64 * 0.26 * theta.sin() + (rand01(&mut seed) - 0.5) * 55.0,
        };
        node_pos[idx] = c;
        if let Kind::Phrase { maqams, rhythm } = &s.nodes[idx].kind {
            let beats: usize = rhythm.chars().filter_map(|c| c.to_digit(10)).map(|d| d as usize).sum();
            let color = palette(maqams.first().map(String::as_str).unwrap_or("unknown"), idx);
            territories.push(Territory {
                node_idx: idx,
                c,
                rx: 145.0 + beats as f64 * 4.8 + maqams.len() as f64 * 24.0,
                ry: 105.0 + beats as f64 * 2.8,
                color,
            });
        }
    }

    for i in 0..s.nodes.len() {
        if matches!(&s.nodes[i].kind, Kind::Phrase { .. }) { continue; }
        let prev = (0..i).rev().find(|&j| matches!(&s.nodes[j].kind, Kind::Phrase { .. })).map(|j| node_pos[j]);
        let next = (i + 1..s.nodes.len()).find(|&j| matches!(&s.nodes[j].kind, Kind::Phrase { .. })).map(|j| node_pos[j]);
        node_pos[i] = match (prev, next) {
            (Some(a), Some(b)) => Pt { x: (a.x + b.x) * 0.5, y: (a.y + b.y) * 0.5 },
            (Some(a), None) => a,
            (None, Some(b)) => b,
            _ => Pt { x: W as f64 * 0.5, y: H as f64 * 0.5 },
        };
    }

    let mut path = Vec::new();
    for node in &s.nodes {
        match &node.kind {
            Kind::Phrase { .. } => path.push(node_pos[node.id]),
            Kind::Jump { target, .. } => {
                path.push(node_pos[node.id]);
                if let Some(ti) = s.nodes.iter().position(|n| n.id == *target) { path.push(node_pos[ti]); }
            }
            _ => {}
        }
    }
    if path.is_empty() { path.push(Pt { x: W as f64 * 0.5, y: H as f64 * 0.5 }); }

    Geometry { territories, node_pos, path }
}

fn render_carpet(s: &Session, g: &Geometry, terminal_safe: bool) -> Image {
    let mut img = Image::new(if terminal_safe { [5, 5, 9] } else { [10, 8, 14] });
    draw_backing(&mut img);
    for t in &g.territories { draw_territory(&mut img, s, *t, terminal_safe); }
    draw_seams(&mut img, s, g, terminal_safe);
    if terminal_safe { img.darkened(0.68) } else { img }
}

fn draw_backing(img: &mut Image) {
    for y in (0..H).step_by(12) {
        for x in 0..W as i32 { img.blend_px(x, y as i32, [36, 34, 50], 0.11); }
    }
    for x in (0..W).step_by(17) {
        for y in 0..H as i32 { img.blend_px(x as i32, y, [28, 32, 42], 0.08); }
    }
}

fn draw_territory(img: &mut Image, s: &Session, t: Territory, terminal_safe: bool) {
    let Kind::Phrase { maqams, rhythm } = &s.nodes[t.node_idx].kind else { return; };
    let base = t.color;
    let xmin = (t.c.x - t.rx * 1.25).max(0.0) as i32;
    let xmax = (t.c.x + t.rx * 1.25).min((W - 1) as f64) as i32;
    let ymin = (t.c.y - t.ry * 1.25).max(0.0) as i32;
    let ymax = (t.c.y + t.ry * 1.25).min((H - 1) as f64) as i32;
    for y in ymin..=ymax {
        for x in xmin..=xmax {
            let dx = (x as f64 - t.c.x) / t.rx;
            let dy = (y as f64 - t.c.y) / t.ry;
            let wob = 0.10 * (dx * 9.0 + dy * 4.0 + t.node_idx as f64).sin()
                + 0.06 * (dx * 17.0 - dy * 6.0).cos();
            let d2 = dx * dx + dy * dy;
            if d2 < 1.0 + wob {
                let edge = (1.0 - d2).max(0.0) as f32;
                img.blend_px(x, y, base, if terminal_safe { 0.11 + edge * 0.10 } else { 0.20 + edge * 0.23 });
            }
        }
    }

    let groups: Vec<usize> = rhythm.chars().filter_map(|c| c.to_digit(10)).map(|d| d as usize).collect();
    let n_groups = groups.len().max(1);
    let total = groups.iter().sum::<usize>().max(1);
    let mut acc = 0usize;
    for (gi, group) in groups.iter().copied().enumerate() {
        let mid = (acc as f64 + group as f64 * 0.5) / total as f64;
        acc += group;
        let y = t.c.y - t.ry * 0.47 + (gi as f64 + 0.5) * (t.ry * 0.94 / n_groups as f64);
        let len = t.rx * (0.7 + group as f64 * 0.04);
        let angle = -0.55 + mid * 1.9;
        let a = Pt { x: t.c.x - len * angle.cos() * 0.5, y: y - len * angle.sin() * 0.20 };
        let b = Pt { x: t.c.x + len * angle.cos() * 0.5, y: y + len * angle.sin() * 0.20 };
        line(img, a, b, tint(base, 45), if terminal_safe { 0.20 } else { 0.36 }, 3 + group as i32);
        for k in 0..group {
            let u = (k as f64 + 0.5) / group as f64;
            disc(img, a.x * (1.0 - u) + b.x * u, a.y * (1.0 - u) + b.y * u, 2.4, [220, 205, 150], if terminal_safe { 0.18 } else { 0.38 });
        }
    }

    for (mi, maqam) in maqams.iter().enumerate() {
        if let Some(ratios) = s.maqams.get(maqam) {
            let cx = t.c.x + (mi as f64 - (maqams.len() as f64 - 1.0) * 0.5) * t.rx * 0.42;
            let cy = t.c.y + t.ry * 0.20;
            ratio_cloud(img, ratios, cx, cy, t.rx.min(t.ry) * 0.18, terminal_safe);
        }
    }

    let mut seed = hash(&s.nodes[t.node_idx].line);
    let count = if terminal_safe { 230 } else { 420 };
    for _ in 0..count {
        let a = rand01(&mut seed) * TAU;
        let r = rand01(&mut seed).sqrt();
        let x = t.c.x + a.cos() * r * t.rx * 0.95;
        let y = t.c.y + a.sin() * r * t.ry * 0.95;
        let len = 5.0 + rand01(&mut seed) * 17.0;
        let th = a + (rand01(&mut seed) - 0.5) * 1.2;
        line(img, Pt { x: x - th.cos() * len * 0.5, y: y - th.sin() * len * 0.5 }, Pt { x: x + th.cos() * len * 0.5, y: y + th.sin() * len * 0.5 }, tint(base, 34), if terminal_safe { 0.10 } else { 0.20 }, 1);
    }
}

fn ratio_cloud(img: &mut Image, ratios: &[Ratio], cx: f64, cy: f64, scale: f64, terminal_safe: bool) {
    let mut pts = Vec::new();
    for &r in ratios {
        let (x, y) = harmonic_offset(r, scale);
        let p = Pt { x: cx + x, y: cy + y };
        pts.push((r, p));
        disc(img, p.x, p.y, if r.p == 1 && r.q == 1 { 4.3 } else { 3.2 }, ratio_color(r), if terminal_safe { 0.22 } else { 0.50 });
    }
    for i in 0..pts.len() {
        for j in i + 1..pts.len() {
            let (a, pa) = pts[i];
            let (b, pb) = pts[j];
            let mut q = a.value() / b.value();
            if q < 1.0 { q = 1.0 / q; }
            if (q - 81.0 / 80.0).abs() < 0.0025 {
                line(img, pa, pb, [245, 230, 135], if terminal_safe { 0.24 } else { 0.72 }, 2);
            } else if ((pa.x - pb.x).powi(2) + (pa.y - pb.y).powi(2)).sqrt() < scale * 1.45 {
                line(img, pa, pb, [170, 160, 130], if terminal_safe { 0.08 } else { 0.18 }, 1);
            }
        }
    }
}

fn harmonic_offset(r: Ratio, scale: f64) -> (f64, f64) {
    let mut n = r.p;
    let mut d = r.q;
    let primes = [2u32, 3, 5, 7, 11, 13];
    let mut x = 0.0;
    let mut y = 0.0;
    for (i, p) in primes.iter().enumerate() {
        let mut e = 0i32;
        while n % *p == 0 { n /= *p; e += 1; }
        while d % *p == 0 { d /= *p; e -= 1; }
        let a = TAU * i as f64 / primes.len() as f64;
        x += e as f64 * a.cos();
        y += e as f64 * a.sin();
    }
    (x * scale, y * scale)
}

fn draw_seams(img: &mut Image, s: &Session, g: &Geometry, terminal_safe: bool) {
    let col = if terminal_safe { [115, 100, 64] } else { [220, 188, 92] };
    let phrase_positions: Vec<Pt> = s.nodes.iter().enumerate()
        .filter(|(_, n)| matches!(&n.kind, Kind::Phrase { .. }))
        .map(|(i, _)| g.node_pos[i])
        .collect();
    for w in phrase_positions.windows(2) { curve(img, w[0], w[1], col, if terminal_safe { 0.18 } else { 0.34 }, 5); }
    for node in &s.nodes {
        if let Kind::Jump { target, count } = &node.kind {
            if let Some(ti) = s.nodes.iter().position(|n| n.id == *target) {
                let from = g.node_pos[node.id];
                let to = g.node_pos[ti];
                curve(img, from, to, col, if terminal_safe { 0.26 } else { 0.56 }, 8);
                knot(img, from, (*count).clamp(1, 9), col, terminal_safe);
            }
        }
    }
}

fn curve(img: &mut Image, a: Pt, b: Pt, rgb: [u8; 3], alpha: f32, thick: i32) {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let len = (dx * dx + dy * dy).sqrt().max(1.0);
    let c = Pt { x: (a.x + b.x) * 0.5 - dy / len * len * 0.18, y: (a.y + b.y) * 0.5 + dx / len * len * 0.18 };
    let mut prev = a;
    for i in 1..=48 {
        let t = i as f64 / 48.0;
        let u = 1.0 - t;
        let p = Pt { x: u * u * a.x + 2.0 * u * t * c.x + t * t * b.x, y: u * u * a.y + 2.0 * u * t * c.y + t * t * b.y };
        line(img, prev, p, rgb, alpha, thick);
        prev = p;
    }
}

fn knot(img: &mut Image, p: Pt, loops: usize, rgb: [u8; 3], terminal_safe: bool) {
    for k in 0..loops {
        let r = 10.0 + k as f64 * 5.5;
        let mut prev = Pt { x: p.x + r, y: p.y };
        for i in 1..=48 {
            let a = TAU * i as f64 / 48.0;
            let q = Pt { x: p.x + r * a.cos(), y: p.y + r * 0.62 * a.sin() };
            line(img, prev, q, rgb, if terminal_safe { 0.17 } else { 0.42 }, 2);
            prev = q;
        }
    }
}

fn disc(img: &mut Image, cx: f64, cy: f64, r: f64, rgb: [u8; 3], alpha: f32) {
    for y in (cy - r).floor() as i32..=(cy + r).ceil() as i32 {
        for x in (cx - r).floor() as i32..=(cx + r).ceil() as i32 {
            let dx = x as f64 - cx;
            let dy = y as f64 - cy;
            if dx * dx + dy * dy <= r * r { img.blend_px(x, y, rgb, alpha); }
        }
    }
}

fn line(img: &mut Image, a: Pt, b: Pt, rgb: [u8; 3], alpha: f32, thick: i32) {
    let mut x0 = a.x.round() as i32;
    let mut y0 = a.y.round() as i32;
    let x1 = b.x.round() as i32;
    let y1 = b.y.round() as i32;
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let half = thick.max(1) / 2;
    loop {
        for oy in -half..=half { for ox in -half..=half { img.blend_px(x0 + ox, y0 + oy, rgb, alpha); } }
        if x0 == x1 && y0 == y1 { break; }
        let e2 = 2 * err;
        if e2 >= dy { err += dy; x0 += sx; }
        if e2 <= dx { err += dx; y0 += sy; }
    }
}

fn path_pos(path: &[Pt], beat: f64) -> Pt {
    if path.is_empty() { return Pt { x: W as f64 * 0.5, y: H as f64 * 0.5 }; }
    if path.len() == 1 { return path[0]; }
    let i = beat.floor().rem_euclid(path.len() as f64) as usize;
    let j = (i + 1) % path.len();
    let t = beat - beat.floor();
    let t = t * t * (3.0 - 2.0 * t);
    Pt { x: path[i].x * (1.0 - t) + path[j].x * t, y: path[i].y * (1.0 - t) + path[j].y * t }
}

fn render_mp4(s: &Session, g: &Geometry, output: &str) -> io::Result<()> {
    let base = render_carpet(s, g, true);
    let dark = base.darkened(0.58);
    let bright = base.brightened();
    let mut child = Command::new("ffmpeg")
        .args([
            "-y", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", "1280x720", "-r", "30", "-i", "-",
            "-an", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-profile:v", "baseline", "-level", "3.1",
            "-movflags", "+faststart", output,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()?;
    {
        let stdin = child.stdin.as_mut().ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "ffmpeg stdin unavailable"))?;
        let beats_per_sec = s.bpm.max(20.0) / 60.0;
        for f in 0..(FPS * TOUR_SECS) {
            let beat = f as f64 / FPS as f64 * beats_per_sec;
            let p = path_pos(&g.path, beat);
            let pulse = (-(beat - beat.floor()) * 10.0).exp() as f32;
            let mut frame = dark.clone();
            frame.redraw_from(&bright, p.x, p.y, 62.0 + pulse as f64 * 10.0, 0.82);
            for k in 1..9 {
                let q = path_pos(&g.path, beat - k as f64 * 0.55);
                frame.redraw_from(&bright, q.x, q.y, 34.0, 0.12 * (1.0 - k as f32 / 10.0));
            }
            stdin.write_all(&frame.rgb)?;
        }
    }
    let status = child.wait()?;
    if !status.success() { return Err(io::Error::new(io::ErrorKind::Other, "ffmpeg failed")); }
    Ok(())
}

fn palette(name: &str, id: usize) -> [u8; 3] {
    match name {
        "hijaz" | "hijaz2" => [170, 68, 112],
        "bayati" | "bayati2" => [74, 150, 105],
        "saba" | "saba2" | "zaba" => [62, 128, 150],
        "rast" | "suznak" => [155, 125, 58],
        "nahawand" | "kurd" => [95, 90, 165],
        "ajam" | "jiharkah" => [160, 135, 78],
        _ => { let h = hash(name).wrapping_add(id as u64 * 7919); [55 + (h & 63) as u8, 65 + ((h >> 8) & 79) as u8, 85 + ((h >> 16) & 75) as u8] }
    }
}

fn ratio_color(r: Ratio) -> [u8; 3] {
    match (r.p, r.q) {
        (1, 1) => [210, 195, 135],
        (13, 12) => [205, 78, 128],
        (12, 11) => [210, 95, 165],
        (32, 27) => [88, 145, 210],
        (6, 5) => [115, 210, 130],
        (5, 4) => [95, 190, 115],
        (4, 3) => [85, 165, 215],
        (3, 2) => [190, 160, 220],
        (9, 8) => [140, 195, 90],
        _ => [175, 145, 190],
    }
}

fn tint(rgb: [u8; 3], lift: i16) -> [u8; 3] {
    [
        (rgb[0] as i16 + lift).clamp(0, 255) as u8,
        (rgb[1] as i16 + lift).clamp(0, 255) as u8,
        (rgb[2] as i16 + lift).clamp(0, 255) as u8,
    ]
}

fn hash(s: &str) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in s.as_bytes() { h ^= *b as u64; h = h.wrapping_mul(0x100000001b3); }
    h
}

fn rand01(seed: &mut u64) -> f64 {
    *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    ((*seed >> 11) as f64) / ((1u64 << 53) as f64)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: {} <session.mq> <out.ppm|out.mp4> [--art]", args[0]);
        eprintln!("example: cargo run --release --bin mq_carpet -- growl.mq growl-carpet.ppm");
        eprintln!("example: cargo run --release --bin mq_carpet -- growl.mq growl-tour.mp4");
        return Ok(());
    }
    let src = fs::read_to_string(&args[1])?;
    let session = parse_session(&src);
    let geom = build_geometry(&session);
    let out = &args[2];
    if out.ends_with(".mp4") {
        render_mp4(&session, &geom, out)?;
    } else {
        let art = args.iter().any(|a| a == "--art");
        render_carpet(&session, &geom, !art).write_ppm(out)?;
        if !out.ends_with(".ppm") {
            eprintln!("note: still output is PPM bytes; prefer .ppm, then convert with ffmpeg");
        }
    }
    println!("saved {out}");
    Ok(())
}
