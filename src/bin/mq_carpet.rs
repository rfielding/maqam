// mq_carpet.rs — deterministic symbolic carpet renderer for .mq sessions
//
// This prototype intentionally renders source structure, not an expanded
// performance trace.  Phrases become dark woven territories, rhythm groups
// modulate the internal weave, ratios become stitch constellations, and jumps
// remain symbolic seam/knot objects.  The optional MP4 keeps the carpet fixed
// and uses a bright re-draw region as a reading cursor.
//
// Usage:
//   cargo run --release --bin mq_carpet -- growl.mq growl-carpet.ppm
//   cargo run --release --bin mq_carpet -- growl.mq growl-tour.mp4
//
// Output is PPM for stills because this repo intentionally has no image crate.
// For PNG, convert with ffmpeg:
//   ffmpeg -y -i growl-carpet.ppm growl-carpet.png

use std::collections::HashMap;
use std::env;
use std::f64::consts::TAU;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Stdio};

const W: usize = 1280;
const H: usize = 720;
const FPS: usize = 30;
const TOUR_SECS: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Ratio {
    p: u32,
    q: u32,
}

impl Ratio {
    fn new(p: u32, q: u32) -> Self {
        let g = gcd(p.max(1), q.max(1));
        Ratio { p: p / g, q: q / g }
    }

    fn parse(s: &str) -> Option<Self> {
        let (a, b) = s.split_once('/')?;
        let p = a.parse().ok()?;
        let q = b.parse().ok()?;
        if q == 0 { return None; }
        Some(Ratio::new(p, q))
    }

    fn as_f64(self) -> f64 { self.p as f64 / self.q as f64 }
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a.max(1)
}

#[derive(Clone, Debug)]
enum NodeKind {
    Header,
    Create { name: String, ratios: Vec<Ratio> },
    Setting { key: String, value: String },
    Phrase { src: String, maqams: Vec<String>, rhythm: String },
    Jump { target: usize, count: usize },
    Other(String),
}

#[derive(Clone, Debug)]
struct Node {
    id: usize,
    line: String,
    kind: NodeKind,
}

#[derive(Clone, Debug)]
struct Session {
    nodes: Vec<Node>,
    maqams: HashMap<String, Vec<Ratio>>,
    bpm: f64,
    sustain: f64,
}

#[derive(Clone, Copy)]
struct Pt { x: f64, y: f64 }

#[derive(Clone)]
struct Territory {
    node_idx: usize,
    center: Pt,
    rx: f64,
    ry: f64,
    color: [u8; 3],
}

#[derive(Clone)]
struct CarpetGeometry {
    territories: Vec<Territory>,
    node_pos: Vec<Pt>,
    reading_path: Vec<Pt>,
}

#[derive(Clone)]
struct Image {
    w: usize,
    h: usize,
    rgb: Vec<u8>,
}

impl Image {
    fn new(w: usize, h: usize, rgb: [u8; 3]) -> Self {
        let mut data = vec![0u8; w * h * 3];
        for px in data.chunks_exact_mut(3) {
            px.copy_from_slice(&rgb);
        }
        Self { w, h, rgb: data }
    }

    fn write_ppm(&self, path: &str) -> io::Result<()> {
        let mut f = fs::File::create(path)?;
        write!(f, "P6\n{} {}\n255\n", self.w, self.h)?;
        f.write_all(&self.rgb)?;
        Ok(())
    }

    fn darkened(&self, factor: f32) -> Self {
        let mut out = self.clone();
        for c in &mut out.rgb {
            *c = ((*c as f32) * factor).round().clamp(0.0, 255.0) as u8;
        }
        out
    }

    fn blend_px(&mut self, x: i32, y: i32, rgb: [u8; 3], alpha: f32) {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 { return; }
        let i = (y as usize * self.w + x as usize) * 3;
        let a = alpha.clamp(0.0, 1.0);
        for ch in 0..3 {
            let base = self.rgb[i + ch] as f32;
            let over = rgb[ch] as f32;
            self.rgb[i + ch] = (base * (1.0 - a) + over * a).round().clamp(0.0, 255.0) as u8;
        }
    }

