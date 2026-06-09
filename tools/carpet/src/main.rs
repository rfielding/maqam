use anyhow::Result;
use clap::Parser;
use image::{Rgba, RgbaImage};
use imageproc::drawing::{draw_filled_circle_mut, draw_line_segment_mut};
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
    fn sub(self, o: Pt) -> Pt { Pt::new(self.x - o.x, self.y - o.y) }
    fn mul(self, k: f64) -> Pt { Pt::new(self.x * k, self.y * k) }
    fn len(self) -> f64 { (self.x * self.x + self.y * self.y).sqrt() }
    fn norm(self) -> Pt { let l = self.len(); if l < 1e-9 { Pt::new(1.0, 0.0) } else { self.mul(1.0 / l) } }
}

#[derive(Clone)]
struct Voice { scale: String, rhythm: String }
#[derive(Clone)]
struct Phrase { id: i32, voices: Vec<Voice> }
struct Score { name: String, phrases: Vec<Phrase>, scales: HashMap<String, Vec<String>> }

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
        _ => (120,110,90),
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
        for ch in s.chars() {
            match ch { 'A' => out.push_str("A-B--B+A++AA+B-"), 'B' => out.push_str("+A-BB--B-A++A+B"), _ => out.push(ch) }
        }
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
    let mut phrases = Vec::new();
    let mut scales = HashMap::new();
    for raw in text.lines().map(str::trim) {
        if raw.is_empty() || raw == "MAQAM_SESSION_V3" { continue; }
        if raw.starts_with("create ") {
            let t: Vec<&str> = raw.split_whitespace().collect();
            if t.len() > 2 { scales.insert(t[1].to_lowercase(), t[2..].iter().map(|x| x.to_string()).collect()); }
            continue;
        }
        let p: Vec<&str> = raw.split('|').collect();
        if p.len() < 3 { continue; }
        if p[0] == "P" {
            let id = p[1].parse().unwrap_or(0);
            let payload = if p.len() > 3 { p[3..].join("|") } else { String::new() };
            phrases.push(Phrase { id, voices: payload.split(',').map(parse_voice).collect() });
        }
    }
    phrases.sort_by_key(|p| p.id);
    Score { name: name.to_string(), phrases, scales }
}
fn ratio_text(score: &Score, scale: &str) -> String {
    score.scales.get(scale).cloned().unwrap_or_default().into_iter().filter(|r| r != "1/1").collect::<Vec<_>>().join(" ")
}

fn draw_label_box(img: &mut RgbaImage, x: i32, y: i32, scale: &str, ratio: &str) {
    let w = ((scale.len().max(ratio.len()) as i32) * 9 + 34).max(88);
    let h = 60;
    let x0 = (x - w / 2).clamp(30, img.width() as i32 - w - 30);
    let y0 = (y - h / 2).clamp(30, img.height() as i32 - h - 30);
    for yy in y0..y0+h { for xx in x0..x0+w { if xx >= 0 && yy >= 0 && xx < img.width() as i32 && yy < img.height() as i32 { img.put_pixel(xx as u32, yy as u32, rgba((0,0,0),255)); } } }
    for i in 0..4 {
        let c = rgba((55,55,55),255);
        draw_line_segment_mut(img, ((x0+i) as f32, (y0+i) as f32), ((x0+w-i) as f32, (y0+i) as f32), c);
        draw_line_segment_mut(img, ((x0+i) as f32, (y0+h-i) as f32), ((x0+w-i) as f32, (y0+h-i) as f32), c);
    }
    // Text drawing is intentionally left to the Python checkpoint for now.
    let _ = (scale, ratio);
}

fn draw_arc(img: &mut RgbaImage, cx: f64, cy: f64, rr: f64, a0: f64, a1: f64, color: (u8,u8,u8), width: f64) {
    let mut pts = Vec::new();
    for i in 0..80 { let a = a0 + (a1 - a0) * (i as f64) / 79.0; pts.push(Pt::new(cx + rr * a.cos(), cy + rr * a.sin())); }
    thread(img, &pts, color, width, 255);
}

fn render(score: &Score, w: u32, h: u32) -> RgbaImage {
    let mut img = RgbaImage::from_pixel(w, h, rgba((8,10,12),255));
    for y in (0..h).step_by(6) { for x in (0..w).step_by(6) { if noise!(&score.name, "field", x/6, y/6) > 0.18 { img.put_pixel(x, y, rgba((20,18,16),255)); } } }
    let rect = (w as f64 * 0.18, h as f64 * 0.20, w as f64 * 0.82, h as f64 * 0.80);
    let g = fit(&gosper(3), rect, 28.0);
    thread(&mut img, &g, (8,8,8), 11.0, 255);
    let cx = (rect.0 + rect.2) / 2.0;
    let cy = (rect.1 + rect.3) / 2.0;
    let rr = ((rect.2 - rect.0).min(rect.3 - rect.1)) * 0.60;
    let total = score.phrases.len().max(1);
    for (i, p) in score.phrases.iter().enumerate() {
        let a0 = -PI / 2.0 + 2.0 * PI * (i as f64) / (total as f64);
        let a1 = -PI / 2.0 + 2.0 * PI * ((i + 1) as f64) / (total as f64) - 0.12;
        let scale = p.voices.first().map(|v| v.scale.as_str()).unwrap_or("rast");
        draw_arc(&mut img, cx, cy, rr, a0, a1, (96,76,44), 14.0);
        for k in 0..8 {
            let a = a0 + (a1 - a0) * (k as f64 + 0.5) / 8.0;
            draw_filled_circle_mut(&mut img, ((cx + (rr+35.0)*a.cos()) as i32, (cy + (rr+35.0)*a.sin()) as i32), if k == 0 { 18 } else { 9 }, rgba(maqam_color(scale),255));
        }
        let mid = (a0 + a1) / 2.0;
        draw_label_box(&mut img, (cx + (rr+175.0)*mid.cos()) as i32, (cy + (rr+175.0)*mid.sin()) as i32, &scale.to_uppercase(), &ratio_text(score, scale));
    }
    img
}

fn carpet(design: &RgbaImage, kw: u32, kh: u32, kp: u32) -> RgbaImage {
    let mut out = RgbaImage::from_pixel(kw * kp, kh * kp, rgba((12,11,10),255));
    for y in 0..kh { for x in 0..kw {
        let sx = x * (design.width() - 1) / (kw - 1).max(1);
        let sy = y * (design.height() - 1) / (kh - 1).max(1);
        let p = *design.get_pixel(sx, sy);
        for yy in 0..kp { for xx in 0..kp { out.put_pixel(x*kp+xx, y*kp+yy, p); } }
    } }
    out
}
fn build(name: &str, text: &str, out: &PathBuf, args: &Args) -> Result<()> {
    let score = parse(name, text);
    let design = render(&score, args.w, args.h);
    let img = carpet(&design, args.knots_w, args.knots_h, args.knot_px);
    img.save(out)?;
    Ok(())
}
fn main() -> Result<()> {
    let args = Args::parse();
    if args.all || args.mq.is_none() {
        build("magiccarpet.mq", MAGICCARPET, &PathBuf::from("magiccarpet_rust.png"), &args)?;
        build("growl.mq", GROWL, &PathBuf::from("growl_rust.png"), &args)?;
    } else {
        let text = fs::read_to_string(args.mq.as_ref().unwrap())?;
        build(&args.name, &text, args.out.as_ref().unwrap(), &args)?;
    }
    Ok(())
}
