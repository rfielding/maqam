#![allow(dead_code)]
// carpet.rs - controlled carpet rendering module
// Visual code for the carpet MP4 background lives here, not in record.rs.

use std::io::Write;

use crate::sequencer::{Phrase, SubdivEvent};

pub const CARPET_H: usize = 720;
pub const CARPET_START_X: usize = 1180;
pub const CARPET_STEP_X: usize = 30;
const SCORE_X0: f32 = 218.0;
const SCORE_Y0: f32 = 158.0;
const SCORE_X1: f32 = 1280.0 - SCORE_X0;
const SCORE_Y1: f32 = CARPET_H as f32 - SCORE_Y0;

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

fn canonical_score_groups(groups: &[u8]) -> Vec<u8> {
    if groups.len() >= 6 && groups.len() % 2 == 0 {
        let half = groups.len() / 2;
        let motif = &groups[..half];
        if motif == &groups[half..]
            && motif
                .iter()
                .copied()
                .collect::<std::collections::HashSet<_>>()
                .len()
                > 1
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
            if phrase.jump.is_some() || phrase.control.is_some() {
                continue;
            }
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

fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

fn blend_px(buf: &mut [u8], w: usize, x: i32, y: i32, rgb: [u8; 3], a: f32) {
    if x < 0 || y < 0 || x >= w as i32 || y >= CARPET_H as i32 {
        return;
    }
    let a = clamp01(a);
    let i = (y as usize * w + x as usize) * 3;
    for c in 0..3 {
        let b = buf[i + c] as f32;
        let o = rgb[c] as f32;
        buf[i + c] = (b * (1.0 - a) + o * a).round().clamp(0.0, 255.0) as u8;
    }
}

fn fill_rect(buf: &mut [u8], w: usize, x: usize, y: usize, ww: usize, hh: usize, rgb: [u8; 3]) {
    let x2 = x.saturating_add(ww).min(w);
    let y2 = y.saturating_add(hh).min(CARPET_H);
    for yy in y.min(CARPET_H)..y2 {
        for xx in x.min(w)..x2 {
            let i = (yy * w + xx) * 3;
            buf[i..i + 3].copy_from_slice(&rgb);
        }
    }
}

fn line(
    buf: &mut [u8],
    w: usize,
    mut x0: i32,
    mut y0: i32,
    x1: i32,
    y1: i32,
    rgb: [u8; 3],
    a: f32,
) {
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        blend_px(buf, w, x0, y0, rgb, a);
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = err * 2;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}

fn thick_line(
    buf: &mut [u8],
    w: usize,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    rgb: [u8; 3],
    a: f32,
    t: i32,
) {
    let half = t.max(1) / 2;
    for off in -half..=half {
        line(buf, w, x0 + off, y0, x1 + off, y1, rgb, a);
        line(buf, w, x0, y0 + off, x1, y1 + off, rgb, a);
    }
}

fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846ca68b);
    x ^ (x >> 16)
}

fn hash01(x: u32) -> f32 {
    hash(x) as f32 / u32::MAX as f32
}

fn dot(buf: &mut [u8], w: usize, cx: i32, cy: i32, r: i32, rgb: [u8; 3], a: f32) {
    let r = r.max(1);
    let rr = (r * r) as f32;
    for y in cy - r..=cy + r {
        for x in cx - r..=cx + r {
            let dx = (x - cx) as f32;
            let dy = (y - cy) as f32;
            let d2 = dx * dx + dy * dy;
            if d2 <= rr {
                let falloff = 1.0 - (d2 / rr).sqrt();
                blend_px(buf, w, x, y, rgb, a * falloff);
            }
        }
    }
}

fn cents_from_root(hz: f64, root_hz: f64) -> f64 {
    if hz <= 0.0 || root_hz <= 0.0 {
        return 0.0;
    }
    let raw = 1200.0 * (hz / root_hz).log2();
    ((raw % 1200.0) + 1200.0) % 1200.0
}

fn event_hz(ev: SubdivEvent) -> f64 {
    match ev {
        SubdivEvent::Kick(hz) | SubdivEvent::Snare(hz) => hz,
    }
}