    fn redraw_from(&mut self, bright: &Image, cx: f64, cy: f64, radius: f64, strength: f32) {
        let r = radius.max(1.0);
        let xmin = (cx - r * 2.5).floor().max(0.0) as i32;
        let xmax = (cx + r * 2.5).ceil().min((self.w - 1) as f64) as i32;
        let ymin = (cy - r * 2.5).floor().max(0.0) as i32;
        let ymax = (cy + r * 2.5).ceil().min((self.h - 1) as f64) as i32;
        for y in ymin..=ymax {
            for x in xmin..=xmax {
                let dx = (x as f64 - cx) / r;
                let dy = (y as f64 - cy) / r;
                let d2 = dx * dx + dy * dy;
                let a = (-(d2) * 0.65).exp() as f32 * strength;
                if a < 0.01 { continue; }
                let i = (y as usize * self.w + x as usize) * 3;
                let rgb = [bright.rgb[i], bright.rgb[i + 1], bright.rgb[i + 2]];
                self.blend_px(x, y, rgb, a);
            }
        }
    }

    fn brighten(&self, gain: f32, lift: u8) -> Self {
        let mut out = self.clone();
        for c in &mut out.rgb {
            *c = ((*c as f32) * gain + lift as f32).round().clamp(0.0, 255.0) as u8;
        }
        out
    }
}

fn parse_session(src: &str) -> Session {
    let mut nodes = Vec::new();
    let mut maqams = builtin_maqams();
    let mut bpm = 120.0;
    let mut sustain = 1.25;

    for raw in src.lines() {
        let line = raw.trim();
        if line.is_empty() { continue; }
        let id = nodes.len();
        let kind = parse_line(line, &mut maqams, &mut bpm, &mut sustain);
        nodes.push(Node { id, line: line.to_string(), kind });
    }

    Session { nodes, maqams, bpm, sustain }
}

fn parse_line(line: &str, maqams: &mut HashMap<String, Vec<Ratio>>, bpm: &mut f64, sustain: &mut f64) -> NodeKind {
    if line == "MAQAM_SESSION_V2" { return NodeKind::Header; }
    let mut toks = line.split_whitespace();
    let Some(first) = toks.next() else { return NodeKind::Other(line.into()); };
    let first_l = first.to_ascii_lowercase();
    match first_l.as_str() {
        "create" => {
            let Some(name) = toks.next() else { return NodeKind::Other(line.into()); };
            let ratios: Vec<Ratio> = toks.filter_map(Ratio::parse).collect();
            if !ratios.is_empty() {
                maqams.insert(name.to_ascii_lowercase(), ratios.clone());
            }
            NodeKind::Create { name: name.to_string(), ratios }
        }
        "bpm" => {
            if let Some(v) = toks.next().and_then(|s| s.parse::<f64>().ok()) { *bpm = v; }
            NodeKind::Setting { key: "bpm".into(), value: line[3..].trim().into() }
        }
        "s" | "sus" => {
            if let Some(v) = toks.next().and_then(|s| s.parse::<f64>().ok()) { *sustain = v; }
            NodeKind::Setting { key: "s".into(), value: line[first.len()..].trim().into() }
        }
        "vol" => NodeKind::Setting { key: "vol".into(), value: line[3..].trim().into() },
        "j" => {
            let target = toks.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let count = toks.next().and_then(|s| s.parse().ok()).unwrap_or(1);
            NodeKind::Jump { target, count }
        }
        _ => parse_phrase_line(line),
    }
}

