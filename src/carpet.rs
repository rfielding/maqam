#![allow(dead_code)]
// carpet.rs - static carpet background for maqam-live MP4 rendering.
// This module owns the visual score background used by record_old.rs.

use std::collections::HashMap;
use std::f32::consts::TAU;
use std::io::Write;

use crate::sequencer::Phrase;

pub const CARPET_H: usize = 720;
pub const CARPET_START_X: usize = 1180;
pub const CARPET_STEP_X: usize = 30;
const W: usize = 1280;
const CX: f32 = 640.0;
const CY: f32 = 360.0;
const INNER_R: f32 = 175.0;
const BEAT_R: f32 = 214.0;

#[derive(Clone, Copy, Debug)]
pub struct CarpetEntry {
    pub phrase_idx: usize,
    pub bpm: f64,
    pub sustain: f64,
}

#[derive(Clone, Debug)]
pub struct CarpetRenderInfo {
    pub path: String,
    pub width: usize,
    pub content_end_x: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WeaveTick {
    pub phrase_id: usize,
    pub group_index: usize,
    pub tick_in_group: usize,
    pub group_len: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WeavePhrase {
    pub phrase_id: usize,
    pub groups: Vec<u8>,
    pub first_tick: usize,
    pub tick_count: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WeaveScore {
    pub phrases: Vec<WeavePhrase>,
    pub ticks: Vec<WeaveTick>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BorderTickLayout {
    pub phrase_id: usize,
    pub score_tick: usize,
    pub x: f32,
    pub y: f32,
    pub is_kick: bool,
    start_t: f32,
    end_t: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JumpLinkCell {
    pub jump_id: usize,
    pub x: f32,
    pub y: f32,
    pub size: i32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct JumpRoute {
    jump_id: usize,
    source_phrase_id: usize,
    target_phrase_id: usize,
    source_t: f32,
    target_t: f32,
}

#[derive(Clone, Copy)]
struct Pt {
    x: f32,
    y: f32,
}
impl Pt {
    fn new(x: f32, y: f32) -> Self { Self { x, y } }
    fn add(self, o: Pt) -> Pt { Pt::new(self.x + o.x, self.y + o.y) }
    fn mul(self, k: f32) -> Pt { Pt::new(self.x * k, self.y * k) }
}

fn canonical_score_groups(groups: &[u8]) -> Vec<u8> {
    if groups.len() >= 6 && groups.len() % 2 == 0 {
        let half = groups.len() / 2;
        let motif = &groups[..half];
        if motif == &groups[half..]
            && motif.iter().copied().collect::<std::collections::HashSet<_>>().len() > 1
        {
            return motif.to_vec();
        }
    }
    groups.to_vec()
}

impl WeaveScore {
    pub fn from_phrases(phrases: &[Phrase]) -> Self {
        let mut score = Self::default();
        for phrase in phrases {
            if phrase.jump.is_some() || phrase.control.is_some() { continue; }
            let groups = canonical_score_groups(&phrase.bar.groups);
            let first_tick = score.ticks.len();
            for (group_index, &group_len) in groups.iter().enumerate() {
                for tick_in_group in 0..group_len as usize {
                    score.ticks.push(WeaveTick {
                        phrase_id: phrase.id,
                        group_index,
                        tick_in_group,
                        group_len: group_len as usize,
                    });
                }
            }
            score.phrases.push(WeavePhrase {
                phrase_id: phrase.id,
                groups,
                first_tick,
                tick_count: score.ticks.len() - first_tick,
            });
        }
        score
    }
}

fn clamp01(x: f32) -> f32 { x.clamp(0.0, 1.0) }
fn blend_px(buf: &mut [u8], w: usize, x: i32, y: i32, rgb: [u8; 3], a: f32) {
    if x < 0 || y < 0 || x >= w as i32 || y >= CARPET_H as i32 { return; }
    let a = clamp01(a);
    let i = (y as usize * w + x as usize) * 3;
    for c in 0..3 {
        let b = buf[i + c] as f32;
        let o = rgb[c] as f32;
        buf[i + c] = (b * (1.0 - a) + o * a).round().clamp(0.0, 255.0) as u8;
    }
}
fn put_px(buf: &mut [u8], w: usize, x: usize, y: usize, rgb: [u8; 3]) {
    if x >= w || y >= CARPET_H { return; }
    let i = (y * w + x) * 3;
    buf[i..i + 3].copy_from_slice(&rgb);
}
fn line(buf: &mut [u8], w: usize, mut x0: i32, mut y0: i32, x1: i32, y1: i32, rgb: [u8; 3], a: f32) {
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        blend_px(buf, w, x0, y0, rgb, a);
        if x0 == x1 && y0 == y1 { break; }
        let e2 = err * 2;
        if e2 >= dy { err += dy; x0 += sx; }
        if e2 <= dx { err += dx; y0 += sy; }
    }
}
fn thick_line(buf: &mut [u8], w: usize, x0: i32, y0: i32, x1: i32, y1: i32, rgb: [u8; 3], a: f32, t: i32) {
    let half = t.max(1) / 2;
    for off in -half..=half {
        line(buf, w, x0 + off, y0, x1 + off, y1, rgb, a);
        line(buf, w, x0, y0 + off, x1, y1 + off, rgb, a);
    }
}
fn dot(buf: &mut [u8], w: usize, cx: i32, cy: i32, r: i32, rgb: [u8; 3], a: f32) {
    let r = r.max(1);
    let rr = (r * r) as f32;
    for y in cy-r..=cy+r {
        for x in cx-r..=cx+r {
            let dx = (x - cx) as f32;
            let dy = (y - cy) as f32;
            let d2 = dx*dx + dy*dy;
            if d2 <= rr {
                let falloff = 0.35 + 0.65 * (1.0 - (d2 / rr).sqrt());
                blend_px(buf, w, x, y, rgb, a * falloff);
            }
        }
    }
}
fn hash(mut x: u32) -> u32 {
    x ^= x >> 16; x = x.wrapping_mul(0x7feb352d); x ^= x >> 15; x = x.wrapping_mul(0x846ca68b); x ^ (x >> 16)
}
fn hash01(x: u32) -> f32 { hash(x) as f32 / u32::MAX as f32 }
fn phrase_color(phrase_id: usize) -> [u8; 3] {
    const COLORS: [[u8;3]; 8] = [
        [88, 44, 122], [32, 110, 78], [30, 104, 132], [148, 74, 46],
        [132, 105, 42], [78, 92, 126], [118, 70, 112], [108, 78, 48],
    ];
    COLORS[phrase_id % COLORS.len()]
}
fn dark(c: [u8; 3], k: f32) -> [u8; 3] {
    [
        (c[0] as f32 * (1.0-k)).round() as u8,
        (c[1] as f32 * (1.0-k)).round() as u8,
        (c[2] as f32 * (1.0-k)).round() as u8,
    ]
}
fn light(c: [u8; 3], k: f32) -> [u8; 3] {
    [
        (c[0] as f32 * (1.0-k) + 255.0*k).round().min(255.0) as u8,
        (c[1] as f32 * (1.0-k) + 255.0*k).round().min(255.0) as u8,
        (c[2] as f32 * (1.0-k) + 255.0*k).round().min(255.0) as u8,
    ]
}

fn hilbert_rot(n: i32, x: &mut i32, y: &mut i32, rx: i32, ry: i32) {
    if ry == 0 {
        if rx == 1 { *x = n - 1 - *x; *y = n - 1 - *y; }
        std::mem::swap(x, y);
    }
}
fn hilbert_d2xy(order: u32, mut d: i32) -> Pt {
    let n = 1_i32 << order;
    let mut x = 0_i32; let mut y = 0_i32; let mut s = 1_i32;
    while s < n {
        let rx = 1 & (d / 2);
        let ry = 1 & (d ^ rx);
        hilbert_rot(s, &mut x, &mut y, rx, ry);
        x += s * rx; y += s * ry; d /= 4; s *= 2;
    }
    Pt::new(x as f32, y as f32)
}
fn hilbert_points(order: u32) -> Vec<Pt> {
    let n = 1_i32 << order;
    let mut pts = Vec::with_capacity((n*n) as usize);
    for d in 0..n*n { pts.push(hilbert_d2xy(order, d)); }
    pts
}
fn fit_points(pts: &[Pt], x0: f32, y0: f32, x1: f32, y1: f32, margin: f32) -> Vec<Pt> {
    let minx = pts.iter().map(|p|p.x).fold(f32::INFINITY, f32::min);
    let maxx = pts.iter().map(|p|p.x).fold(f32::NEG_INFINITY, f32::max);
    let miny = pts.iter().map(|p|p.y).fold(f32::INFINITY, f32::min);
    let maxy = pts.iter().map(|p|p.y).fold(f32::NEG_INFINITY, f32::max);
    let sx = (x1 - x0 - 2.0*margin) / (maxx-minx).max(0.001);
    let sy = (y1 - y0 - 2.0*margin) / (maxy-miny).max(0.001);
    let sc = sx.min(sy);
    let ox = x0 + (x1-x0 - (maxx-minx)*sc)/2.0 - minx*sc;
    let oy = y0 + (y1-y0 - (maxy-miny)*sc)/2.0 - miny*sc;
    pts.iter().map(|p| Pt::new(ox+p.x*sc, oy+p.y*sc)).collect()
}
fn norm_t(t: f32) -> f32 { t.rem_euclid(1.0) }
fn angle_t(x: f32, y: f32) -> f32 { ((y-CY).atan2(x-CX) / TAU + 1.25).rem_euclid(1.0) }
fn point_on_circle(r: f32, t: f32) -> Pt {
    let a = (t - 0.25) * TAU;
    Pt::new(CX + r*a.cos(), CY + r*a.sin())
}

pub fn score_border_layout(score: &WeaveScore) -> Vec<BorderTickLayout> {
    let phrase_count = score.phrases.len().max(1);
    let slot = 1.0 / phrase_count as f32;
    let gap = (slot * 0.08).min(0.014);
    let mut layout = Vec::with_capacity(score.ticks.len());
    for (phrase_index, phrase) in score.phrases.iter().enumerate() {
        let phrase_start = phrase_index as f32 * slot + gap * 0.5;
        let phrase_span = (slot - gap).max(slot * 0.5);
        let tick_count = phrase.tick_count.max(1);
        for score_tick in 0..phrase.tick_count {
            let start_t = phrase_start + phrase_span * score_tick as f32 / tick_count as f32;
            let end_t = phrase_start + phrase_span * (score_tick + 1) as f32 / tick_count as f32;
            let tick = &score.ticks[phrase.first_tick + score_tick];
            let p = point_on_circle(BEAT_R, (start_t + end_t) * 0.5);
            layout.push(BorderTickLayout {
                phrase_id: phrase.phrase_id,
                score_tick,
                x: p.x,
                y: p.y,
                is_kick: tick.tick_in_group == 0,
                start_t,
                end_t,
            });
        }
    }
    layout
}

fn jump_routes(phrases: &[Phrase], score: &WeaveScore) -> Vec<JumpRoute> {
    let layout = score_border_layout(score);
    let mut bounds = HashMap::new();
    for phrase in &score.phrases {
        let phrase_ticks: Vec<_> = layout.iter().filter(|tick| tick.phrase_id == phrase.phrase_id).collect();
        if let (Some(first), Some(last)) = (phrase_ticks.first(), phrase_ticks.last()) {
            bounds.insert(phrase.phrase_id, (first.start_t, last.end_t));
        }
    }
    let musical_ids: Vec<usize> = phrases.iter()
        .filter(|phrase| phrase.jump.is_none() && phrase.control.is_none())
        .map(|phrase| phrase.id).collect();
    let resolve_target = |target_id: usize| -> Option<usize> {
        if bounds.contains_key(&target_id) { return Some(target_id); }
        musical_ids.iter().copied().find(|id| *id >= target_id).or_else(|| musical_ids.first().copied())
    };
    let mut previous_phrase_id = None;
    let mut routes = Vec::new();
    for phrase in phrases {
        if let Some(jump) = &phrase.jump {
            let Some(source_phrase_id) = previous_phrase_id else { continue; };
            let Some(target_phrase_id) = resolve_target(jump.target_id) else { continue; };
            let (Some((_, source_t)), Some((target_t, _))) = (bounds.get(&source_phrase_id).copied(), bounds.get(&target_phrase_id).copied()) else { continue; };
            routes.push(JumpRoute { jump_id: phrase.id, source_phrase_id, target_phrase_id, source_t, target_t });
        } else if phrase.control.is_none() {
            previous_phrase_id = Some(phrase.id);
        }
    }
    routes
}

pub fn jump_link_cells(phrases: &[Phrase]) -> Vec<JumpLinkCell> {
    let score = WeaveScore::from_phrases(phrases);
    let routes = jump_routes(phrases, &score);
    let mut cells = Vec::new();
    for (order, route) in routes.into_iter().enumerate() {
        let r = (INNER_R - 22.0 - order as f32 * 18.0).max(58.0);
        let mut target_t = route.target_t;
        while target_t >= route.source_t { target_t -= 1.0; }
        let steps = (((route.source_t - target_t).abs() * 130.0).ceil() as usize).max(12);
        for step in 0..=steps {
            let t = route.source_t + (target_t - route.source_t) * step as f32 / steps as f32;
            let p = point_on_circle(r, t);
            cells.push(JumpLinkCell { jump_id: route.jump_id, x: p.x, y: p.y, size: if step == steps { 7 } else { 5 } });
        }
    }
    cells
}

fn draw_arc(buf: &mut [u8], w: usize, r: f32, t0: f32, t1: f32, rgb: [u8;3], a: f32, thick: i32) {
    let mut end = t1;
    if end < t0 { end += 1.0; }
    let steps = ((end - t0).abs() * 180.0).ceil().max(8.0) as usize;
    let mut prev = point_on_circle(r, t0);
    for i in 1..=steps {
        let t = t0 + (end - t0) * i as f32 / steps as f32;
        let p = point_on_circle(r, norm_t(t));
        thick_line(buf, w, prev.x.round() as i32, prev.y.round() as i32, p.x.round() as i32, p.y.round() as i32, rgb, a, thick);
        prev = p;
    }
}
fn draw_base_weave(buf: &mut [u8], w: usize) {
    for y in 0..CARPET_H {
        for x in 0..w {
            let n = hash01((x as u32).wrapping_mul(73856093) ^ (y as u32).wrapping_mul(19349663));
            let warp = if x % 4 < 2 { 1.08 } else { 0.92 };
            let weft = if y % 4 < 2 { 1.04 } else { 0.96 };
            let base = [10.0, 9.0, 8.0];
            put_px(buf, w, x, y, [
                (base[0] * warp * weft + n * 5.0) as u8,
                (base[1] * warp * weft + n * 4.0) as u8,
                (base[2] * warp * weft + n * 3.0) as u8,
            ]);
        }
    }
}
fn draw_outer_hilbert(buf: &mut [u8], w: usize, score: &WeaveScore) {
    let pts = fit_points(&hilbert_points(6), 28.0, 24.0, w as f32 - 28.0, CARPET_H as f32 - 24.0, 10.0);
    let clear_r2 = (BEAT_R + 34.0) * (BEAT_R + 34.0);
    for seg in pts.windows(2) {
        let a = seg[0]; let b = seg[1]; let m = a.add(b).mul(0.5);
        let dx = m.x - CX; let dy = m.y - CY;
        if dx*dx + dy*dy <= clear_r2 { continue; }
        let t = angle_t(m.x, m.y);
        let phrase_idx = ((t * score.phrases.len().max(1) as f32).floor() as usize).min(score.phrases.len().saturating_sub(1));
        let phrase_id = score.phrases.get(phrase_idx).map(|p| p.phrase_id).unwrap_or(0);
        let c = dark(phrase_color(phrase_id), 0.35);
        thick_line(buf, w, a.x.round() as i32, a.y.round() as i32, b.x.round() as i32, b.y.round() as i32, dark(c, 0.65), 0.32, 4);
        thick_line(buf, w, a.x.round() as i32, a.y.round() as i32, b.x.round() as i32, b.y.round() as i32, c, 0.58, 2);
    }
}
fn draw_ring_score(buf: &mut [u8], w: usize, score: &WeaveScore) {
    dot(buf, w, CX.round() as i32, CY.round() as i32, INNER_R.round() as i32 + 14, [0, 0, 0], 0.74);
    dot(buf, w, CX.round() as i32, CY.round() as i32, INNER_R.round() as i32 - 4, [8, 7, 6], 0.58);
    for phrase in &score.phrases {
        let ticks: Vec<_> = score_border_layout(score).into_iter().filter(|t| t.phrase_id == phrase.phrase_id).collect();
        if let (Some(first), Some(last)) = (ticks.first(), ticks.last()) {
            let color = phrase_color(phrase.phrase_id);
            draw_arc(buf, w, INNER_R, first.start_t, last.end_t, dark(color, 0.18), 0.92, 13);
            draw_arc(buf, w, INNER_R - 16.0, first.start_t, last.end_t, [28, 22, 16], 0.75, 4);
        }
    }
    for tick in score_border_layout(score) {
        let c = if tick.is_kick { [214, 166, 94] } else { light(phrase_color(tick.phrase_id), 0.18) };
        dot(buf, w, tick.x.round() as i32, tick.y.round() as i32, if tick.is_kick { 12 } else { 7 }, [8, 6, 4], 0.95);
        dot(buf, w, tick.x.round() as i32, tick.y.round() as i32, if tick.is_kick { 8 } else { 5 }, c, 0.96);
    }
}
fn draw_jump_arrows(buf: &mut [u8], w: usize, phrases: &[Phrase], score: &WeaveScore) {
    for (order, route) in jump_routes(phrases, score).into_iter().enumerate() {
        let r = (INNER_R - 22.0 - order as f32 * 18.0).max(58.0);
        let mut target_t = route.target_t;
        while target_t >= route.source_t { target_t -= 1.0; }
        let color = if order % 2 == 0 { [218, 174, 108] } else { [190, 142, 82] };
        draw_arc(buf, w, r, route.source_t, target_t, [7, 5, 3], 0.90, 8);
        draw_arc(buf, w, r, route.source_t, target_t, color, 0.90, 4);
        let tip = point_on_circle(BEAT_R, route.target_t);
        dot(buf, w, tip.x.round() as i32, tip.y.round() as i32, 5, color, 0.95);
    }
}
fn draw_frame(buf: &mut [u8], w: usize) {
    for x in (24..w-24).step_by(8) {
        let len = 15 + (hash01(x as u32) * 18.0) as i32;
        line(buf, w, x as i32, 28, x as i32 + (hash01(x as u32 + 7)*12.0 - 6.0) as i32, 28-len, [118,100,76], 0.72);
        line(buf, w, x as i32, 692, x as i32 + (hash01(x as u32 + 13)*12.0 - 6.0) as i32, 692+len, [118,100,76], 0.72);
    }
    for y in (36..CARPET_H-36).step_by(8) {
        let len = 12 + (hash01(y as u32 + 99) * 16.0) as i32;
        line(buf, w, 38, y as i32, 38-len, y as i32 + (hash01(y as u32 + 3)*12.0 - 6.0) as i32, [118,100,76], 0.62);
        line(buf, w, w as i32-38, y as i32, w as i32-38+len, y as i32 + (hash01(y as u32 + 5)*12.0 - 6.0) as i32, [118,100,76], 0.62);
    }
    thick_line(buf, w, 52, 40, w as i32 - 52, 40, [72, 48, 30], 0.70, 8);
    thick_line(buf, w, 52, 680, w as i32 - 52, 680, [72, 48, 30], 0.70, 8);
    thick_line(buf, w, 52, 40, 52, 680, [72, 48, 30], 0.70, 8);
    thick_line(buf, w, w as i32 - 52, 40, w as i32 - 52, 680, [72, 48, 30], 0.70, 8);
}

pub fn write_carpet_background(path: impl AsRef<std::path::Path>, _entries: &[CarpetEntry], phrases: &[Phrase]) -> anyhow::Result<CarpetRenderInfo> {
    let score = WeaveScore::from_phrases(phrases);
    let mut buf = vec![0u8; W * CARPET_H * 3];
    draw_base_weave(&mut buf, W);
    draw_outer_hilbert(&mut buf, W, &score);
    draw_ring_score(&mut buf, W, &score);
    draw_jump_arrows(&mut buf, W, phrases, &score);
    // Beat dots are redrawn after jump arrows so the active-ring positions stay readable.
    draw_ring_score(&mut buf, W, &score);
    draw_frame(&mut buf, W);
    let path = path.as_ref();
    let mut f = std::fs::File::create(path)?;
    write!(f, "P6\n{} {}\n255\n", W, CARPET_H)?;
    f.write_all(&buf)?;
    f.flush()?;
    Ok(CarpetRenderInfo { path: path.to_string_lossy().replace('\\', "/"), width: W, content_end_x: W })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn score_cycles_match_border_tick_example() {
        assert_eq!(canonical_score_groups(&[4, 4, 4, 4]), vec![4, 4, 4, 4]);
        assert_eq!(canonical_score_groups(&[3, 3, 2, 3, 3, 2]), vec![3, 3, 2]);
        let total: usize = canonical_score_groups(&[4, 4, 4, 4])
            .into_iter()
            .chain(canonical_score_groups(&[3, 3, 2, 3, 3, 2]))
            .map(usize::from)
            .sum();
        assert_eq!(total, 24);
    }

    #[test]
    fn circular_layout_has_points() {
        let score = WeaveScore { phrases: vec![WeavePhrase { phrase_id: 0, groups: vec![4], first_tick: 0, tick_count: 4 }], ticks: (0..4).map(|i| WeaveTick { phrase_id: 0, group_index: 0, tick_in_group: i, group_len: 4 }).collect() };
        let layout = score_border_layout(&score);
        assert_eq!(layout.len(), 4);
        for tick in layout {
            assert!((tick.x - CX).abs() <= BEAT_R + 2.0);
            assert!((tick.y - CY).abs() <= BEAT_R + 2.0);
        }
    }
}
