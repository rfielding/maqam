use anyhow::Result;
use clap::Parser;
use image::{Rgba, RgbaImage};
use imageproc::drawing::{draw_filled_circle_mut, draw_line_segment_mut, draw_polygon_mut};
use imageproc::point::Point;
use std::{collections::HashMap, f64::consts::PI, fs, path::PathBuf};

#[derive(Parser, Debug)]
struct Args {
    #[arg(long)] mq: Option<PathBuf>,
    #[arg(long)] out: Option<PathBuf>,
    #[arg(long, default_value = "score.mq")] name: String,
    #[arg(long)] all: bool,
    #[arg(long, default_value_t = 1800)] w: u32,
    #[arg(long, default_value_t = 900)] h: u32,
    #[arg(long, default_value_t = 1440)] knots_w: u32,
    #[arg(long, default_value_t = 720)] knots_h: u32,
    #[arg(long, default_value_t = 2)] knot_px: u32,
    #[arg(long, default_value_t = 40)] fringe_top: u32,
    #[arg(long, default_value_t = 80)] fringe_side: u32,
}

const MAGICCARPET: &str = r#"MAQAM_SESSION_V3
create saba2 1/1 13/12 6/5 5/4
vol 1
B|0|180
S|1|2
P|2|1|d bayati, f hijaz 4444
J|3|0|3
P|4|1|a saba, c hijaz
P|5|1|a saba2, c hijaz
J|6|4|4
P|7|1|g rast 664664
J|8|7|4
J|9|0|4
"#;

const GROWL: &str = r#"MAQAM_SESSION_V3
create bayati2 8/9 1/1 12/11 32/27 4/3 3/2
create hijaz2 8/9 1/1 15/14 6/5 4/3 3/2
create saba2 8/9 1/1 13/12 6/5 5/4 11/8 3/2
vol 1
B|0|180
S|1|1.2
P|2|1|g hijaz 4444
J|3|2|3
P|4|1|g bayati 332332
P|5|1|g saba 664
J|6|5|4
J|7|0|4
"#;

#[derive(Clone, Copy)]
struct Pt { x: f64, y: f64 }
impl Pt {
    fn new(x: f64, y: f64) -> Self { Self { x, y } }
    fn add(self, o: Pt) -> Pt { Pt::new(self.x + o.x, self.y + o.y) }
    fn sub(self, o: Pt) -> Pt { Pt::new(self.x - o.x, self.y - o.y) }
    fn mul(self, k: f64) -> Pt { Pt::new(self.x * k, self.y * k) }
    fn len(self) -> f64 { (self.x * self.x + self.y * self.y).sqrt() }
    fn norm(self) -> Pt { let l = self.len(); if l < 1e-9 { Pt::new(1.0, 0.0) } else { self.mul(1.0 / l) } }
}

#[derive(Clone)]
struct Voice { scale: String, rhythm: String }
#[derive(Clone)]
struct Phrase { id: i32, voices: Vec<Voice> }
#[derive(Clone)]
struct Jump { id: i32, target: i32 }
struct Score { name: String, phrases: Vec<Phrase>, jumps: Vec<Jump>, scales: HashMap<String, Vec<String>> }

fn rgba(c: (u8, u8, u8), a: u8) -> Rgba<u8> { Rgba([c.0, c.1, c.2, a]) }
fn mix(a: (u8,u8,u8), b: (u8,u8,u8), t: f64) -> (u8,u8,u8) {
    let f = |x: u8, y: u8| ((1.0 - t) * x as f64 + t * y as f64).round().clamp(0.0, 255.0) as u8;
    (f(a.0,b.0), f(a.1,b.1), f(a.2,b.2))
}
fn light(c: (u8,u8,u8), t: f64) -> (u8,u8,u8) { mix(c, (255,255,255), t) }
fn dark(c: (u8,u8,u8), t: f64) -> (u8,u8,u8) { mix(c, (0,0,0), t) }
fn base(s: &str) -> String { s.trim().to_lowercase().trim_end_matches(|c: char| c.is_ascii_digit()).to_string() }
fn maqam_color(s: &str) -> (u8,u8,u8) {
    match base(s).as_str() {
        "bayati" => (92,170,128), "hijaz" => (176,92,58), "saba" => (126,88,164),
        "rast" => (178,145,68), "ajam" => (180,152,84), "kurd" => (86,118,98),
        "nahawand" => (76,130,178), "zaba" => (150,96,144), _ => (120,110,90),
    }
}