fn parse_phrase_line(line: &str) -> NodeKind {
    let mut rhythm = "44".to_string();
    for tok in line.split_whitespace().rev() {
        if tok.chars().all(|c| c.is_ascii_digit() && c != '0') {
            rhythm = tok.to_string();
            break;
        }
    }

    let phrase_part = line.strip_suffix(&rhythm).unwrap_or(line).trim();
    let mut maqams = Vec::new();
    for part in phrase_part.split(',') {
        let words: Vec<&str> = part.split_whitespace().collect();
        if let Some(name) = words.get(1) {
            maqams.push(name.to_ascii_lowercase());
        } else if let Some(name) = words.get(0) {
            maqams.push(name.to_ascii_lowercase());
        }
    }
    if maqams.is_empty() { maqams.push("unknown".into()); }

    NodeKind::Phrase { src: phrase_part.to_string(), maqams, rhythm }
}

fn builtin_maqams() -> HashMap<String, Vec<Ratio>> {
    let mut m = HashMap::new();
    let specs = [
        ("nahawand", "1/1 9/8 32/27 4/3 3/2"),
        ("bayati", "1/1 12/11 32/27 4/3 3/2"),
        ("hijaz", "1/1 256/243 81/64 4/3 3/2"),
        ("rast", "1/1 9/8 27/22 4/3 3/2"),
        ("kurd", "1/1 256/243 32/27 4/3 3/2"),
        ("saba", "1/1 13/12 32/27 6/5"),
        ("zaba", "1/1 12/11 32/27 11/8"),
        ("ajam", "1/1 9/8 5/4 4/3 3/2"),
    ];
    for (name, ratios) in specs {
        m.insert(name.into(), ratios.split_whitespace().filter_map(Ratio::parse).collect());
    }
    m
}

fn palette_for(name: &str, id: usize) -> [u8; 3] {
    match name {
        "hijaz" | "hijaz2" => [170, 68, 112],
        "bayati" | "bayati2" => [74, 150, 105],
        "saba" | "saba2" | "zaba" => [62, 128, 150],
        "rast" | "suznak" => [155, 125, 58],
        "nahawand" | "kurd" => [95, 90, 165],
        "ajam" | "jiharkah" => [160, 135, 78],
        _ => {
            let h = hash_str(name).wrapping_add(id as u64 * 7919);
            [55 + (h & 63) as u8, 65 + ((h >> 8) & 79) as u8, 85 + ((h >> 16) & 75) as u8]
        }
    }
}

fn ratio_color(r: Ratio) -> [u8; 3] {
    match (r.p, r.q) {
        (1, 1) => [210, 195, 135],
        (13, 12) => [205, 78, 128],
        (12, 11) => [210, 95, 165],
        (11, 10) => [220, 110, 145],
        (32, 27) => [88, 145, 210],
        (6, 5) => [115, 210, 130],
        (5, 4) => [95, 190, 115],
        (4, 3) => [85, 165, 215],
        (3, 2) => [190, 160, 220],
        (9, 8) => [140, 195, 90],
        _ => [175, 145, 190],
    }
}

fn hash_str(s: &str) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn lcg(seed: &mut u64) -> f64 {
    *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    ((*seed >> 11) as f64) / ((1u64 << 53) as f64)
}

