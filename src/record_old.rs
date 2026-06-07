// record.rs — offline render to MP4

use std::collections::HashMap;
use std::io::Write;
use std::process::{Command, Stdio};

use crate::sequencer::{ControlSpec, Phrase, SubdivEvent};
use crate::synth::{
    evolve_bar, spawn_phrase_start, spawn_sub_bass, spawn_voices, Milestone, Voice,
};

const SR: f64 = 44100.0;

fn temp_path(name: &str) -> String {
    let mut p = std::env::temp_dir();
    p.push(name);
    p.to_string_lossy().replace('\\', "/")
}

fn ffmpeg_status(cmd: &mut Command) -> anyhow::Result<bool> {
    match cmd.status() {
        Ok(status) => Ok(status.success()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!(
                "video rendering requires ffmpeg on your PATH; install ffmpeg and try again"
            )
        }
        Err(err) => Err(err.into()),
    }
}

#[derive(Clone, Copy)]
struct RenderOccurrence {
    phrase_idx: usize,
    snap_idx: usize,
    bpm: f64,
    sustain: f64,
}

#[derive(Clone, Copy)]
struct RenderEntry {
    phrase_idx: usize,
    play_num: usize,
    snap_idx: usize,
    bpm: f64,
    sustain: f64,
}

// ── Ruler geometry (1280×720, x: 40..1240 = 1200px = 1px/¢) ─────────────────
//
//  y=588  ┌─ pitch indicator ───┐  h=12 → bottom 600
//         └─────────────────────┘
//  y=603  ┌─ rail ──────────────┤  h=16 → bottom 619
//  y=591  │  boundary ticks top │  (28px tall, same bottom 619)
//         └─────────────────────┘
//  y=621     ratio labels          11pt ≈ 15px → bottom 636
//
//         (21px clear gap)
//  y=657     URL text top
//  y=682     URL baseline (MarginV=38)

const IND_Y: i32 = 588;
const IND_W: i32 = 12;
const IND_H: i32 = 12;
const RAIL_Y: i32 = 603;
const RAIL_H: i32 = 16;
#[allow(dead_code)]
const BOUND_Y: i32 = 591;
#[allow(dead_code)]
const TICK_W: i32 = 2;
const LABEL_Y: i32 = 621;
const RULER_X0: i32 = 40;
const BG_H: usize = 720;

// ── JI ratio arithmetic ───────────────────────────────────────────────────────

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
}

/// Best rational p/q ≈ x with q ≤ max_denom (brute-force; fast for max_denom≤1024).
fn best_rational(x: f64, max_denom: u64) -> (u64, u64) {
    if x <= 0.0 {
        return (0, 1);
    }
    let mut best = (1u64, 1u64);
    let mut best_err = (x - 1.0).abs();
    for q in 1u64..=max_denom {
        let p = (x * q as f64).round() as u64;
        if p == 0 {
            continue;
        }
        let err = (x - p as f64 / q as f64).abs();
        if err < best_err {
            best_err = err;
            best = (p, q);
            if err < 1e-9 {
                break;
            }
        }
    }
    let g = gcd(best.0, best.1);
    (best.0 / g, best.1 / g)
}

/// Format hz/root_hz as a JI ratio like "4/3", normalised to [1, 2).
/// Special-cases: 0¢ → "1/1", 1200¢ → "2/1".
fn ratio_label(hz: f64, root_hz: f64) -> String {
    if root_hz <= 0.0 || hz <= 0.0 {
        return "?".into();
    }
    let c = cents_from_root(hz, root_hz);
    if c < 1.0 {
        return "1/1".into();
    }
    if c > 1199.0 {
        return "2/1".into();
    }
    let mut r = hz / root_hz;
    while r < 1.0 {
        r *= 2.0;
    }
    while r >= 2.0 {
        r /= 2.0;
    }
    let (p, q) = best_rational(r, 1024);
    format!("{p}/{q}")
}

fn cents_from_root(hz: f64, root_hz: f64) -> f64 {
    if root_hz <= 0.0 || hz <= 0.0 {
        return 0.0;
    }
    let raw = 1200.0 * (hz / root_hz).log2();
    ((raw % 1200.0) + 1200.0) % 1200.0
}

fn fill_rect(buf: &mut [u8], buf_w: usize, x: usize, y: usize, w: usize, h: usize, rgb: [u8; 3]) {
    let x1 = x.min(buf_w);
    let y1 = y.min(BG_H);
    let x2 = x.saturating_add(w).min(buf_w);
    let y2 = y.saturating_add(h).min(BG_H);
    for yy in y1..y2 {
        for xx in x1..x2 {
            let i = (yy * buf_w + xx) * 3;
            buf[i..i + 3].copy_from_slice(&rgb);
        }
    }
}

fn blend_px(buf: &mut [u8], buf_w: usize, x: i32, y: i32, rgb: [u8; 3], alpha: f32) {
    if x < 0 || y < 0 || x >= buf_w as i32 || y >= BG_H as i32 {
        return;
    }
    let idx = (y as usize * buf_w + x as usize) * 3;
    for c in 0..3 {
        let base = buf[idx + c] as f32;
        let over = rgb[c] as f32;
        buf[idx + c] = (base * (1.0 - alpha) + over * alpha)
            .round()
            .clamp(0.0, 255.0) as u8;
    }
}