fn hash(parts: &[String]) -> u64 {
    let mut h = 0xcbf29ce484222325_u64;
    for s in parts {
        for b in s.as_bytes() { h ^= *b as u64; h = h.wrapping_mul(0x100000001b3); }
        h ^= 0xff; h = h.wrapping_mul(0x100000001b3);
    }
    h
}
macro_rules! noise { ($($x:expr),* $(,)?) => {{ let v = vec![$($x.to_string()),*]; ((hash(&v) >> 11) as f64) / ((1_u64 << 53) as f64) }}; }

fn thick_line(img: &mut RgbaImage, a: Pt, b: Pt, color: (u8,u8,u8), width: f64, alpha: u8) {
    let d = b.sub(a);
    let n = Pt::new(-d.y, d.x).norm();
    let half = (width / 2.0).ceil() as i32;
    for k in -half..=half {
        let o = n.mul(k as f64);
        draw_line_segment_mut(img, ((a.x + o.x) as f32, (a.y + o.y) as f32), ((b.x + o.x) as f32, (b.y + o.y) as f32), rgba(color, alpha));
    }
}
fn thread(img: &mut RgbaImage, pts: &[Pt], color: (u8,u8,u8), width: f64, alpha: u8) {
    for p in pts.windows(2) {
        thick_line(img, p[0], p[1], dark(color, 0.7), width + 4.0, alpha);
        thick_line(img, p[0], p[1], color, width, alpha);
    }
}

fn gosper(order: usize) -> Vec<Pt> {
    let mut s = "A".to_string();
    for _ in 0..order {
        let mut out = String::new();
        for ch in s.chars() { match ch { 'A' => out.push_str("A-B--B+A++AA+B-"), 'B' => out.push_str("+A-BB--B-A++A+B"), _ => out.push(ch) } }
        s = out;
    }
    let (mut x, mut y, mut heading): (f64, f64, f64) = (0.0, 0.0, 0.0);
    let mut pts = vec![Pt::new(x, y)];
    for ch in s.chars() {
        match ch {
            'A' | 'B' => { x += heading.cos(); y += heading.sin(); pts.push(Pt::new(x, y)); }
            '+' => heading += PI / 3.0,
            '-' => heading -= PI / 3.0,
            _ => {}
        }
    }
    pts
}
fn fit(pts: &[Pt], rect: (f64,f64,f64,f64), margin: f64) -> Vec<Pt> {
    let min_x = pts.iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
    let max_x = pts.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max);
    let min_y = pts.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
    let max_y = pts.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max);
    let sx = (rect.2 - rect.0 - 2.0 * margin) / (max_x - min_x).max(1e-9);
    let sy = (rect.3 - rect.1 - 2.0 * margin) / (max_y - min_y).max(1e-9);
    let sc = sx.min(sy);
    let ox = rect.0 + (rect.2 - rect.0 - (max_x - min_x) * sc) / 2.0 - min_x * sc;
    let oy = rect.1 + (rect.3 - rect.1 - (max_y - min_y) * sc) / 2.0 - min_y * sc;
    pts.iter().map(|p| Pt::new(ox + p.x * sc, oy + p.y * sc)).collect()
}