fn build_geometry(session: &Session) -> CarpetGeometry {
    let phrase_idxs: Vec<usize> = session.nodes.iter().enumerate()
        .filter(|(_, n)| matches!(n.kind, NodeKind::Phrase { .. }))
        .map(|(i, _)| i)
        .collect();
    let n = phrase_idxs.len().max(1);
    let mut territories = Vec::new();
    let mut node_pos = vec![Pt { x: W as f64 * 0.5, y: H as f64 * 0.5 }; session.nodes.len()];

    // Organic territories on a loose ellipse.  This preserves source locality
    // without advertising rectangles or a visible Hilbert grid.
    let cx0 = W as f64 * 0.50;
    let cy0 = H as f64 * 0.50;
    let major = W as f64 * 0.28;
    let minor = H as f64 * 0.25;
    for (rank, node_idx) in phrase_idxs.iter().copied().enumerate() {
        let theta = -0.9 + TAU * rank as f64 / n as f64;
        let mut seed = hash_str(&session.nodes[node_idx].line);
        let jitter_x = (lcg(&mut seed) - 0.5) * 75.0;
        let jitter_y = (lcg(&mut seed) - 0.5) * 55.0;
        let center = Pt { x: cx0 + major * theta.cos() + jitter_x, y: cy0 + minor * theta.sin() + jitter_y };
        node_pos[node_idx] = center;
        if let NodeKind::Phrase { maqams, rhythm, .. } = &session.nodes[node_idx].kind {
            let beats: usize = rhythm.chars().filter_map(|c| c.to_digit(10)).map(|d| d as usize).sum();
            let name = maqams.first().map(|s| s.as_str()).unwrap_or("unknown");
            let rx = 160.0 + beats as f64 * 4.5 + maqams.len() as f64 * 28.0;
            let ry = 118.0 + beats as f64 * 2.7;
            territories.push(Territory { node_idx, center, rx, ry, color: palette_for(name, node_idx) });
        }
    }

    // Jump/control nodes live on seams between nearby phrase territories.
    for (idx, node) in session.nodes.iter().enumerate() {
        if matches!(node.kind, NodeKind::Phrase { .. }) { continue; }
        let prev = previous_phrase_pos(idx, &session.nodes, &node_pos).unwrap_or(Pt { x: cx0 - 120.0, y: cy0 });
        let next = next_phrase_pos(idx, &session.nodes, &node_pos).unwrap_or(Pt { x: cx0 + 120.0, y: cy0 });
        node_pos[idx] = Pt { x: (prev.x + next.x) * 0.5, y: (prev.y + next.y) * 0.5 };
    }

    let reading_path = build_reading_path(session, &node_pos);
    CarpetGeometry { territories, node_pos, reading_path }
}

fn previous_phrase_pos(idx: usize, nodes: &[Node], node_pos: &[Pt]) -> Option<Pt> {
    (0..idx).rev().find(|&i| matches!(nodes[i].kind, NodeKind::Phrase { .. })).map(|i| node_pos[i])
}

fn next_phrase_pos(idx: usize, nodes: &[Node], node_pos: &[Pt]) -> Option<Pt> {
    (idx + 1..nodes.len()).find(|&i| matches!(nodes[i].kind, NodeKind::Phrase { .. })).map(|i| node_pos[i])
}

fn build_reading_path(session: &Session, pos: &[Pt]) -> Vec<Pt> {
    let mut path = Vec::new();
    for node in &session.nodes {
        match node.kind {
            NodeKind::Jump { target, .. } => {
                path.push(pos[node.id]);
                if let Some(target_idx) = session.nodes.iter().position(|n| n.id == target) {
                    path.push(pos[target_idx]);
                }
            }
            NodeKind::Phrase { .. } => path.push(pos[node.id]),
            _ => {}
        }
    }
    if path.is_empty() { path.push(Pt { x: W as f64 * 0.5, y: H as f64 * 0.5 }); }
    path
}

fn render_carpet(session: &Session, geom: &CarpetGeometry, terminal_mode: bool) -> Image {
    let mut img = Image::new(W, H, if terminal_mode { [5, 5, 9] } else { [10, 8, 14] });
    draw_background_weave(&mut img);

    // Draw broad territory fields first; let them overlap and blend like fabric.
    for territory in &geom.territories {
        draw_territory_field(&mut img, session, territory, terminal_mode);
    }

    // Draw seams and knots on top, but keep them dark enough for terminal use.
    draw_source_seams(&mut img, session, geom, terminal_mode);

    // Final veil for terminal/background readability.
    if terminal_mode {
        let veil = Image::new(W, H, [0, 0, 0]);
        let mut out = img.clone();
        for i in (0..out.rgb.len()).step_by(3) {
            for ch in 0..3 {
                out.rgb[i + ch] = (out.rgb[i + ch] as f32 * 0.62 + veil.rgb[i + ch] as f32 * 0.38)
                    .round().clamp(0.0, 255.0) as u8;
            }
        }
        out
    } else {
        img
    }
}