fn fill_background(buf: &mut [u8], w: usize) {
    fill_rect(buf, w, 0, 0, w, CARPET_H, [3, 7, 12]);
    fill_rect(buf, w, 0, 0, w, 82, [9, 8, 20]);
    fill_rect(buf, w, 0, CARPET_H - 92, w, 92, [7, 11, 15]);
    for x in (0..w).step_by(18) {
        let c = if (x / 18) % 2 == 0 {
            [26, 20, 42]
        } else {
            [16, 39, 34]
        };
        fill_rect(buf, w, x, 12, 10, 18, c);
        fill_rect(buf, w, x, CARPET_H - 32, 10, 18, c);
    }
    for x in (0..w).step_by(43) {
        thick_line(
            buf,
            w,
            x as i32,
            88,
            x as i32 + 22,
            116,
            [76, 52, 92],
            0.28,
            2,
        );
        thick_line(
            buf,
            w,
            x as i32,
            604,
            x as i32 + 22,
            632,
            [55, 84, 80],
            0.24,
            2,
        );
    }
}

fn draw_region_cell(buf: &mut [u8], w: usize, x: i32, y: i32, idx: usize, active: bool) {
    let palette = [
        [62, 30, 82],
        [22, 70, 54],
        [18, 66, 86],
        [92, 48, 30],
        [72, 60, 24],
    ];
    let edge = [158, 126, 74];
    let fill = palette[idx % palette.len()];
    let a = if active { 0.58 } else { 0.30 };
    for yy in -18i32..=18i32 {
        let span = 26 - (yy.abs() / 2);
        for xx in -span..=span {
            blend_px(buf, w, x + xx, y + yy, fill, a * 0.75);
        }
    }
    thick_line(
        buf,
        w,
        x - 26,
        y,
        x - 11,
        y - 18,
        edge,
        if active { 0.55 } else { 0.26 },
        if active { 3 } else { 1 },
    );
    thick_line(
        buf,
        w,
        x - 11,
        y - 18,
        x + 18,
        y - 16,
        edge,
        if active { 0.55 } else { 0.26 },
        if active { 3 } else { 1 },
    );
    thick_line(
        buf,
        w,
        x + 18,
        y - 16,
        x + 27,
        y + 3,
        edge,
        if active { 0.55 } else { 0.26 },
        if active { 3 } else { 1 },
    );
    thick_line(
        buf,
        w,
        x + 27,
        y + 3,
        x + 8,
        y + 20,
        edge,
        if active { 0.55 } else { 0.26 },
        if active { 3 } else { 1 },
    );
    thick_line(
        buf,
        w,
        x + 8,
        y + 20,
        x - 22,
        y + 15,
        edge,
        if active { 0.55 } else { 0.26 },
        if active { 3 } else { 1 },
    );
    thick_line(
        buf,
        w,
        x - 22,
        y + 15,
        x - 26,
        y,
        edge,
        if active { 0.55 } else { 0.26 },
        if active { 3 } else { 1 },
    );
}

