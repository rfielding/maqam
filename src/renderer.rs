// renderer.rs — pixel-only helpers for carpet/background video rendering
//
// This module intentionally does not try to procedurally invent the carpet.
// Its job is the part that already works visually:
//
//     fixed RGB carpet image -> darkened frame -> local bright re-draw mask
//
// The still carpet can come from a generated PNG decoded elsewhere, a future
// deterministic renderer, or an embedded/raw asset. Playback state should not
// create new geometry; it only guides the eye by re-drawing the existing carpet
// brighter around the current reading position.

#[derive(Clone)]
pub struct RgbImage {
    pub w: usize,
    pub h: usize,
    pub rgb: Vec<u8>,
}

#[derive(Clone, Copy, Debug)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl RgbImage {
    pub fn new(w: usize, h: usize, color: [u8; 3]) -> Self {
        let mut rgb = vec![0u8; w * h * 3];
        for px in rgb.chunks_exact_mut(3) {
            px.copy_from_slice(&color);
        }
        Self { w, h, rgb }
    }

    pub fn from_rgb(w: usize, h: usize, rgb: Vec<u8>) -> anyhow::Result<Self> {
        anyhow::ensure!(rgb.len() == w * h * 3, "RGB buffer has wrong length");
        Ok(Self { w, h, rgb })
    }

    pub fn darkened(&self, factor: f32) -> Self {
        let mut out = self.clone();
        for c in &mut out.rgb {
            *c = ((*c as f32) * factor).round().clamp(0.0, 255.0) as u8;
        }
        out
    }

    pub fn brightened(&self, brightness: f32, lift: f32, contrast: f32) -> Self {
        let mut out = self.clone();
        for px in out.rgb.chunks_exact_mut(3) {
            for c in px {
                let centered = *c as f32 - 128.0;
                *c = (128.0 + centered * contrast + lift + *c as f32 * (brightness - 1.0))
                    .round()
                    .clamp(0.0, 255.0) as u8;
            }
        }
        out
    }

    pub fn blend_px(&mut self, x: i32, y: i32, rgb: [u8; 3], alpha: f32) {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            return;
        }
        let i = (y as usize * self.w + x as usize) * 3;
        let a = alpha.clamp(0.0, 1.0);
        for ch in 0..3 {
            let base = self.rgb[i + ch] as f32;
            let over = rgb[ch] as f32;
            self.rgb[i + ch] = (base * (1.0 - a) + over * a).round().clamp(0.0, 255.0) as u8;
        }
    }

    pub fn composite_from_with_gaussian_mask(
        &mut self,
        bright_source: &RgbImage,
        center: Point,
        radius: f64,
        strength: f32,
    ) {
        if self.w != bright_source.w || self.h != bright_source.h {
            return;
        }
        let r = radius.max(1.0);
        let xmin = (center.x - r * 2.8).floor().max(0.0) as i32;
        let xmax = (center.x + r * 2.8).ceil().min((self.w - 1) as f64) as i32;
        let ymin = (center.y - r * 2.8).floor().max(0.0) as i32;
        let ymax = (center.y + r * 2.8).ceil().min((self.h - 1) as f64) as i32;
        for y in ymin..=ymax {
            for x in xmin..=xmax {
                let dx = (x as f64 - center.x) / r;
                let dy = (y as f64 - center.y) / r;
                let alpha = (-(dx * dx + dy * dy) * 0.70).exp() as f32 * strength;
                if alpha < 0.01 {
                    continue;
                }
                let i = (y as usize * self.w + x as usize) * 3;
                self.blend_px(
                    x,
                    y,
                    [bright_source.rgb[i], bright_source.rgb[i + 1], bright_source.rgb[i + 2]],
                    alpha,
                );
            }
        }
    }

    pub fn apply_vignette(&mut self, strength: f32) {
        let cx = (self.w as f32 - 1.0) * 0.5;
        let cy = (self.h as f32 - 1.0) * 0.5;
        let max_d = (cx * cx + cy * cy).sqrt().max(1.0);
        for y in 0..self.h {
            for x in 0..self.w {
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;
                let t = ((dx * dx + dy * dy).sqrt() / max_d).clamp(0.0, 1.0);
                let darken = 1.0 - strength * t * t;
                let i = (y * self.w + x) * 3;
                for ch in 0..3 {
                    self.rgb[i + ch] = (self.rgb[i + ch] as f32 * darken).round().clamp(0.0, 255.0) as u8;
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FitResult {
    pub x0: usize,
    pub y0: usize,
    pub w: usize,
    pub h: usize,
}

pub fn fit_center_rgb(src: &RgbImage, out_w: usize, out_h: usize, margin: usize) -> (RgbImage, FitResult) {
    let usable_w = out_w.saturating_sub(margin).max(1) as f64;
    let usable_h = out_h.saturating_sub(margin).max(1) as f64;
    let scale = (usable_w / src.w.max(1) as f64).min(usable_h / src.h.max(1) as f64);
    let rw = (src.w as f64 * scale).round().max(1.0).min(out_w as f64) as usize;
    let rh = (src.h as f64 * scale).round().max(1.0).min(out_h as f64) as usize;
    let x0 = (out_w - rw) / 2;
    let y0 = (out_h - rh) / 2;
    let mut out = RgbImage::new(out_w, out_h, [6, 6, 10]);

    for y in 0..rh {
        let sy = ((y as f64 + 0.5) * src.h as f64 / rh as f64).floor().min((src.h - 1) as f64) as usize;
        for x in 0..rw {
            let sx = ((x as f64 + 0.5) * src.w as f64 / rw as f64).floor().min((src.w - 1) as f64) as usize;
            let si = (sy * src.w + sx) * 3;
            let di = ((y0 + y) * out_w + (x0 + x)) * 3;
            out.rgb[di..di + 3].copy_from_slice(&src.rgb[si..si + 3]);
        }
    }

    (out, FitResult { x0, y0, w: rw, h: rh })
}

pub fn default_reading_path(fit: FitResult) -> Vec<Point> {
    let points = [
        (0.18, 0.32),
        (0.31, 0.28),
        (0.45, 0.34),
        (0.60, 0.28),
        (0.72, 0.30),
        (0.79, 0.45),
        (0.69, 0.58),
        (0.56, 0.62),
        (0.43, 0.56),
        (0.32, 0.64),
        (0.21, 0.56),
        (0.15, 0.42),
        (0.18, 0.32),
    ];
    points
        .iter()
        .map(|(x, y)| Point {
            x: fit.x0 as f64 + x * fit.w as f64,
            y: fit.y0 as f64 + y * fit.h as f64,
        })
        .collect()
}

pub fn growl_like_beat_positions(path: &[Point]) -> Vec<Point> {
    if path.len() < 13 {
        return path.to_vec();
    }
    let beats_per_section = [16usize, 4, 16, 16, 4, 4];
    let section_ranges = [(0usize, 2usize), (2, 3), (3, 6), (6, 10), (10, 11), (11, 12)];
    let mut out = Vec::new();
    for ((a, b), beats) in section_ranges.iter().copied().zip(beats_per_section) {
        let p0 = path[a];
        let p1 = path[b];
        for i in 0..beats {
            let t = i as f64 / beats.max(1) as f64;
            let tt = t * t * (3.0 - 2.0 * t);
            out.push(Point {
                x: p0.x * (1.0 - tt) + p1.x * tt,
                y: p0.y * (1.0 - tt) + p1.y * tt,
            });
        }
    }
    out.push(*path.last().unwrap());
    out
}

pub fn position_at_beat(beat_positions: &[Point], beat: f64) -> Point {
    if beat_positions.is_empty() {
        return Point { x: 0.0, y: 0.0 };
    }
    if beat_positions.len() == 1 {
        return beat_positions[0];
    }
    let i = beat.floor().rem_euclid(beat_positions.len() as f64) as usize;
    let j = (i + 1) % beat_positions.len();
    let frac = beat - beat.floor();
    Point {
        x: beat_positions[i].x * (1.0 - frac) + beat_positions[j].x * frac,
        y: beat_positions[i].y * (1.0 - frac) + beat_positions[j].y * frac,
    }
}

pub fn render_bright_redraw_frame(dark: &RgbImage, bright: &RgbImage, pos: Point, beat_phase: f64) -> RgbImage {
    let pulse = (-10.0 * beat_phase).exp() as f32;
    let mut frame = dark.clone();
    frame.composite_from_with_gaussian_mask(bright, pos, 58.0 + pulse as f64 * 10.0, 0.82);
    frame
}