fn draw_background_weave(img: &mut Image) {
    for y in (0..H).step_by(12) {
        let a = if (y / 12) % 2 == 0 { 0.18 } else { 0.09 };
        for x in 0..W as i32 { img.blend_px(x, y as i32, [42, 38, 54], a); }
    }
    for x in (0..W).step_by(16) {
        let a = if (x / 16) % 2 == 0 { 0.12 } else { 0.06 };
        for y in 0..H as i32 { img.blend_px(x as i32, y, [30, 34, 45], a); }
    }
}

fn draw_territory_field(img: &mut Image, session: &Session, territory: &Territory, terminal_mode: bool) {
    let node = &session.nodes[territory.node_idx];
    let NodeKind::Phrase { maqams, rhythm, .. } = &node.kind else { return; };
    let base = territory.color;
    let mut seed = hash_str(&node.line);

    // Filled organic ellipse-ish territory, no box boundary.
    let xmin = (territory.center.x - territory.rx * 1.25).max(0.0) as i32;
    let xmax = (territory.center.x + territory.rx * 1.25).min((W - 1) as f64) as i32;
    let ymin = (territory.center.y - territory.ry * 1.25).max(0.0) as i32;
    let ymax = (territory.center.y + territory.ry * 1.25).min((H - 1) as f64) as i32;

    for y in ymin..=ymax {
        for x in xmin..=xmax {
            let dx = (x as f64 - territory.center.x) / territory.rx;
            let dy = (y as f64 - territory.center.y) / territory.ry;
            let wob = 0.11 * ((dx * 8.0 + dy * 3.0 + territory.node_idx as f64).sin())
                + 0.06 * ((dx * 17.0 - dy * 9.0).cos());
            let d2 = dx * dx + dy * dy;
            if d2 < 1.0 + wob {
                let edge = (1.0 - d2).max(0.0) as f32;
                let alpha = if terminal_mode { 0.10 + edge * 0.10 } else { 0.18 + edge * 0.22 };
                img.blend_px(x, y, base, alpha);
            }
        }
    }

    // Rhythm group bands: purely structural, no semantic assumptions.
    let groups: Vec<usize> = rhythm.chars().filter_map(|c| c.to_digit(10)).map(|d| d as usize).collect();
    let total: usize = groups.iter().sum::<usize>().max(1);
    let mut accum = 0usize;
    for (gi, g) in groups.iter().copied().enumerate() {
        let t0 = accum as f64 / total as f64;
        let t1 = (accum + g) as f64 / total as f64;
        accum += g;
        let angle = -0.45 + (t0 + t1) * 1.9;
        let len = territory.rx * (0.68 + g as f64 * 0.035);
        let band_y = territory.center.y - territory.ry * 0.48 + (gi as f64 + 0.5) * (territory.ry * 0.96 / groups.len().max(1) as f64);
        let cx = territory.center.x + (t0 - 0.5) * territory.rx * 0.42;
        let p0 = Pt { x: cx - len * angle.cos() * 0.5, y: band_y - len * angle.sin() * 0.25 };
        let p1 = Pt { x: cx + len * angle.cos() * 0.5, y: band_y + len * angle.sin() * 0.25 };
        let alpha = if terminal_mode { 0.18 } else { 0.34 };
        draw_thick_line_img(img, p0, p1, tint(base, 46), alpha, 3 + g as i32);
        // Subdivision dots within group.
        for k in 0..g {
            let u = (k as f64 + 0.5) / g as f64;
            let x = p0.x * (1.0 - u) + p1.x * u;
            let y = p0.y * (1.0 - u) + p1.y * u;
            draw_disc(img, x, y, 2.0 + g as f64 * 0.15, [220, 205, 150], if terminal_mode { 0.20 } else { 0.38 });
        }
    }

    // Ratio stitch constellations from each maqam in the phrase.
    for (mi, name) in maqams.iter().enumerate() {
        if let Some(ratios) = session.maqams.get(name) {
            let cluster_cx = territory.center.x + (mi as f64 - (maqams.len() as f64 - 1.0) * 0.5) * territory.rx * 0.42;
            let cluster_cy = territory.center.y + territory.ry * 0.20;
            draw_ratio_cloud(img, ratios, cluster_cx, cluster_cy, territory.ry.min(territory.rx) * 0.18, terminal_mode);
        }
    }

    // Dense stitch texture; deterministic but does not look gridded.
    let stitch_count = if terminal_mode { 260 } else { 470 };
    for _ in 0..stitch_count {
        let a = lcg(&mut seed) * TAU;
        let r = lcg(&mut seed).sqrt();
        let x = territory.center.x + a.cos() * r * territory.rx * (0.25 + 0.72 * lcg(&mut seed));
        let y = territory.center.y + a.sin() * r * territory.ry * (0.25 + 0.72 * lcg(&mut seed));
        let len = 5.0 + 18.0 * lcg(&mut seed);
        let th = a + (lcg(&mut seed) - 0.5) * 1.2;
        let p0 = Pt { x: x - th.cos() * len * 0.5, y: y - th.sin() * len * 0.5 };
        let p1 = Pt { x: x + th.cos() * len * 0.5, y: y + th.sin() * len * 0.5 };
        draw_thick_line_img(img, p0, p1, tint(base, 35), if terminal_mode { 0.10 } else { 0.20 }, 1);
    }
}