fn parse_voice(s: &str) -> Voice {
    let mut toks: Vec<&str> = s.split_whitespace().collect();
    let mut rhythm = String::new();
    if let Some(last) = toks.last().copied() {
        if last.chars().all(|c| c.is_ascii_digit() || "xX._-".contains(c)) { rhythm = last.to_string(); toks.pop(); }
    }
    Voice { scale: toks.get(1).copied().unwrap_or("rast").to_lowercase(), rhythm }
}
fn parse(name: &str, text: &str) -> Score {
    let mut phrases = Vec::new(); let mut jumps = Vec::new(); let mut scales = HashMap::new();
    for raw in text.lines().map(str::trim) {
        if raw.is_empty() || raw == "MAQAM_SESSION_V3" { continue; }
        if raw.starts_with("create ") { let t: Vec<&str> = raw.split_whitespace().collect(); if t.len() > 2 { scales.insert(t[1].to_lowercase(), t[2..].iter().map(|x| x.to_string()).collect()); } continue; }
        let p: Vec<&str> = raw.split('|').collect(); if p.len() < 3 { continue; }
        match p[0] {
            "P" => { let id = p[1].parse().unwrap_or(0); let payload = if p.len() > 3 { p[3..].join("|") } else { String::new() }; phrases.push(Phrase { id, voices: payload.split(',').map(parse_voice).collect() }); }
            "J" => jumps.push(Jump { id: p[1].parse().unwrap_or(0), target: p[2].parse().unwrap_or(0) }),
            _ => {}
        }
    }
    phrases.sort_by_key(|p| p.id); jumps.sort_by_key(|j| j.id);
    Score { name: name.to_string(), phrases, jumps, scales }
}
fn default_ratios(scale: &str) -> Vec<String> {
    match base(scale).as_str() {
        "bayati" => "1/1 12/11 32/27 4/3 3/2", "hijaz" => "1/1 256/243 81/64 4/3 3/2",
        "saba" => "1/1 13/12 32/27 5/4", "rast" => "1/1 9/8 27/22 4/3 3/2", _ => "",
    }.split_whitespace().map(|s| s.to_string()).collect()
}
fn ratio_text(score: &Score, scale: &str) -> String {
    let b = base(scale);
    let ratios = score.scales.get(scale).or_else(|| score.scales.get(&b)).cloned().unwrap_or_else(|| default_ratios(scale));
    ratios.into_iter().filter(|r| r != "1/1").collect::<Vec<_>>().join(" ")
}

fn glyph(c: char) -> [u8; 7] {
    match c.to_ascii_uppercase() {
        'A' => [14,17,17,31,17,17,17], 'B' => [30,17,17,30,17,17,30], 'C' => [14,17,16,16,16,17,14], 'D' => [30,17,17,17,17,17,30],
        'E' => [31,16,16,30,16,16,31], 'F' => [31,16,16,30,16,16,16], 'G' => [14,17,16,23,17,17,14], 'H' => [17,17,17,31,17,17,17],
        'I' => [14,4,4,4,4,4,14], 'J' => [7,2,2,2,18,18,12], 'K' => [17,18,20,24,20,18,17], 'L' => [16,16,16,16,16,16,31],
        'M' => [17,27,21,21,17,17,17], 'N' => [17,25,21,19,17,17,17], 'O' => [14,17,17,17,17,17,14], 'P' => [30,17,17,30,16,16,16],
        'Q' => [14,17,17,17,21,18,13], 'R' => [30,17,17,30,20,18,17], 'S' => [15,16,16,14,1,1,30], 'T' => [31,4,4,4,4,4,4],
        'U' => [17,17,17,17,17,17,14], 'V' => [17,17,17,17,17,10,4], 'W' => [17,17,17,21,21,21,10], 'X' => [17,17,10,4,10,17,17],
        'Y' => [17,17,10,4,4,4,4], 'Z' => [31,1,2,4,8,16,31], '0' => [14,17,19,21,25,17,14], '1' => [4,12,4,4,4,4,14],
        '2' => [14,17,1,2,4,8,31], '3' => [30,1,1,14,1,1,30], '4' => [2,6,10,18,31,2,2], '5' => [31,16,16,30,1,1,30],
        '6' => [14,16,16,30,17,17,14], '7' => [31,1,2,4,8,8,8], '8' => [14,17,17,14,17,17,14], '9' => [14,17,17,15,1,1,14],
        '/' => [1,1,2,4,8,16,16], '-' => [0,0,0,31,0,0,0], _ => [0,0,0,0,0,0,0],
    }
}
fn draw_text(img: &mut RgbaImage, text: &str, x: i32, y: i32, scale: i32, color: Rgba<u8>) {
    let mut xx = x;
    for ch in text.chars() { let rows = glyph(ch); for (yy, row) in rows.iter().enumerate() { for bit in 0_i32..5_i32 { if (row >> ((4 - bit) as u32)) & 1 == 1 { for dy in 0..scale { for dx in 0..scale { let px = xx + bit * scale + dx; let py = y + yy as i32 * scale + dy; if px >= 0 && py >= 0 && px < img.width() as i32 && py < img.height() as i32 { img.put_pixel(px as u32, py as u32, color); } } } } } } xx += 6 * scale; }
}
fn draw_label_box(img: &mut RgbaImage, x: i32, y: i32, scale: &str, ratio: &str) {
    let text_w = ((scale.len().max(ratio.len()) as i32) * 6 * 3).max(90); let w = text_w + 36; let h = 76;
    let x0 = (x - w / 2).clamp(30, img.width() as i32 - w - 30); let y0 = (y - h / 2).clamp(30, img.height() as i32 - h - 30);
    for yy in y0..y0+h { for xx in x0..x0+w { if xx >= 0 && yy >= 0 && xx < img.width() as i32 && yy < img.height() as i32 { img.put_pixel(xx as u32, yy as u32, rgba((0,0,0),255)); } } }
    for i in 0..5 { let c = rgba((55,55,55),255); draw_line_segment_mut(img, ((x0+i) as f32, (y0+i) as f32), ((x0+w-i) as f32, (y0+i) as f32), c); draw_line_segment_mut(img, ((x0+i) as f32, (y0+h-i) as f32), ((x0+w-i) as f32, (y0+h-i) as f32), c); draw_line_segment_mut(img, ((x0+i) as f32, (y0+i) as f32), ((x0+i) as f32, (y0+h-i) as f32), c); draw_line_segment_mut(img, ((x0+w-i) as f32, (y0+i) as f32), ((x0+w-i) as f32, (y0+h-i) as f32), c); }
    draw_text(img, scale, x0 + 18, y0 + 12, 3, rgba((230,230,230),255)); draw_text(img, ratio, x0 + 18, y0 + 48, 2, rgba((205,205,205),255));
}