fn draw_line(
    buf: &mut [u8],
    buf_w: usize,
    mut x0: i32,
    mut y0: i32,
    x1: i32,
    y1: i32,
    rgb: [u8; 3],
    alpha: f32,
) {
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        blend_px(buf, buf_w, x0, y0, rgb, alpha);
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
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

fn draw_thick_line(
    buf: &mut [u8],
    buf_w: usize,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    rgb: [u8; 3],
    alpha: f32,
    thickness: i32,
) {
    let half = thickness.max(1) / 2;
    for off in -half..=half {
        draw_line(buf, buf_w, x0 + off, y0, x1 + off, y1, rgb, alpha);
        draw_line(buf, buf_w, x0, y0 + off, x1, y1 + off, rgb, alpha);
    }
}

fn nearest_degree_index(hz: f64, freqs: &[f64]) -> usize {
    let mut best = 0usize;
    let mut best_dist = f64::MAX;
    for (i, &f) in freqs.iter().enumerate() {
        let dist = (f / hz).log2().abs();
        if dist < best_dist {
            best_dist = dist;
            best = i;
        }
    }
    best
}

fn normalized_octave_ratio(hz: f64, root_hz: f64) -> f64 {
    let mut ratio = hz / root_hz.max(f64::MIN_POSITIVE);
    while ratio >= 2.0 {
        ratio *= 0.5;
    }
    while ratio < 1.0 {
        ratio *= 2.0;
    }
    ratio
}

fn degree_shape_bias(hz: f64, root_hz: f64) -> (i32, i32) {
    let ratio = normalized_octave_ratio(hz, root_hz);
    let cents = ratio.log2() * 1200.0;
    let wave = (cents / 1200.0) * std::f64::consts::TAU;
    let lift = (wave.sin() * 7.0).round() as i32;
    let skew = (wave.cos() * 5.0).round() as i32;
    (lift, skew)
}

fn column_shape_bias(hz: f64, root_hz: f64, col_idx: usize) -> (i32, i32) {
    let ratio = normalized_octave_ratio(hz, root_hz);
    let cents = ratio.log2() * 1200.0;
    let phase = (cents / 1200.0) * std::f64::consts::TAU + col_idx as f64 * 0.45;
    let lift = (phase.sin() * 4.0).round() as i32;
    let skew = (phase.cos() * 3.0).round() as i32;
    (lift, skew)
}

fn hex_points(cx: i32, cy: i32, rx: i32, ry: i32, skew: i32) -> [(i32, i32); 6] {
    [
        (cx - rx, cy),
        (cx - rx / 2 + skew, cy - ry),
        (cx + rx / 2 + skew, cy - ry),
        (cx + rx, cy),
        (cx + rx / 2 - skew, cy + ry),
        (cx - rx / 2 - skew, cy + ry),
    ]
}

fn fill_polygon(buf: &mut [u8], buf_w: usize, pts: &[(i32, i32)], rgb: [u8; 3], alpha: f32) {
    if pts.len() < 3 {
        return;
    }
    let min_y = pts.iter().map(|p| p.1).min().unwrap_or(0).max(0);
    let max_y = pts
        .iter()
        .map(|p| p.1)
        .max()
        .unwrap_or(0)
        .min(BG_H as i32 - 1);
    for y in min_y..=max_y {
        let mut xs: Vec<i32> = Vec::new();
        for i in 0..pts.len() {
            let (x0, y0) = pts[i];
            let (x1, y1) = pts[(i + 1) % pts.len()];
            if y0 == y1 {
                continue;
            }
            let ymin = y0.min(y1);
            let ymax = y0.max(y1);
            if y < ymin || y >= ymax {
                continue;
            }
            let t = (y - y0) as f32 / (y1 - y0) as f32;
            xs.push((x0 as f32 + t * (x1 - x0) as f32).round() as i32);
        }
        xs.sort_unstable();
        for pair in xs.chunks(2) {
            if let [xa, xb] = pair {
                for x in (*xa).max(0)..=(*xb).min(buf_w as i32 - 1) {
                    blend_px(buf, buf_w, x, y, rgb, alpha);
                }
            }
        }
    }
}

fn draw_polygon_outline(
    buf: &mut [u8],
    buf_w: usize,
    pts: &[(i32, i32)],
    rgb: [u8; 3],
    alpha: f32,
    thickness: i32,
) {
    if pts.len() < 2 {
        return;
    }
    for i in 0..pts.len() {
        let (x0, y0) = pts[i];
        let (x1, y1) = pts[(i + 1) % pts.len()];
        draw_thick_line(buf, buf_w, x0, y0, x1, y1, rgb, alpha, thickness);
    }
}

fn write_tiling_background(
    full_seq: &[RenderEntry],
    phrases: &[Phrase],
) -> anyhow::Result<(String, usize, usize)> {
    const BG_START_X: usize = 1100;
    const BG_STEP_X: usize = 22;

    let path = temp_path("maqam-tiling.ppm");
    let tick_count: usize = full_seq
        .iter()
        .map(|entry| phrases[entry.phrase_idx].bar.events.len().max(1))
        .sum();
    let step_x = 42usize;
    let buf_w = (tick_count * step_x + 640).max(1600);
    let mut buf = vec![0u8; buf_w * BG_H * 3];
    fill_rect(&mut buf, buf_w, 0, 0, buf_w, BG_H, [4, 23, 12]);
    fill_rect(&mut buf, buf_w, 0, 0, buf_w, 220, [2, 16, 8]);
    fill_rect(&mut buf, buf_w, 0, 560, buf_w, 160, [2, 16, 8]);

    let line_main = [58, 62, 68];
    let line_accent = [90, 96, 104];
    let fill_main = [14, 16, 18];
    let fill_accent = [56, 60, 66];
    let start_x = BG_START_X;
    let mut x = start_x as i32;
    let mut active_until: Vec<usize> = Vec::new();
    let rx = 18i32;
    let step_x = BG_STEP_X as i32;
    let mut col_idx = 0usize;

    for entry in full_seq {
        let phrase_idx = entry.phrase_idx;
        let bar = &phrases[phrase_idx].bar;
        let rows = bar.frequencies.len().max(1);
        if active_until.len() != rows {
            active_until = vec![0; rows];
        }
        let sustain_steps =
            ((entry.sustain / (60.0 / (entry.bpm * 2.0))).ceil() as usize).clamp(1, 12);
        let degree_biases: Vec<(i32, i32)> = bar
            .frequencies
            .iter()
            .map(|&f| degree_shape_bias(f, bar.root_hz))
            .collect();
        let top = 230i32;
        let bottom = 530i32;
        let usable = bottom - top;
        let ry = ((usable / (rows as i32 * 2 + 1)).max(7)).min(18);
        let base_gap = ry * 2;
        let y_offset = if col_idx % 2 == 0 { 0 } else { ry };
        for ev in &bar.events {
            let hz = match ev {
                SubdivEvent::Kick(hz) | SubdivEvent::Snare(hz) => *hz,
            };
            let row = nearest_degree_index(hz, &bar.frequencies);
            let (col_lift, col_skew) = column_shape_bias(hz, bar.root_hz, col_idx);
            active_until[row] = col_idx + sustain_steps;
            for r in 0..rows {
                let (row_lift, row_skew) = degree_biases[r];
                let cy = top + y_offset + ry + r as i32 * base_gap + row_lift + col_lift;
                let pts = hex_points(x, cy, rx, ry, row_skew + col_skew);
                let fade = if active_until[r] > col_idx {
                    ((active_until[r] - col_idx) as f32 / sustain_steps as f32).clamp(0.15, 1.0)
                } else {
                    0.0
                };
                if fade > 0.0 {
                    let rgb = if r == row { fill_accent } else { fill_main };
                    let alpha = if r == row {
                        0.20 + fade * 0.12
                    } else {
                        0.03 + fade * 0.05
                    };
                    fill_polygon(&mut buf, buf_w, &pts, rgb, alpha);
                }
                draw_polygon_outline(
                    &mut buf,
                    buf_w,
                    &pts,
                    if r == row { line_accent } else { line_main },
                    if r == row { 0.60 } else { 0.36 },
                    if r == row { 3 } else { 1 },
                );
            }
            x += step_x;
            col_idx += 1;
        }
    }

    let mut f = std::fs::File::create(&path)?;
    write!(f, "P6\n{} {}\n255\n", buf_w, BG_H)?;
    f.write_all(&buf)?;
    f.flush()?;
    Ok((path, buf_w, x.max(start_x as i32) as usize))
}

// ── Ruler drawbox filter builder ──────────────────────────────────────────────

/// Build ffmpeg drawbox filter elements for the ruler + beat cursor.
/// Returns a Vec of individual filter strings to be chained with commas.
#[allow(dead_code)]
fn build_ruler_boxes(
    full_seq: &[(usize, usize, usize)],
    phrases: &[Phrase],
    subdiv_secs: f64,
    bar_samples_for: &dyn Fn(usize) -> usize,
    total_secs: f64,
) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut sample: usize = 0;
    let n_seq = full_seq.len();

    for (seq_i, &(phrase_idx, _play, _snap)) in full_seq.iter().enumerate() {
        let bs = bar_samples_for(phrase_idx);
        let t0 = sample as f64 / SR;
        // Last entry extends to total_secs so the ruler stays visible through the tail
        let t1 = if seq_i + 1 == n_seq {
            total_secs - 0.001
        } else {
            (sample + bs) as f64 / SR - 0.001
        };
        let en = format!("'between(t,{t0:.6},{t1:.6})'");

        let bar = &phrases[phrase_idx].bar;
        let root_hz = bar.root_hz;

        // ── Rail ───────────────────────────────────────────────────────────
        parts.push(format!(
            "drawbox=x={RULER_X0}:y={RAIL_Y}:w=1200:h={RAIL_H}\
             :color=0x003300@0.5:t=fill:enable={en}"
        ));

        /*
        // ── Pitch tick marks ───────────────────────────────────────────────
        for &hz in &bar.frequencies {
            let c  = cents_from_root(hz, root_hz);
            if c < 0.0 || c > 1200.5 { continue; }
            let x  = (RULER_X0 as f64 + c.clamp(0.0, 1200.0)).round() as i32;
            let ty = if c < 1.0 || c > 1199.0 { BOUND_Y } else { RAIL_Y };
            let h  = RAIL_Y + RAIL_H - ty;
            parts.push(format!(
                "drawbox=x={x}:y={ty}:w={TICK_W}:h={h}\
                 :color=0x44BB44:t=fill:enable={en}"
            ));
        }
        */

        // ── Active pitch indicator (yellow box, per subdivision) ───────────
        for (si, ev) in bar.events.iter().enumerate() {
            let st = t0 + si as f64 * subdiv_secs;
            let et = (t0 + (si + 1) as f64 * subdiv_secs).min(t1) - 0.0001;
            if st >= et {
                continue;
            }
            let sub_en = format!("'between(t,{st:.6},{et:.6})'");
            let hz = match ev {
                SubdivEvent::Kick(hz) | SubdivEvent::Snare(hz) => *hz,
            };
            let c = cents_from_root(hz, root_hz);
            if c < 0.0 || c > 1200.0 {
                continue;
            }
            let x = (RULER_X0 as f64 + c).round() as i32 - IND_W / 2;
            parts.push(format!(
                "drawbox=x={x}:y={IND_Y}:w={IND_W}:h={IND_H}\
                 :color=0xFFFF00:t=fill:enable={sub_en}"
            ));
        }

        sample += bs;
    }

    parts
}