fn draw_ratio_cloud(img: &mut Image, ratios: &[Ratio], cx: f64, cy: f64, scale: f64, terminal_mode: bool) {
    let mut points = Vec::new();
    for &r in ratios {
        let (x, y) = harmonic_offset(r, scale);
        let px = cx + x;
        let py = cy + y;
        points.push((r, px, py));
        draw_disc(img, px, py, if r.p == 1 && r.q == 1 { 4.5 } else { 3.4 }, ratio_color(r), if terminal_mode { 0.22 } else { 0.50 });
    }

    // Edges between close harmonic neighbors, with 81/80 comma highlighted.
    for i in 0..points.len() {
        for j in i + 1..points.len() {
            let (a, x0, y0) = points[i];
            let (b, x1, y1) = points[j];
            let mut q = a.as_f64() / b.as_f64();
            if q < 1.0 { q = 1.0 / q; }
            if (q - 81.0 / 80.0).abs() < 0.002 {
                draw_thick_line_img(img, Pt { x: x0, y: y0 }, Pt { x: x1, y: y1 }, [245, 230, 135], if terminal_mode { 0.25 } else { 0.75 }, 2);
            } else if (x0 - x1).hypot(y0 - y1) < scale * 1.45 {
                draw_thick_line_img(img, Pt { x: x0, y: y0 }, Pt { x: x1, y: y1 }, [170, 160, 130], if terminal_mode { 0.08 } else { 0.17 }, 1);
            }
        }
    }
}

