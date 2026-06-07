#![allow(dead_code)]
// carpet.rs - controlled carpet rendering module
// Visual code for the carpet MP4 background lives here, not in record.rs.

use std::io::Write;

use crate::sequencer::{Phrase, SubdivEvent};

pub const CARPET_H: usize = 720;
pub const CARPET_START_X: usize = 1180;
pub const CARPET_STEP_X: usize = 30;

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

pub fn write_carpet_background(
    path: impl AsRef<std::path::Path>,
    entries: &[CarpetEntry],
    phrases: &[Phrase],
) -> anyhow::Result<CarpetRenderInfo> {
    let mut tick_count = 0usize;
    for e in entries {
        tick_count += phrases[e.phrase_idx].bar.events.len().max(1);
    }
    let w = (tick_count * CARPET_STEP_X + 760).max(1600);
    let mut buf = vec![0u8; w * CARPET_H * 3];
    fill_background(&mut buf, w);
    let mut x = CARPET_START_X as i32;
    let mut tick = 0usize;
    for e in entries {
        let bar = &phrases[e.phrase_idx].bar;
        let rows = bar.frequencies.len().max(1);
        for ev in &bar.events {
            let hz = event_hz(*ev);
            let row = bar
                .frequencies
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| {
                    let da = ((**a / hz).log2()).abs();
                    let db = ((**b / hz).log2()).abs();
                    da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(i, _)| i)
                .unwrap_or(0);
            for r in 0..rows {
                let top = 150i32;
                let bottom = 545i32;
                let y = top
                    + ((bottom - top) as f32 * ((r + 1) as f32 / (rows + 1) as f32)).round() as i32;
                let wave = ((tick as f32 * 0.41) + r as f32 * 0.73).sin();
                let active = r == row;
                draw_region_cell(
                    &mut buf,
                    w,
                    x,
                    y + (wave * 13.0) as i32,
                    e.phrase_idx + r,
                    active,
                );
                if active {
                    draw_stitches(&mut buf, w, x, y, tick as u32, bar.root_hz, hz);
                }
            }
            if tick > 0 {
                thick_line(
                    &mut buf,
                    w,
                    x - CARPET_STEP_X as i32,
                    350,
                    x,
                    350 + ((tick as f32 * 0.37).sin() * 44.0) as i32,
                    [96, 76, 58],
                    0.18,
                    2,
                );
            }
            x += CARPET_STEP_X as i32;
            tick += 1;
        }
    }
    let path = path.as_ref();
    let mut f = std::fs::File::create(path)?;
    write!(f, "P6\n{} {}\n255\n", w, CARPET_H)?;
    f.write_all(&buf)?;
    f.flush()?;
    Ok(CarpetRenderInfo {
        path: path.to_string_lossy().replace('\\', "/"),
        width: w,
        content_end_x: x.max(CARPET_START_X as i32) as usize,
    })
}