fn draw_stitches(buf: &mut [u8], w: usize, x: i32, y: i32, seed: u32, root_hz: f64, hz: f64) {
    let cents = cents_from_root(hz, root_hz) as f32;
    let base_ang = cents / 1200.0 * std::f32::consts::TAU;
    let colors = [
        [220, 176, 88],
        [90, 184, 162],
        [150, 96, 196],
        [190, 82, 73],
    ];
    for i in 0..16u32 {
        let rr = 5.0 + 20.0 * hash01(seed ^ (i * 9187));
        let th = base_ang + i as f32 * 2.399963 + 0.8 * hash01(seed.wrapping_add(i));
        let px = x + (rr * th.cos()).round() as i32;
        let py = y + (rr * th.sin()).round() as i32;
        dot(buf, w, px, py, 3, colors[i as usize % colors.len()], 0.66);
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct BorderPoint {
    x: f32,
    y: f32,
}

fn border_point(x0: f32, y0: f32, x1: f32, y1: f32, t: f32) -> BorderPoint {
    let width = x1 - x0;
    let height = y1 - y0;
    let perimeter = 2.0 * (width + height);
    let distance = t.rem_euclid(1.0) * perimeter;
    if distance < width {
        BorderPoint {
            x: x0 + distance,
            y: y0,
        }
    } else if distance < width + height {
        BorderPoint {
            x: x1,
            y: y0 + distance - width,
        }
    } else if distance < width * 2.0 + height {
        BorderPoint {
            x: x1 - (distance - width - height),
            y: y1,
        }
    } else {
        BorderPoint {
            x: x0,
            y: y1 - (distance - width * 2.0 - height),
        }
    }
}

pub fn score_border_layout(score: &WeaveScore) -> Vec<BorderTickLayout> {
    let phrase_count = score.phrases.len().max(1);
    let slot = 1.0 / phrase_count as f32;
    let gap = (slot * 0.10).min(0.018);
    let mut layout = Vec::with_capacity(score.ticks.len());

    for (phrase_index, phrase) in score.phrases.iter().enumerate() {
        let phrase_start = phrase_index as f32 * slot + gap * 0.5;
        let phrase_span = (slot - gap).max(slot * 0.5);
        let tick_count = phrase.tick_count.max(1);
        for score_tick in 0..phrase.tick_count {
            let cell_start = phrase_start + phrase_span * score_tick as f32 / tick_count as f32;
            let cell_end = phrase_start + phrase_span * (score_tick + 1) as f32 / tick_count as f32;
            let tick_gap = ((cell_end - cell_start) * 0.18).min(0.0035);
            let start_t = cell_start + tick_gap * 0.5;
            let end_t = cell_end - tick_gap * 0.5;
            let midpoint = border_point(
                SCORE_X0,
                SCORE_Y0,
                SCORE_X1,
                SCORE_Y1,
                (start_t + end_t) * 0.5,
            );
            let tick = &score.ticks[phrase.first_tick + score_tick];
            layout.push(BorderTickLayout {
                phrase_id: phrase.phrase_id,
                score_tick,
                x: midpoint.x,
                y: midpoint.y,
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
    let mut bounds = std::collections::HashMap::new();
    for phrase in &score.phrases {
        let phrase_ticks: Vec<_> = layout
            .iter()
            .filter(|tick| tick.phrase_id == phrase.phrase_id)
            .collect();
        if let (Some(first), Some(last)) = (phrase_ticks.first(), phrase_ticks.last()) {
            bounds.insert(phrase.phrase_id, (first.start_t, last.end_t));
        }
    }
    let musical_ids: Vec<usize> = phrases
        .iter()
        .filter(|phrase| phrase.jump.is_none() && phrase.control.is_none())
        .map(|phrase| phrase.id)
        .collect();
    let resolve_target = |target_id: usize| -> Option<usize> {
        if bounds.contains_key(&target_id) {
            return Some(target_id);
        }
        musical_ids
            .iter()
            .copied()
            .find(|id| *id >= target_id)
            .or_else(|| musical_ids.first().copied())
    };

    let mut previous_phrase_id = None;
    let mut routes = Vec::new();
    for phrase in phrases {
        if let Some(jump) = &phrase.jump {
            let Some(source_phrase_id) = previous_phrase_id else {
                continue;
            };
            let Some(target_phrase_id) = resolve_target(jump.target_id) else {
                continue;
            };
            let (Some((_, source_t)), Some((target_t, _))) = (
                bounds.get(&source_phrase_id).copied(),
                bounds.get(&target_phrase_id).copied(),
            ) else {
                continue;
            };
            routes.push(JumpRoute {
                jump_id: phrase.id,
                source_phrase_id,
                target_phrase_id,
                source_t,
                target_t,
            });
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
        let inset = 22.0 + order as f32 * 28.0;
        let x0 = SCORE_X0 + inset;
        let y0 = SCORE_Y0 + inset;
        let x1 = SCORE_X1 - inset;
        let y1 = SCORE_Y1 - inset;
        let mut target_t = route.target_t;
        while target_t >= route.source_t {
            target_t -= 1.0;
        }
        let steps = (((route.source_t - target_t) * 90.0).ceil() as usize).max(12);
        for step in 0..=steps {
            let t = route.source_t + (target_t - route.source_t) * step as f32 / steps as f32;
            let p = border_point(x0, y0, x1, y1, t);
            cells.push(JumpLinkCell {
                jump_id: route.jump_id,
                x: p.x,
                y: p.y,
                size: if step == steps { 7 } else { 5 },
            });
        }
    }
    cells
}

fn draw_woven_connector(
    buf: &mut [u8],
    w: usize,
    start: BorderPoint,
    end: BorderPoint,
    color: [u8; 3],
    depth: usize,
    crossing_phase: usize,
) {
    thick_line(
        buf,
        w,
        start.x.round() as i32,
        start.y.round() as i32,
        end.x.round() as i32,
        end.y.round() as i32,
        color,
        0.82,
        3,
    );
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let length = (dx * dx + dy * dy).sqrt().max(0.001);
    let ux = dx / length;
    let uy = dy / length;
    for crossing in 0..depth {
        let t = (crossing + 1) as f32 / (depth + 1) as f32;
        let center = BorderPoint {
            x: start.x + dx * t,
            y: start.y + dy * t,
        };
        dot(
            buf,
            w,
            center.x.round() as i32,
            center.y.round() as i32,
            6,
            [10, 7, 4],
            0.96,
        );
        if (crossing + crossing_phase) % 2 == 0 {
            let half = 7.0;
            thick_line(
                buf,
                w,
                (center.x - ux * half).round() as i32,
                (center.y - uy * half).round() as i32,
                (center.x + ux * half).round() as i32,
                (center.y + uy * half).round() as i32,
                [218, 174, 108],
                0.94,
                4,
            );
        }
    }
}

fn draw_jump_arrow(buf: &mut [u8], w: usize, route: JumpRoute, depth: usize, order: usize) {
    let inset = 22.0 + depth as f32 * 28.0;
    let x0 = SCORE_X0 + inset;
    let y0 = SCORE_Y0 + inset;
    let x1 = SCORE_X1 - inset;
    let y1 = SCORE_Y1 - inset;
    let mut target_t = route.target_t;
    while target_t >= route.source_t {
        target_t -= 1.0;
    }
    let steps = (((route.source_t - target_t) * 180.0).ceil() as usize).max(16);
    let color = if order % 2 == 0 {
        [218, 174, 108]
    } else {
        [190, 142, 82]
    };
    let source_anchor = border_point(SCORE_X0, SCORE_Y0, SCORE_X1, SCORE_Y1, route.source_t);
    let target_anchor = border_point(SCORE_X0, SCORE_Y0, SCORE_X1, SCORE_Y1, route.target_t);
    let route_start = border_point(x0, y0, x1, y1, route.source_t);
    let route_end = border_point(x0, y0, x1, y1, target_t);
    draw_woven_connector(buf, w, source_anchor, route_start, color, depth, order);

    let mut previous = route_start;
    for step in 1..=steps {
        let t = route.source_t + (target_t - route.source_t) * step as f32 / steps as f32;
        let current = border_point(x0, y0, x1, y1, t);
        if (current.x - previous.x).abs() < 60.0 && (current.y - previous.y).abs() < 60.0 {
            thick_line(
                buf,
                w,
                previous.x.round() as i32,
                previous.y.round() as i32,
                current.x.round() as i32,
                current.y.round() as i32,
                [8, 5, 3],
                0.88,
                7,
            );
            thick_line(
                buf,
                w,
                previous.x.round() as i32,
                previous.y.round() as i32,
                current.x.round() as i32,
                current.y.round() as i32,
                color,
                0.92,
                4,
            );
        }
        previous = current;
    }

    draw_woven_connector(buf, w, route_end, target_anchor, color, depth, order + 1);

    let tip = target_anchor;
    let dx = tip.x - route_end.x;
    let dy = tip.y - route_end.y;
    let length = (dx * dx + dy * dy).sqrt().max(0.001);
    let ux = dx / length;
    let uy = dy / length;
    for side in [-1.0, 1.0] {
        let wing_x = tip.x - ux * 13.0 + -uy * 9.0 * side;
        let wing_y = tip.y - uy * 13.0 + ux * 9.0 * side;
        thick_line(
            buf,
            w,
            tip.x.round() as i32,
            tip.y.round() as i32,
            wing_x.round() as i32,
            wing_y.round() as i32,
            [210, 166, 102],
            0.92,
            4,
        );
    }
    dot(
        buf,
        w,
        tip.x.round() as i32,
        tip.y.round() as i32,
        3,
        [220, 178, 112],
        0.88,
    );
}

fn draw_jump_arrows(buf: &mut [u8], w: usize, phrases: &[Phrase], score: &WeaveScore) {
    let routes = jump_routes(phrases, score);
    for (order, route) in routes.into_iter().enumerate() {
        draw_jump_arrow(buf, w, route, order, order);
    }
}

fn draw_perimeter_segment(
    buf: &mut [u8],
    w: usize,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    start_t: f32,
    end_t: f32,
    color: [u8; 3],
    alpha: f32,
    thickness: i32,
) {
    let steps = 32usize;
    let mut previous = border_point(x0, y0, x1, y1, start_t);
    for step in 1..=steps {
        let t = start_t + (end_t - start_t) * step as f32 / steps as f32;
        let current = border_point(x0, y0, x1, y1, t);
        if (current.x - previous.x).abs() < 80.0 && (current.y - previous.y).abs() < 80.0 {
            thick_line(
                buf,
                w,
                previous.x.round() as i32,
                previous.y.round() as i32,
                current.x.round() as i32,
                current.y.round() as i32,
                color,
                alpha,
                thickness,
            );
        }
        previous = current;
    }
}

fn draw_woven_field(buf: &mut [u8], w: usize) {
    fill_rect(buf, w, 0, 0, w, CARPET_H, [8, 6, 4]);
    for y in 0..CARPET_H {
        let weft = if y % 4 < 2 { [31, 22, 13] } else { [18, 13, 8] };
        for x in 0..w {
            let warp = if x % 6 < 3 { [24, 16, 9] } else { [11, 8, 5] };
            let i = (y * w + x) * 3;
            let over = ((x / 3 + y / 2) & 1) == 0;
            let color = if over { warp } else { weft };
            buf[i..i + 3].copy_from_slice(&color);
        }
    }
}

fn draw_rect_border(
    buf: &mut [u8],
    w: usize,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    color: [u8; 3],
    thickness: i32,
) {
    thick_line(buf, w, x0, y0, x1, y0, color, 0.88, thickness);
    thick_line(buf, w, x1, y0, x1, y1, color, 0.88, thickness);
    thick_line(buf, w, x1, y1, x0, y1, color, 0.88, thickness);
    thick_line(buf, w, x0, y1, x0, y0, color, 0.88, thickness);
}

fn phrase_color(phrase_id: usize) -> [u8; 3] {
    const COLORS: [[u8; 3]; 6] = [
        [108, 64, 34],
        [126, 82, 44],
        [92, 54, 28],
        [142, 96, 52],
        [100, 72, 44],
        [118, 80, 48],
    ];
    COLORS[phrase_id % COLORS.len()]
}

fn draw_score_tick_border(buf: &mut [u8], w: usize, score: &WeaveScore) {
    let x0 = SCORE_X0;
    let y0 = SCORE_Y0;
    let x1 = w as f32 - SCORE_X0;
    let y1 = SCORE_Y1;
    draw_rect_border(
        buf,
        w,
        x0 as i32,
        y0 as i32,
        x1 as i32,
        y1 as i32,
        [48, 34, 21],
        12,
    );

    for tick in score_border_layout(score) {
        let color = if tick.is_kick {
            [174, 126, 70]
        } else {
            phrase_color(tick.phrase_id)
        };
        draw_perimeter_segment(
            buf,
            w,
            x0,
            y0,
            x1,
            y1,
            tick.start_t,
            tick.end_t,
            color,
            if tick.is_kick { 0.98 } else { 0.74 },
            if tick.is_kick { 10 } else { 6 },
        );
        dot(
            buf,
            w,
            tick.x.round() as i32,
            tick.y.round() as i32,
            if tick.is_kick { 4 } else { 2 },
            if tick.is_kick {
                [204, 158, 94]
            } else {
                [108, 76, 44]
            },
            0.88,
        );
    }
}

pub fn write_carpet_background(
    path: impl AsRef<std::path::Path>,
    _entries: &[CarpetEntry],
    phrases: &[Phrase],
) -> anyhow::Result<CarpetRenderInfo> {
    let score = WeaveScore::from_phrases(phrases);
    let w = 1280;
    let mut buf = vec![0u8; w * CARPET_H * 3];
    draw_woven_field(&mut buf, w);
    draw_rect_border(&mut buf, w, 44, 28, w as i32 - 44, 692, [118, 76, 38], 18);
    draw_rect_border(&mut buf, w, 64, 48, w as i32 - 64, 672, [166, 118, 62], 5);
    draw_score_tick_border(&mut buf, w, &score);
    draw_jump_arrows(&mut buf, w, phrases, &score);
    let path = path.as_ref();
    let mut f = std::fs::File::create(path)?;
    write!(f, "P6\n{} {}\n255\n", w, CARPET_H)?;
    f.write_all(&buf)?;
    f.flush()?;
    Ok(CarpetRenderInfo {
        path: path.to_string_lossy().replace('\\', "/"),
        width: w,
        content_end_x: w,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_phrase(id: usize, groups: Vec<u8>) -> Phrase {
        crate::sequencer::build_phrase(
            id,
            format!(
                "d bayati {}",
                groups.iter().map(u8::to_string).collect::<String>()
            ),
            vec![crate::sequencer::BarSpec {
                src: "d bayati".to_string(),
                root: crate::tuning::Pitch {
                    letter: 'd',
                    accidental: 0,
                    octave: 4,
                },
                maqam: crate::tuning::Maqam::new("Bayati"),
                groups,
            }],
            4,
            1,
        )
    }

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
    fn border_path_is_closed() {
        let start = border_point(10.0, 20.0, 110.0, 80.0, 0.0);
        let end = border_point(10.0, 20.0, 110.0, 80.0, 1.0);
        assert_eq!(start, end);
    }

    #[test]
    fn phrases_receive_equal_border_spans_with_gaps() {
        let score = WeaveScore {
            phrases: vec![
                WeavePhrase {
                    phrase_id: 0,
                    groups: vec![4, 4, 4, 4],
                    first_tick: 0,
                    tick_count: 16,
                },
                WeavePhrase {
                    phrase_id: 2,
                    groups: vec![3, 3, 2],
                    first_tick: 16,
                    tick_count: 8,
                },
            ],
            ticks: (0..24)
                .map(|index| WeaveTick {
                    phrase_id: if index < 16 { 0 } else { 2 },
                    group_index: 0,
                    tick_in_group: index % 4,
                    group_len: 4,
                })
                .collect(),
        };
        let layout = score_border_layout(&score);
        let first_span = layout[15].end_t - layout[0].start_t;
        let second_span = layout[23].end_t - layout[16].start_t;
        assert!((first_span - second_span).abs() < 0.0001);
        assert!(layout[16].start_t > layout[15].end_t);
    }

    #[test]
    fn jumps_route_from_previous_phrase_to_target_start() {
        let phrases = vec![
            test_phrase(0, vec![4, 4]),
            test_phrase(2, vec![3, 3, 2]),
            crate::sequencer::build_jump_entry(3, 0, 3),
        ];
        let score = WeaveScore::from_phrases(&phrases);
        let routes = jump_routes(&phrases, &score);
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].source_phrase_id, 2);
        assert_eq!(routes[0].target_phrase_id, 0);
        assert!(routes[0].source_t > routes[0].target_t);
    }

    #[test]
    fn jumps_to_control_lines_route_to_next_musical_phrase() {
        let phrases = vec![
            crate::sequencer::build_control_entry(
                0,
                "bpm 180".to_string(),
                crate::sequencer::ControlSpec::SetBpm(180.0),
            ),
            crate::sequencer::build_control_entry(
                1,
                "s 2".to_string(),
                crate::sequencer::ControlSpec::SetSustain(2.0),
            ),
            test_phrase(2, vec![4, 4, 4, 4]),
            crate::sequencer::build_jump_entry(3, 0, 3),
        ];
        let score = WeaveScore::from_phrases(&phrases);
        let routes = jump_routes(&phrases, &score);
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].source_phrase_id, 2);
        assert_eq!(routes[0].target_phrase_id, 2);
        assert!(routes[0].source_t > routes[0].target_t);
    }

    #[test]
    fn later_jumps_receive_deeper_lanes() {
        let phrases = vec![
            test_phrase(0, vec![4]),
            crate::sequencer::build_jump_entry(1, 0, 2),
            test_phrase(2, vec![3, 2]),
            crate::sequencer::build_jump_entry(3, 0, 3),
            test_phrase(4, vec![4]),
            crate::sequencer::build_jump_entry(5, 2, 4),
        ];
        let score = WeaveScore::from_phrases(&phrases);
        let routes = jump_routes(&phrases, &score);
        assert_eq!(routes.len(), 3);
        let insets: Vec<f32> = routes
            .iter()
            .enumerate()
            .map(|(order, _)| 22.0 + order as f32 * 28.0)
            .collect();
        assert_eq!(insets, vec![22.0, 50.0, 78.0]);
    }

    #[test]
    fn magiccarpet_jumps_form_four_inward_loops() {
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("magiccarpet.mq"),
        )
        .unwrap();
        let jump_count = source.lines().filter(|line| line.starts_with("J|")).count();
        assert_eq!(jump_count, 4);
        let insets: Vec<f32> = (0..jump_count)
            .map(|order| 22.0 + order as f32 * 28.0)
            .collect();
        assert_eq!(insets, vec![22.0, 50.0, 78.0, 106.0]);
    }
}