fn harmonic_offset(r: Ratio, scale: f64) -> (f64, f64) {
    let mut n = r.p;
    let mut d = r.q;
    let primes = [2, 3, 5, 7, 11, 13];
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

fn draw_source_seams(img: &mut Image, session: &Session, geom: &CarpetGeometry, terminal_mode: bool) {
    let seam_col = if terminal_mode { [116, 100, 62] } else { [218, 188, 94] };

    // Fall-through thread.
    let phrase_positions: Vec<Pt> = session.nodes.iter().enumerate()
        .filter(|(_, n)| matches!(n.kind, NodeKind::Phrase { .. }))
        .map(|(i, _)| geom.node_pos[i])
        .collect();
    for w in phrase_positions.windows(2) {
        draw_curve(img, w[0], w[1], seam_col, if terminal_mode { 0.18 } else { 0.34 }, 5);
    }

    // Symbolic jump seams/knots.
    for node in &session.nodes {
        if let NodeKind::Jump { target, count } = node.kind {
            let from = geom.node_pos[node.id];
            if let Some(target_idx) = session.nodes.iter().position(|n| n.id == target) {
                let to = geom.node_pos[target_idx];
                draw_curve(img, from, to, seam_col, if terminal_mode { 0.26 } else { 0.58 }, 8);
                draw_knot(img, from, count, seam_col, terminal_mode);
            }
        }
    }
}

fn draw_curve(img: &mut Image, a: Pt, b: Pt, rgb: [u8; 3], alpha: f32, thickness: i32) {
    let mx = (a.x + b.x) * 0.5;
    let my = (a.y + b.y) * 0.5;
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let len = (dx * dx + dy * dy).sqrt().max(1.0);
    let nx = -dy / len;
    let ny = dx / len;
    let c = Pt { x: mx + nx * len * 0.18, y: my + ny * len * 0.18 };
    let steps = 44;
    let mut prev = a;
    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        let p = quad_bezier(a, c, b, t);
        draw_thick_line_img(img, prev, p, rgb, alpha, thickness);
        prev = p;
    }
}

fn quad_bezier(a: Pt, c: Pt, b: Pt, t: f64) -> Pt {
    let u = 1.0 - t;
    Pt { x: u * u * a.x + 2.0 * u * t * c.x + t * t * b.x,
         y: u * u * a.y + 2.0 * u * t * c.y + t * t * b.y }
}

fn draw_knot(img: &mut Image, p: Pt, count: usize, rgb: [u8; 3], terminal_mode: bool) {
    let loops = count.clamp(1, 9);
    for k in 0..loops {
        let r = 10.0 + k as f64 * 5.5;
        let alpha = if terminal_mode { 0.17 } else { 0.44 };
        let steps = 56;
        let mut prev = Pt { x: p.x + r, y: p.y };
        for i in 1..=steps {
            let a = TAU * i as f64 / steps as f64;
            let q = Pt { x: p.x + r * a.cos(), y: p.y + r * 0.62 * a.sin() };
            draw_thick_line_img(img, prev, q, rgb, alpha, 2);
            prev = q;
        }
    }
}

fn draw_disc(img: &mut Image, cx: f64, cy: f64, r: f64, rgb: [u8; 3], alpha: f32) {
    let xmin = (cx - r).floor() as i32;
    let xmax = (cx + r).ceil() as i32;
    let ymin = (cy - r).floor() as i32;
    let ymax = (cy + r).ceil() as i32;
    for y in ymin..=ymax {
        for x in xmin..=xmax {
            let dx = x as f64 - cx;
            let dy = y as f64 - cy;
            if dx * dx + dy * dy <= r * r {
                img.blend_px(x, y, rgb, alpha);
            }
        }
    }
}

fn draw_thick_line_img(img: &mut Image, a: Pt, b: Pt, rgb: [u8; 3], alpha: f32, thickness: i32) {
    let mut x0 = a.x.round() as i32;
    let mut y0 = a.y.round() as i32;
    let x1 = b.x.round() as i32;
    let y1 = b.y.round() as i32;
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let half = thickness.max(1) / 2;
    loop {
        for oy in -half..=half {
            for ox in -half..=half {
                img.blend_px(x0 + ox, y0 + oy, rgb, alpha);
            }
        }
        if x0 == x1 && y0 == y1 { break; }
        let e2 = 2 * err;
        if e2 >= dy { err += dy; x0 += sx; }
        if e2 <= dx { err += dx; y0 += sy; }
    }
}