#[derive(Clone)]
struct Group { phrase_id: i32, start: f64, end: f64, angles: Vec<f64>, kinds: Vec<bool> }
fn groups(score: &Score) -> Vec<Group> {
    let total_ticks: usize = score.phrases.iter().map(|p| { let rhythm = p.voices.iter().rev().find(|v| !v.rhythm.is_empty()).map(|v| v.rhythm.as_str()).unwrap_or("4"); rhythm.chars().filter_map(|c| c.to_digit(10)).map(|d| d as usize).sum::<usize>().max(1) }).sum::<usize>().max(1);
    let gap = 10.0_f64.to_radians(); let step = (2.0 * PI - score.phrases.len() as f64 * gap) / total_ticks as f64; let mut a = -PI / 2.0; let mut out = Vec::new();
    for p in &score.phrases { let rhythm = p.voices.iter().rev().find(|v| !v.rhythm.is_empty()).map(|v| v.rhythm.as_str()).unwrap_or("4"); let mut angles = Vec::new(); let mut kinds = Vec::new(); let start = a; for ch in rhythm.chars().filter_map(|c| c.to_digit(10)) { angles.push(a); kinds.push(true); a += step; for _ in 1..ch { angles.push(a); kinds.push(false); a += step; } } if angles.is_empty() { angles.push(a); kinds.push(true); a += step; } let end = *angles.last().unwrap_or(&start); out.push(Group { phrase_id: p.id, start, end, angles, kinds }); a += gap; }
    out
}
fn draw_arc(img: &mut RgbaImage, cx: f64, cy: f64, rr: f64, a0: f64, a1: f64, color: (u8,u8,u8), width: f64) { let mut pts = Vec::new(); for i in 0..96 { let a = a0 + (a1 - a0) * (i as f64) / 95.0; pts.push(Pt::new(cx + rr * a.cos(), cy + rr * a.sin())); } thread(img, &pts, color, width, 255); }
fn draw_chevrons(img: &mut RgbaImage, pts: &[Pt], color: (u8,u8,u8), width: f64) { let mut dist = 0.0; for p in pts.windows(2) { let seg = p[1].sub(p[0]); let len = seg.len(); dist += len; if dist > 38.0 { dist = 0.0; let t = seg.norm(); let n = Pt::new(-t.y, t.x); let mid = p[0].add(p[1]).mul(0.5); thick_line(img, mid.sub(t.mul(9.0)).add(n.mul(7.0)), mid.add(t.mul(9.0)), color, width, 255); thick_line(img, mid.sub(t.mul(9.0)).sub(n.mul(7.0)), mid.add(t.mul(9.0)), color, width, 255); } } }
fn draw_jumps(img: &mut RgbaImage, score: &Score, cx: f64, cy: f64, rr: f64, gs: &[Group]) { for j in &score.jumps { let src = gs.iter().filter(|g| g.phrase_id < j.id).last().or_else(|| gs.last()); let dst = gs.iter().find(|g| g.phrase_id >= j.target).or_else(|| gs.first()); let (Some(src), Some(dst)) = (src, dst) else { continue; }; let a0 = src.end; let a1 = dst.start; let p0 = Pt::new(cx + (rr - 20.0) * a0.cos(), cy + (rr - 20.0) * a0.sin()); let p3 = Pt::new(cx + (rr - 20.0) * a1.cos(), cy + (rr - 20.0) * a1.sin()); let c1 = Pt::new(cx + (rr * 0.25) * a0.cos(), cy + (rr * 0.25) * a0.sin()); let c2 = Pt::new(cx + (rr * 0.25) * a1.cos(), cy + (rr * 0.25) * a1.sin()); let mut pts = Vec::new(); for i in 0..140 { let t = i as f64 / 139.0; let u = 1.0 - t; pts.push(p0.mul(u*u*u).add(c1.mul(3.0*u*u*t)).add(c2.mul(3.0*u*t*t)).add(p3.mul(t*t*t))); } thread(img, &pts, (8,8,8), 10.0, 190); draw_chevrons(img, &pts, (245,245,245), 3.0); } }