// ── Sequence expansion ────────────────────────────────────────────────────────

fn expand_one_cycle(
    phrases: &[Phrase],
    start_bpm: f64,
    start_sustain: f64,
) -> (Vec<RenderOccurrence>, Vec<HashMap<usize, (usize, usize)>>) {
    let mut out: Vec<RenderOccurrence> = Vec::new();
    let mut snapshots: Vec<HashMap<usize, (usize, usize)>> = Vec::new();
    let mut cur: usize = 0;
    let mut jc: HashMap<usize, usize> = HashMap::new();
    let mut bpm = start_bpm;
    let mut sustain = start_sustain;
    let max_items = phrases.len() * 512 + 1;

    while out.len() < max_items {
        if cur >= phrases.len() {
            break;
        }
        let phrase = &phrases[cur];
        if let Some(js) = &phrase.jump {
            let pid = phrase.id;
            let remaining = jc.entry(pid).or_insert(js.times.saturating_sub(1));
            if *remaining > 0 {
                *remaining -= 1;
                let target = phrases
                    .iter()
                    .position(|p| p.id == js.target_id)
                    .unwrap_or(0)
                    .min(phrases.len().saturating_sub(1));
                let ids: Vec<usize> = if target < cur {
                    phrases[target..cur]
                        .iter()
                        .filter_map(|p| p.jump.as_ref().map(|_| p.id))
                        .collect()
                } else {
                    vec![]
                };
                for id in ids {
                    jc.remove(&id);
                }
                cur = target;
            } else {
                jc.remove(&pid);
                cur += 1;
            }
            continue;
        }
        if let Some(ctrl) = phrase.control {
            match ctrl {
                ControlSpec::SetBpm(v) => bpm = v,
                ControlSpec::SetSustain(v) => sustain = v,
            }
            cur += 1;
            continue;
        }
        let snap: HashMap<usize, (usize, usize)> = phrases
            .iter()
            .filter_map(|p| {
                p.jump.as_ref().map(|js| {
                    let remaining = jc.get(&p.id).copied().unwrap_or(js.times.saturating_sub(1));
                    let pass = js.times.saturating_sub(remaining);
                    (p.id, (pass, js.times))
                })
            })
            .collect();
        out.push(RenderOccurrence {
            phrase_idx: cur,
            snap_idx: snapshots.len(),
            bpm,
            sustain,
        });
        snapshots.push(snap);
        cur += 1;
        if cur >= phrases.len() {
            break;
        }
    }
    (out, snapshots)
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[allow(unused_variables)]
pub fn record_cycle(
    phrases: Vec<Phrase>,
    bpm: f64,
    sustain: f64,
    cycle_repeat: usize,
) -> anyhow::Result<String> {
    if phrases.is_empty() {
        return Err(anyhow::anyhow!("nothing to record"));
    }

    let bar_samples_for = |idx: usize, bpm: f64| -> usize {
        let subdiv_secs = 60.0 / (bpm * 2.0);
        let subdiv_samples = SR * subdiv_secs;
        ((subdiv_samples * phrases[idx].bar.total_subdivs as f64).round() as usize).max(1)
    };

    let (one_cycle_seq, one_cycle_snaps) = expand_one_cycle(&phrases, bpm, sustain);
    let _ = &one_cycle_snaps;
    if one_cycle_seq.is_empty() {
        return Err(anyhow::anyhow!("no musical phrases to render"));
    }

    let cycles = cycle_repeat.max(1);
    let mut tail_sustain = sustain;
    let mut full_seq: Vec<RenderEntry> = Vec::new();
    for _ in 0..cycles {
        for occ in &one_cycle_seq {
            let idx = occ.phrase_idx;
            tail_sustain = occ.sustain;
            for play in 0..phrases[idx].repeat.max(1) {
                full_seq.push(RenderEntry {
                    phrase_idx: idx,
                    play_num: play,
                    snap_idx: occ.snap_idx,
                    bpm: occ.bpm,
                    sustain: occ.sustain,
                });
            }
        }
    }
    let tail_samples = (SR * (tail_sustain + 1.0)) as usize;

    // ── Progress setup ────────────────────────────────────────────────────
    let render_samples: usize = full_seq
        .iter()
        .map(|entry| bar_samples_for(entry.phrase_idx, entry.bpm))
        .sum::<usize>()
        + tail_samples;
    crate::REC_SAMPLES_TOTAL.store(render_samples, std::sync::atomic::Ordering::Relaxed);
    crate::REC_SAMPLES_DONE.store(0, std::sync::atomic::Ordering::Relaxed);
    crate::REC_ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);

    // ── Audio render ──────────────────────────────────────────────────────
    let mut phrases_v = phrases.to_vec();
    let mut voices: Vec<Voice> = Vec::new();
    let mut left_buf: Vec<f32> = Vec::new();
    let mut right_buf: Vec<f32> = Vec::new();

    for (seq_pos, entry) in full_seq.iter().enumerate() {
        let phrase_idx = entry.phrase_idx;
        let play_num = entry.play_num;
        let bs = bar_samples_for(phrase_idx, entry.bpm);
        let is_first = play_num == 0;
        let repeats = phrases_v[phrase_idx].repeat.max(1);
        let subdiv_secs = 60.0 / (entry.bpm * 2.0);
        let subdiv_samples = SR * subdiv_secs;
        let sustain = entry.sustain;

        if is_first {
            let root_hz = phrases_v[phrase_idx].bar.root_hz;
            let phrase_secs =
                (phrases_v[phrase_idx].bar.total_subdivs as f64 * subdiv_secs * repeats as f64)
                    .min(3.0);
            spawn_phrase_start(root_hz, sustain, &mut voices);
            spawn_sub_bass(root_hz, phrase_secs, &mut voices);
        }

        let total_subdivs = phrases_v[phrase_idx].bar.total_subdivs;
        let mut bar_pos: usize = 0;
        let mut last_subdiv: Option<usize> = None;

        for _ in 0..bs {
            let ev = if total_subdivs > 0 {
                let curr = ((bar_pos as f64 / subdiv_samples) as usize).min(total_subdivs - 1);
                let ev = if last_subdiv != Some(curr) {
                    last_subdiv = Some(curr);
                    let is_last_play = play_num + 1 >= repeats;
                    let is_last_subdiv = curr + 1 >= total_subdivs;
                    // Look ahead in full_seq: is next entry a different phrase?
                    let next_is_different = full_seq
                        .get(seq_pos + 1)
                        .map_or(false, |next| next.phrase_idx != phrase_idx);
                    let milestone = if is_first && curr == 0 {
                        Milestone::PhraseStart
                    } else if is_last_play && is_last_subdiv {
                        if next_is_different {
                            Milestone::Turnaround
                        } else {
                            Milestone::CrossPhraseWarning
                        }
                    } else {
                        Milestone::None
                    };
                    phrases_v[phrase_idx]
                        .bar
                        .events
                        .get(curr)
                        .copied()
                        .map(|e| (e, milestone))
                } else {
                    None
                };
                bar_pos += 1;
                ev
            } else {
                None
            };

            if let Some((ev, milestone)) = ev {
                spawn_voices(
                    ev,
                    sustain,
                    &mut voices,
                    milestone,
                    &phrases_v[phrase_idx].bar.frequencies,
                );
            }

            let (mut l, mut r) = (0f32, 0f32);
            for v in voices.iter_mut() {
                let s = v.sample(SR);
                let angle = (v.pan + 1.0) * std::f32::consts::FRAC_PI_4;
                l += s * angle.cos();
                r += s * angle.sin();
            }
            left_buf.push(l);
            right_buf.push(r);
            voices.retain(|v| !v.done);
        }

        let done = left_buf.len().min(render_samples);
        crate::REC_SAMPLES_DONE.store(done, std::sync::atomic::Ordering::Relaxed);

        evolve_bar(&mut phrases_v[phrase_idx].bar, true);
        let _ = seq_pos;
    }

    if let Some(first) = full_seq.first() {
        let root_hz = phrases_v[first.phrase_idx].bar.root_hz;
        spawn_phrase_start(root_hz, first.sustain, &mut voices);
        spawn_sub_bass(root_hz, first.sustain.min(2.0), &mut voices);
    }
    for _ in 0..tail_samples {
        let (mut l, mut r) = (0f32, 0f32);
        for v in voices.iter_mut() {
            let s = v.sample(SR);
            let angle = (v.pan + 1.0) * std::f32::consts::FRAC_PI_4;
            l += s * angle.cos();
            r += s * angle.sin();
        }
        left_buf.push(l);
        right_buf.push(r);
        voices.retain(|v| !v.done);
    }

    // ── Normalize + write WAV ─────────────────────────────────────────────
    let peak = left_buf
        .iter()
        .chain(right_buf.iter())
        .map(|s| s.abs())
        .fold(0f32, f32::max);
    let gain = if peak > 0.001 { 0.9 / peak } else { 1.0 };

    let wav_path = temp_path("maqam-live.wav");
    {
        let n = left_buf.len() as u32;
        let sr = SR as u32;
        let dl = n * 4;
        let mut f = std::fs::File::create(&wav_path)?;
        f.write_all(b"RIFF")?;
        f.write_all(&(36 + dl).to_le_bytes())?;
        f.write_all(b"WAVE")?;
        f.write_all(b"fmt ")?;
        f.write_all(&16u32.to_le_bytes())?;
        f.write_all(&1u16.to_le_bytes())?;
        f.write_all(&2u16.to_le_bytes())?;
        f.write_all(&sr.to_le_bytes())?;
        f.write_all(&(sr * 4).to_le_bytes())?;
        f.write_all(&4u16.to_le_bytes())?;
        f.write_all(&16u16.to_le_bytes())?;
        f.write_all(b"data")?;
        f.write_all(&dl.to_le_bytes())?;
        for i in 0..left_buf.len() {
            let l = (left_buf[i] * gain * 32767.0).clamp(-32768.0, 32767.0) as i16;
            let r = (right_buf[i] * gain * 32767.0).clamp(-32768.0, 32767.0) as i16;
            f.write_all(&l.to_le_bytes())?;
            f.write_all(&r.to_le_bytes())?;
        }
        f.flush()?;
        f.sync_all()?;
    }
    let wav_path = wav_path.as_str();

    // ── ASS subtitle file ─────────────────────────────────────────────────
    let total_secs = left_buf.len() as f64 / SR;
    let ass_path_s = temp_path("maqam-live.ass");
    let ass_path = ass_path_s.as_str();
    {
        let mut f = std::fs::File::create(ass_path)?;
        writeln!(f, "[Script Info]")?;
        writeln!(f, "ScriptType: v4.00+")?;
        writeln!(f, "PlayResX: 1280")?;
        writeln!(f, "PlayResY: 720")?;
        writeln!(f, "WrapStyle: 0")?;
        writeln!(f, "[V4+ Styles]")?;
        writeln!(f, "Format: Name,Fontname,Fontsize,PrimaryColour,SecondaryColour,OutlineColour,BackColour,Bold,Italic,Underline,Strikeout,ScaleX,ScaleY,Spacing,Angle,BorderStyle,Outline,Shadow,Alignment,MarginL,MarginR,MarginV,Encoding")?;
        // Temporary Apple IIe-ish look: bold blocky monospace, phosphor green,
        // dark outline, and a touch of horizontal stretch.
        writeln!(f, "Style: Line,Courier New,22,&H0088FF88,&H0088FF88,&H00081402,&H00081402,-1,0,0,0,108,100,0,0,1,3,0,7,20,20,10,1")?;
        // URL: MarginV=38 → baseline y≈682, text top y≈657, well below ruler labels
        writeln!(f, "Style: URL,Courier New,18,&H0066CC66,&H0066CC66,&H00081402,&H00081402,-1,0,0,0,106,100,0,0,1,2,0,1,20,20,38,1")?;
        // Ratio labels: dimmer green so they feel like monitor overlay text too
        writeln!(f, "Style: RulerLabel,Courier New,12,&H004FA84F,&H004FA84F,&H00081402,&H00081402,-1,0,0,0,104,100,0,0,1,1,0,1,0,0,0,1")?;
        writeln!(f, "[Events]")?;
        writeln!(
            f,
            "Format: Layer,Start,End,Style,Name,MarginL,MarginR,MarginV,Effect,Text"
        )?;

        let one_len: usize = one_cycle_seq
            .iter()
            .map(|occ| phrases[occ.phrase_idx].repeat.max(1))
            .sum();

        let fmt_t = |s: f64| -> String {
            let hh = (s / 3600.0) as u32;
            let mm = ((s % 3600.0) / 60.0) as u32;
            let ss = (s % 60.0) as u32;
            let cs = ((s % 1.0) * 100.0) as u32;
            format!("{hh}:{mm:02}:{ss:02}.{cs:02}")
        };

        let mut sample: usize = 0;
        for (i, entry) in full_seq.iter().enumerate() {
            let phrase_idx = entry.phrase_idx;
            let play_num = entry.play_num;
            let snap_idx = entry.snap_idx;
            let bs = bar_samples_for(phrase_idx, entry.bpm);
            let start_s = sample as f64 / SR;
            let end_s = if i + 1 < full_seq.len() {
                (sample + bs) as f64 / SR
            } else {
                total_secs
            };
            let t0 = fmt_t(start_s);
            let t1 = fmt_t(end_s);

            let cycle_num = if one_len > 0 { i / one_len } else { 0 };
            let cycle_disp = if cycles > 1 {
                format!("  cycle {}/{}", cycle_num + 1, cycles)
            } else {
                String::new()
            };

            let subdiv_secs = 60.0 / (entry.bpm * 2.0);
            let hdr = format!(
                "   bpm:{:<4} sus:{:.1}s{}",
                entry.bpm.round() as u32,
                entry.sustain,
                cycle_disp
            );
            writeln!(f, "Dialogue: 0,{t0},{t1},Line,,0,0,0,,{hdr}")?;
            writeln!(
                f,
                "Dialogue: 0,{t0},{t1},URL,,0,0,0,,https://github.com/rfielding/maqam"
            )?;

            // JI ratio labels below each pitch tick
            let bar = &phrases[phrase_idx].bar;
            let root_hz = bar.root_hz;
            for &hz in &bar.frequencies {
                let c = cents_from_root(hz, root_hz);
                if c < 0.0 || c > 1200.5 {
                    continue;
                }
                let x = ((RULER_X0 as f64 + c.clamp(0.0, 1200.0)) as i32).max(0);
                let lbl = ratio_label(hz, root_hz);
                // \an1 = bottom-left anchor so text reads rightward from tick position
                writeln!(
                    f,
                    "Dialogue: 1,{t0},{t1},RulerLabel,,0,0,0,,\
                     {{\\pos({x},{LABEL_Y})\\an1}}{lbl}"
                )?;
            }

            // Phrase list
            // Active phrase: one Dialogue per subdivision so the beat cursor
            // character (white+bold) advances in real time.
            // Inactive phrases: single Dialogue for the whole phrase window.
            let line_h: usize = 26;
            let mut margin_v: usize = 30;
            for (pi, p) in phrases.iter().enumerate() {
                let active = p.jump.is_none() && pi == phrase_idx;
                let id = format!("{:>3}", p.id);

                if let Some(js) = &p.jump {
                    // Jump entry — always static
                    let snap = one_cycle_snaps.get(snap_idx % one_cycle_snaps.len().max(1));
                    let (pass, total) = snap
                        .and_then(|s| s.get(&p.id))
                        .copied()
                        .unwrap_or((0, js.times));
                    let text = format!("- {id}: {:<20} [{}/{}]", p.src, pass, total);
                    writeln!(f, "Dialogue: 0,{t0},{t1},Line,,0,0,{margin_v},,{text}")?;
                } else if active {
                    // Active musical phrase: per-subdivision beat cursor
                    let rhythm_plain = p.bar.rhythm_display();
                    let maqam_str = p.bar.ratio_strs.join(" | ");
                    let ctr = format!("[{}/{}]", play_num + 1, p.repeat.max(1));
                    let n = p.bar.events.len().max(1);

                    for si in 0..n {
                        let ts0 = fmt_t(start_s + si as f64 * subdiv_secs);
                        let ts1 = fmt_t((start_s + (si + 1) as f64 * subdiv_secs).min(end_s));
                        // Build rhythm string: active char white+bold, rest green
                        let mut rhy = String::new();
                        for (ci, ch) in rhythm_plain.chars().enumerate() {
                            if ci == si {
                                // Inverted: black text on white box.
                                // ASS has no per-char background, so we use a thick
                                // white outline (\3c white, \bord6) which fills the
                                // character cell, then black text (\1c black) on top.
                                // \shad0 suppresses shadow so it doesn't bleed.
                                // After: reset to green text, thin outline, no shadow.
                                rhy.push_str(&format!(
                                    "{{\\1c&H00000000&\\3c&H00FFFFFF&\\bord6\\shad0}}{ch}\
                                     {{\\1c&H0000FF00&\\3c&H00000000&\\bord2\\shad0}}"
                                ));
                            } else {
                                rhy.push(ch);
                            }
                        }
                        // Preserve {:<10} field width using plain rhythm length
                        let pad = " ".repeat(10usize.saturating_sub(rhythm_plain.chars().count()));
                        let body =
                            format!("{:<20} {}{} {:<16} {}", p.src, rhy, pad, maqam_str, ctr);
                        let text = format!("▶ {id}: {body}");
                        writeln!(f, "Dialogue: 0,{ts0},{ts1},Line,,0,0,{margin_v},,{text}")?;
                    }

                    // Tail: last phrase stays visible through the sustain tail.
                    // Per-subdivision Dialogues end at phrase end; this covers
                    // the silence tail so the row doesn't go black.
                    let phrase_end_s = start_s + n as f64 * subdiv_secs;
                    if phrase_end_s < end_s {
                        let ts0 = fmt_t(phrase_end_s);
                        let body = format!(
                            "{:<20} {:<10} {:<16} {}",
                            p.src, rhythm_plain, maqam_str, ctr
                        );
                        let text = format!("> {id}: {body}");
                        writeln!(f, "Dialogue: 0,{ts0},{t1},Line,,0,0,{margin_v},,{text}")?;
                    }
                } else {
                    // Inactive musical phrase — static
                    let rhythm = p.bar.rhythm_display();
                    let maqam_str = p.bar.ratio_strs.join(" | ");
                    let body = format!("{:<20} {:<10} {:<16}", p.src, rhythm, maqam_str);
                    let text = format!("- {id}: {body}");
                    writeln!(f, "Dialogue: 0,{t0},{t1},Line,,0,0,{margin_v},,{text}")?;
                }

                margin_v += line_h;
            }

            sample += bs;
        }
        f.flush()?;
    }

    let result = (|| -> anyhow::Result<String> {
        // ── Build filter complex ──────────────────────────────────────────────
        let (bg_path, bg_w, content_right) = write_tiling_background(&full_seq, &phrases)?;
        let scroll_range = bg_w.saturating_sub(1280);
        const BG_START_X: usize = 1100;
        const BG_STEP_X: usize = 22;
        let base_subdiv_secs = full_seq
            .first()
            .map(|entry| 60.0 / (entry.bpm * 2.0))
            .unwrap_or(0.5);

        let start_x = BG_START_X;
        let right_margin = 28usize;
        let latest_target = 1280usize.saturating_sub(right_margin);
        let scroll_expr = if scroll_range == 0 {
            "0".to_string()
        } else {
            let max_follow = content_right
                .saturating_sub(latest_target)
                .min(scroll_range);
            format!(
                "min(max(0,({start_x}+{step}*floor(t/{:.6}))-{latest_target}),{max_follow})",
                base_subdiv_secs.max(0.001),
                step = BG_STEP_X
            )
        };

        let filter_with_subs = format!(
            "[1:v]crop=1280:720:x='{scroll_expr}':y=0[bg];\
             [bg]subtitles={ass_path}[v]"
        );
        let filter_plain = format!(
            "[1:v]crop=1280:720:x='{scroll_expr}':y=0[bg];\
             [bg]null[v]"
        );
        let filter_bare = format!("[1:v]crop=1280:720:x='{scroll_expr}':y=0[v]");

        // Write to script file to sidestep OS command-line length limits.
        let fscript_path = temp_path("maqam-filter.txt");
        std::fs::write(&fscript_path, &filter_with_subs)?;

        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let out = format!("./maqam-{ts}.mp4");

        let log_path = temp_path("maqam-ffmpeg.log");

        // Pass 1: script file (ruler + subtitles)
        let ok1 = ffmpeg_status(
            Command::new("ffmpeg")
                .args([
                    "-y",
                    "-i",
                    wav_path,
                    "-loop",
                    "1",
                    "-framerate",
                    "30",
                    "-i",
                    &bg_path,
                    "-filter_complex_script",
                    &fscript_path,
                    "-map",
                    "[v]",
                    "-map",
                    "0:a",
                    "-c:v",
                    "libx264",
                    "-crf",
                    "18",
                    "-pix_fmt",
                    "yuv420p",
                    "-movflags",
                    "+faststart",
                    "-c:a",
                    "aac",
                    "-b:a",
                    "320k",
                    "-r",
                    "30",
                    "-shortest",
                    &out,
                ])
                .stdout(Stdio::null())
                .stderr(
                    std::fs::File::create(&log_path)
                        .map(Stdio::from)
                        .unwrap_or(Stdio::null()),
                ),
        )?;

        if !ok1 {
            // Pass 2: inline (ruler + subtitles)
            let ok2 = ffmpeg_status(
                Command::new("ffmpeg")
                    .args([
                        "-y",
                        "-i",
                        wav_path,
                        "-loop",
                        "1",
                        "-framerate",
                        "30",
                        "-i",
                        &bg_path,
                        "-filter_complex",
                        &filter_with_subs,
                        "-map",
                        "[v]",
                        "-map",
                        "0:a",
                        "-c:v",
                        "libx264",
                        "-crf",
                        "18",
                        "-pix_fmt",
                        "yuv420p",
                        "-movflags",
                        "+faststart",
                        "-c:a",
                        "aac",
                        "-b:a",
                        "320k",
                        "-r",
                        "30",
                        "-shortest",
                        &out,
                    ])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null()),
            )?;

            if !ok2 {
                // Pass 3: ruler, no subtitles
                let ok3 = ffmpeg_status(
                    Command::new("ffmpeg")
                        .args([
                            "-y",
                            "-i",
                            wav_path,
                            "-loop",
                            "1",
                            "-framerate",
                            "30",
                            "-i",
                            &bg_path,
                            "-filter_complex",
                            &filter_plain,
                            "-map",
                            "[v]",
                            "-map",
                            "0:a",
                            "-c:v",
                            "libx264",
                            "-crf",
                            "18",
                            "-pix_fmt",
                            "yuv420p",
                            "-movflags",
                            "+faststart",
                            "-c:a",
                            "aac",
                            "-b:a",
                            "320k",
                            "-r",
                            "30",
                            "-shortest",
                            &out,
                        ])
                        .stdout(Stdio::null())
                        .stderr(Stdio::null()),
                )?;

                if !ok3 {
                    // Pass 4: plain background, no subtitles
                    ffmpeg_status(
                        Command::new("ffmpeg")
                            .args([
                                "-y",
                                "-i",
                                wav_path,
                                "-loop",
                                "1",
                                "-framerate",
                                "30",
                                "-i",
                                &bg_path,
                                "-filter_complex",
                                &filter_bare,
                                "-map",
                                "[v]",
                                "-map",
                                "0:a",
                                "-c:v",
                                "libx264",
                                "-crf",
                                "18",
                                "-pix_fmt",
                                "yuv420p",
                                "-movflags",
                                "+faststart",
                                "-c:a",
                                "aac",
                                "-b:a",
                                "320k",
                                "-r",
                                "30",
                                "-shortest",
                                &out,
                            ])
                            .stdout(Stdio::null())
                            .stderr(Stdio::null()),
                    )?;
                }
            }
        }

        Ok(out)
    })();

    crate::REC_ACTIVE.store(false, std::sync::atomic::Ordering::Relaxed);
    crate::REC_SAMPLES_DONE.store(render_samples, std::sync::atomic::Ordering::Relaxed);

    result
}