fn tint(rgb: [u8; 3], lift: i16) -> [u8; 3] {
    [
        (rgb[0] as i16 + lift).clamp(0, 255) as u8,
        (rgb[1] as i16 + lift).clamp(0, 255) as u8,
        (rgb[2] as i16 + lift).clamp(0, 255) as u8,
    ]
}

fn path_position(path: &[Pt], beat: f64) -> Pt {
    if path.is_empty() { return Pt { x: W as f64 * 0.5, y: H as f64 * 0.5 }; }
    if path.len() == 1 { return path[0]; }
    let i = beat.floor() as usize % path.len();
    let j = (i + 1) % path.len();
    let t = beat - beat.floor();
    let smooth = t * t * (3.0 - 2.0 * t);
    Pt { x: path[i].x * (1.0 - smooth) + path[j].x * smooth,
         y: path[i].y * (1.0 - smooth) + path[j].y * smooth }
}

fn render_mp4(session: &Session, geom: &CarpetGeometry, out: &str) -> io::Result<()> {
    let base = render_carpet(session, geom, true);
    let dark = base.darkened(0.58);
    let bright = base.brighten(1.28, 8);
    let mut child = Command::new("ffmpeg")
        .args([
            "-y", "-f", "rawvideo", "-pix_fmt", "rgb24",
            "-s", "1280x720", "-r", &FPS.to_string(), "-i", "-",
            "-an", "-c:v", "libx264", "-pix_fmt", "yuv420p",
            "-profile:v", "baseline", "-level", "3.1", "-movflags", "+faststart",
            out,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    let beats_per_sec = session.bpm.max(20.0) / 60.0;
    let frames = FPS * TOUR_SECS;
    {
        let stdin = child.stdin.as_mut().ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "no ffmpeg stdin"))?;
        for f in 0..frames {
            let t = f as f64 / FPS as f64;
            let beat = t * beats_per_sec;
            let p = path_position(&geom.reading_path, beat);
            let phase = beat - beat.floor();
            let pulse = (-10.0 * phase).exp() as f32;
            let mut frame = dark.clone();
            // The highlight is a bright re-draw, not a ring or pointer.
            frame.redraw_from(&bright, p.x, p.y, 62.0 + pulse as f64 * 10.0, 0.82);
            // Faint memory trail, also bright re-draw.
            for k in 1..9 {
                let q = path_position(&geom.reading_path, beat - k as f64 * 0.55);
                frame.redraw_from(&bright, q.x, q.y, 34.0, 0.12 * (1.0 - k as f32 / 10.0));
            }
            stdin.write_all(&frame.rgb)?;
        }
    }

    let status = child.wait()?;
    if !status.success() {
        return Err(io::Error::new(io::ErrorKind::Other, "ffmpeg failed while writing carpet tour"));
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: {} <session.mq> <out.ppm|out.mp4> [--art]", args[0]);
        eprintln!("  PPM stills are dependency-free; convert to PNG with ffmpeg if desired.");
        return Ok(());
    }
    let input = &args[1];
    let output = &args[2];
    let art_mode = args.iter().any(|a| a == "--art");
    let src = fs::read_to_string(input)?;
    let session = parse_session(&src);
    let geom = build_geometry(&session);

    if output.ends_with(".mp4") {
        render_mp4(&session, &geom, output)?;
    } else {
        let carpet = render_carpet(&session, &geom, !art_mode);
        carpet.write_ppm(output)?;
        if output.ends_with(".png") {
            eprintln!("warning: wrote PPM bytes to {output}; use .ppm or convert via ffmpeg for true PNG");
        }
    }

    println!("saved {output}");
    println!("nodes:{} bpm:{:.0} sustain:{:.2}s", session.nodes.len(), session.bpm, session.sustain);
    if Path::new(output).extension().and_then(|e| e.to_str()) == Some("ppm") {
        println!("convert: ffmpeg -y -i {output} {}.png", output.trim_end_matches(".ppm"));
    }
    Ok(())
}