fn draw_micro_weave(img: &mut RgbaImage, w: u32, h: u32, name: &str) {
    for y in (0..h).step_by(9) { let c = if y % 18 == 0 { (24,20,18) } else { (14,18,18) }; draw_line_segment_mut(img, (0.0, y as f32), (w as f32, y as f32), rgba(c, 80)); }
    for x in (0..w).step_by(13) { let c = if x % 26 == 0 { (20,18,24) } else { (15,17,14) }; draw_line_segment_mut(img, (x as f32, 0.0), (x as f32, h as f32), rgba(c, 45)); }
    for _i in 0..700 { let x = (noise!(name, "speckx", _i) * w as f64) as i32; let y = (noise!(name, "specky", _i) * h as f64) as i32; let c = if noise!(name, "speckc", _i) > 0.5 { (43,34,23) } else { (18,28,24) }; draw_filled_circle_mut(img, (x,y), 1, rgba(c, 150)); }
}
fn draw_sector_tendrils(img: &mut RgbaImage, cx: f64, cy: f64, rr: f64, g: &Group, scale: &str, idx: usize) {
    let col = dark(maqam_color(scale), 0.25); let span = (g.end - g.start).max(0.01);
    for t in 0..7 { let a = g.start + span * (t as f64 + 0.5) / 7.0; let mut pts = Vec::new(); for k in 0..10 { let r = rr + 62.0 + k as f64 * 46.0; let wig = (noise!("tendril", idx, t, k) - 0.5) * 0.035; let aa = a + wig; pts.push(Pt::new(cx + r * aa.cos(), cy + r * aa.sin())); } thread(img, &pts, col, 3.0, 120); }
}

fn render(score: &Score, w: u32, h: u32) -> RgbaImage {
    let mut img = RgbaImage::from_pixel(w, h, rgba((8,10,12),255));
    for y in (0..h).step_by(5) { for x in (0..w).step_by(5) { let v = noise!(&score.name, "field", x / 5, y / 5); let c = if v > 0.70 { (25,22,20) } else if v > 0.45 { (18,23,22) } else { (16,15,14) }; img.put_pixel(x, y, rgba(c,255)); } }
    draw_micro_weave(&mut img, w, h, &score.name);
    let rect = (w as f64 * 0.18, h as f64 * 0.20, w as f64 * 0.82, h as f64 * 0.80); let cx = (rect.0 + rect.2) / 2.0; let cy = (rect.1 + rect.3) / 2.0; let rr = ((rect.2 - rect.0).min(rect.3 - rect.1)) * 0.60;
    let center = fit(&gosper(3), rect, 28.0); thread(&mut img, &center, (8,8,8), 12.0, 255); draw_chevrons(&mut img, &center, (90,72,42), 2.0);
    let gs = groups(score); let phrase_map: HashMap<i32, &Phrase> = score.phrases.iter().map(|p| (p.id, p)).collect();
    for (gi, g) in gs.iter().enumerate() { let Some(p) = phrase_map.get(&g.phrase_id) else { continue; }; let scale = p.voices.first().map(|v| v.scale.as_str()).unwrap_or("rast"); let col = maqam_color(scale); draw_sector_tendrils(&mut img, cx, cy, rr, g, scale, gi); draw_arc(&mut img, cx, cy, rr, g.start, g.end, (96,76,44), 18.0); draw_arc(&mut img, cx, cy, rr - 18.0, g.start, g.end, (26,21,16), 4.0);
        for ring in 0..7 { let r2 = rr + 58.0 + ring as f64 * 50.0; let count = 5 + ring * 2; for k in 0..count { let a = g.start + (g.end - g.start) * (k as f64 + 0.5) / count as f64; let jitter = (noise!("motif", &score.name, gi, ring, k) - 0.5) * 18.0; let x = cx + (r2 + jitter) * a.cos(); let y = cy + (r2 + jitter) * a.sin(); let s = if ring % 2 == 0 { 7 } else { 5 }; draw_filled_circle_mut(&mut img, (x as i32, y as i32), s + 5, rgba(dark(col,0.55),180)); draw_filled_circle_mut(&mut img, (x as i32, y as i32), s, rgba(light(col,0.10),215)); } }
        for (a, is_kick) in g.angles.iter().zip(g.kinds.iter()) { let rdot = if *is_kick { 20 } else { 10 }; let x = cx + (rr + 35.0) * a.cos(); let y = cy + (rr + 35.0) * a.sin(); draw_filled_circle_mut(&mut img, (x as i32, y as i32), rdot + 5, rgba((18,15,12),255)); draw_filled_circle_mut(&mut img, (x as i32, y as i32), rdot, rgba(col,255)); }
        let mid = (g.start + g.end) / 2.0; draw_label_box(&mut img, (cx + (rr + 176.0) * mid.cos()) as i32, (cy + (rr + 176.0) * mid.sin()) as i32, &scale.to_uppercase(), &ratio_text(score, scale)); }
    draw_jumps(&mut img, score, cx, cy, rr, &gs); img
}

fn carpet(design: &RgbaImage, kw: u32, kh: u32, kp: u32) -> RgbaImage {
    let mut out = RgbaImage::from_pixel(kw * kp, kh * kp, rgba((12,11,10),255));
    for y in 0..kh { for x in 0..kw { let sx = x * (design.width() - 1) / (kw - 1).max(1); let sy = y * (design.height() - 1) / (kh - 1).max(1); let mut p = *design.get_pixel(sx, sy); let v = (noise!("knot", x, y) - 0.5) * 18.0; p[0] = (p[0] as f64 + v).round().clamp(0.0, 255.0) as u8; p[1] = (p[1] as f64 + v).round().clamp(0.0, 255.0) as u8; p[2] = (p[2] as f64 + v).round().clamp(0.0, 255.0) as u8; let cx = (x*kp + kp/2) as i32; let cy = (y*kp + kp/2) as i32; let px = kp as i32; let poly = [Point::new(cx, cy - px/2), Point::new(cx + px/2, cy), Point::new(cx, cy + px/2), Point::new(cx - px/2, cy)]; draw_polygon_mut(&mut out, &poly, p); } }
    out
}
fn add_fringes(rug: &RgbaImage, top: u32, side: u32) -> RgbaImage { let mut out = RgbaImage::from_pixel(rug.width() + side*2, rug.height() + top*2, rgba((9,8,7),255)); image::imageops::overlay(&mut out, rug, side.into(), top.into()); for x in (side+8..side+rug.width()-8).step_by(6) { let i = x - side; let len = (24.0 + noise!("fringe", i) * 32.0) as i32; let bend = ((noise!("fringe-b", i) - 0.5) * 28.0) as i32; draw_line_segment_mut(&mut out, (x as f32, top as f32), ((x as i32 + bend) as f32, (top as i32 - len) as f32), rgba((116,100,76),210)); draw_line_segment_mut(&mut out, (x as f32, (top + rug.height()) as f32), ((x as i32 - bend) as f32, (top as i32 + rug.height() as i32 + len) as f32), rgba((116,100,76),210)); } out }

fn build(name: &str, text: &str, out: &PathBuf, args: &Args) -> Result<()> { let score = parse(name, text); let design = render(&score, args.w, args.h); let rug = carpet(&design, args.knots_w, args.knots_h, args.knot_px); let img = add_fringes(&rug, args.fringe_top, args.fringe_side); img.save(out)?; Ok(()) }
fn main() -> Result<()> { let args = Args::parse(); if args.all || args.mq.is_none() { build("magiccarpet.mq", MAGICCARPET, &PathBuf::from("magiccarpet_rust.png"), &args)?; build("growl.mq", GROWL, &PathBuf::from("growl_rust.png"), &args)?; } else { let text = fs::read_to_string(args.mq.as_ref().unwrap())?; build(&args.name, &text, args.out.as_ref().unwrap(), &args)?; } Ok(()) }
