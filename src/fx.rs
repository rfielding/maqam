#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CabinetModel {
    Vintage,
    Modern,
    Open,
}

impl CabinetModel {
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "vintage" | "4x12" => Some(Self::Vintage),
            "modern" | "tight" => Some(Self::Modern),
            "open" | "1x12" | "open-back" => Some(Self::Open),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Vintage => "vintage",
            Self::Modern => "modern",
            Self::Open => "open",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FxSettings {
    pub flanger_enabled: bool,
    pub flanger_rate_hz: f32,
    pub flanger_depth: f32,
    pub flanger_delay_ms: f32,
    pub flanger_feedback: f32,
    pub flanger_mix: f32,
    pub flanger_rate_step_per_tick: f32,
    pub flanger_depth_step_per_tick: f32,
    pub flanger_delay_step_per_tick: f32,
    pub flanger_feedback_step_per_tick: f32,
    pub flanger_mix_step_per_tick: f32,
    pub chorus_enabled: bool,
    pub chorus_rate_hz: f32,
    pub chorus_depth: f32,
    pub chorus_delay_ms: f32,
    pub chorus_mix: f32,
    pub chorus_rate_step_per_tick: f32,
    pub chorus_depth_step_per_tick: f32,
    pub chorus_delay_step_per_tick: f32,
    pub chorus_mix_step_per_tick: f32,
    pub reverb_enabled: bool,
    pub reverb_mix: f32,
    pub reverb_decay: f32,
    pub reverb_mix_step_per_tick: f32,
    pub reverb_decay_step_per_tick: f32,
    pub delay_enabled: bool,
    pub delay_time_secs: f32,
    pub delay_feedback: f32,
    pub delay_mix: f32,
    pub delay_time_step_per_tick: f32,
    pub delay_feedback_step_per_tick: f32,
    pub delay_mix_step_per_tick: f32,
    pub cabinet_enabled: bool,
    pub cabinet_model: CabinetModel,
    pub cabinet_mix: f32,
}

impl Default for FxSettings {
    fn default() -> Self {
        Self {
            flanger_enabled: false,
            flanger_rate_hz: 0.22,
            flanger_depth: 0.75,
            flanger_delay_ms: 2.8,
            flanger_feedback: 0.45,
            flanger_mix: 0.35,
            flanger_rate_step_per_tick: 0.0,
            flanger_depth_step_per_tick: 0.0,
            flanger_delay_step_per_tick: 0.0,
            flanger_feedback_step_per_tick: 0.0,
            flanger_mix_step_per_tick: 0.0,
            chorus_enabled: false,
            chorus_rate_hz: 0.7,
            chorus_depth: 0.55,
            chorus_delay_ms: 18.0,
            chorus_mix: 0.32,
            chorus_rate_step_per_tick: 0.0,
            chorus_depth_step_per_tick: 0.0,
            chorus_delay_step_per_tick: 0.0,
            chorus_mix_step_per_tick: 0.0,
            reverb_enabled: false,
            reverb_mix: 0.18,
            reverb_decay: 0.65,
            reverb_mix_step_per_tick: 0.0,
            reverb_decay_step_per_tick: 0.0,
            delay_enabled: false,
            delay_time_secs: 0.28,
            delay_feedback: 0.42,
            delay_mix: 0.22,
            delay_time_step_per_tick: 0.0,
            delay_feedback_step_per_tick: 0.0,
            delay_mix_step_per_tick: 0.0,
            cabinet_enabled: false,
            cabinet_model: CabinetModel::Vintage,
            cabinet_mix: 1.0,
        }
    }
}

/// A short, zero-allocation FIR cabinet stage intended for the live NAM input.
/// The bundled responses make scores portable; they are deliberately short to
/// keep the per-sample audio callback predictable.
pub struct CabinetProcessor {
    impulse: Vec<f32>,
    history: Vec<f32>,
    pos: usize,
    model: CabinetModel,
}

impl CabinetProcessor {
    pub fn new(sample_rate: f32) -> Self {
        let model = CabinetModel::Vintage;
        let impulse = cabinet_impulse(model, sample_rate);
        Self { history: vec![0.0; impulse.len()], impulse, pos: 0, model }
    }

    pub fn set_model(&mut self, model: CabinetModel, sample_rate: f32) {
        if self.model == model { return; }
        self.model = model;
        self.impulse = cabinet_impulse(model, sample_rate);
        self.history.resize(self.impulse.len(), 0.0);
        self.history.fill(0.0);
        self.pos = 0;
    }

    pub fn process(&mut self, input: f32, settings: FxSettings) -> f32 {
        if !settings.cabinet_enabled { return input; }
        self.history[self.pos] = input;
        let mut wet = 0.0;
        let mut read = self.pos;
        for coefficient in &self.impulse {
            wet += self.history[read] * coefficient;
            read = if read == 0 { self.history.len() - 1 } else { read - 1 };
        }
        self.pos = (self.pos + 1) % self.history.len();
        input * (1.0 - settings.cabinet_mix) + wet * settings.cabinet_mix
    }
}

fn cabinet_impulse(model: CabinetModel, sample_rate: f32) -> Vec<f32> {
    let (low_hz, high_hz, resonance, damping) = match model {
        CabinetModel::Vintage => (85.0, 5_200.0, 115.0, 32.0),
        CabinetModel::Modern => (105.0, 6_400.0, 145.0, 42.0),
        CabinetModel::Open => (70.0, 7_200.0, 92.0, 25.0),
    };
    let sr = sample_rate.max(8_000.0);
    let len = ((sr * 0.006).round() as usize).clamp(64, 384);
    let mut out = Vec::with_capacity(len);
    let mut previous_lp = 0.0;
    let lp_alpha = 1.0 - (-2.0 * std::f32::consts::PI * high_hz / sr).exp();
    let hp_decay = (-2.0 * std::f32::consts::PI * low_hz / sr).exp();
    let mut hp_state = 0.0;
    for n in 0..len {
        let t = n as f32 / sr;
        let excitation = if n == 0 { 1.0 } else { 0.0 }
            + (2.0 * std::f32::consts::PI * resonance * t).sin() * (-damping * t).exp() * 0.22;
        previous_lp += lp_alpha * (excitation - previous_lp);
        let high_passed = previous_lp - hp_state;
        hp_state = previous_lp + hp_decay * high_passed;
        out.push(high_passed * (-70.0 * t).exp());
    }
    let peak = out.iter().fold(0.0f32, |p, value| p.max(value.abs())).max(1e-6);
    for value in &mut out { *value /= peak; }
    out
}

impl FxSettings {
    pub fn active(self) -> bool {
        self.flanger_enabled || self.chorus_enabled || self.reverb_enabled || self.delay_enabled
    }

    pub fn advance_tick(&mut self) {
        if self.flanger_enabled {
            self.flanger_rate_hz =
                (self.flanger_rate_hz + self.flanger_rate_step_per_tick).clamp(0.01, 8.0);
            self.flanger_depth =
                (self.flanger_depth + self.flanger_depth_step_per_tick).clamp(0.0, 1.0);
            self.flanger_delay_ms =
                (self.flanger_delay_ms + self.flanger_delay_step_per_tick).clamp(0.1, 10.0);
            self.flanger_feedback =
                (self.flanger_feedback + self.flanger_feedback_step_per_tick).clamp(-0.95, 0.95);
            self.flanger_mix = (self.flanger_mix + self.flanger_mix_step_per_tick).clamp(0.0, 1.0);
        }
        if self.chorus_enabled {
            self.chorus_rate_hz =
                (self.chorus_rate_hz + self.chorus_rate_step_per_tick).clamp(0.01, 8.0);
            self.chorus_depth =
                (self.chorus_depth + self.chorus_depth_step_per_tick).clamp(0.0, 1.0);
            self.chorus_delay_ms =
                (self.chorus_delay_ms + self.chorus_delay_step_per_tick).clamp(5.0, 35.0);
            self.chorus_mix = (self.chorus_mix + self.chorus_mix_step_per_tick).clamp(0.0, 1.0);
        }
        if self.reverb_enabled {
            self.reverb_mix = (self.reverb_mix + self.reverb_mix_step_per_tick).clamp(0.0, 1.0);
            self.reverb_decay =
                (self.reverb_decay + self.reverb_decay_step_per_tick).clamp(0.0, 0.98);
        }
        if self.delay_enabled {
            self.delay_time_secs =
                (self.delay_time_secs + self.delay_time_step_per_tick).clamp(0.01, 2.0);
            self.delay_feedback =
                (self.delay_feedback + self.delay_feedback_step_per_tick).clamp(0.0, 0.95);
            self.delay_mix = (self.delay_mix + self.delay_mix_step_per_tick).clamp(0.0, 1.0);
        }
    }
}

pub struct FxProcessor {
    settings: FxSettings,
    sample_rate: f32,
    delay_l: Vec<f32>,
    delay_r: Vec<f32>,
    delay_pos: usize,
    delay_samples: usize,
    flanger_l: Vec<f32>,
    flanger_r: Vec<f32>,
    flanger_pos: usize,
    flanger_phase: f32,
    chorus_l: Vec<f32>,
    chorus_r: Vec<f32>,
    chorus_pos: usize,
    chorus_phase: f32,
    rev_l: Vec<f32>,
    rev_r: Vec<f32>,
    rev_pos: [usize; 4],
    rev_len: [usize; 4],
}

impl FxProcessor {
    pub fn new(sample_rate: f32) -> Self {
        let sample_rate = sample_rate.max(1.0);
        let max_delay = (sample_rate * 2.0).ceil() as usize + 1;
        let max_flanger = (sample_rate * 0.025).ceil() as usize + 4;
        let max_chorus = (sample_rate * 0.060).ceil() as usize + 4;
        let reverb_len = (sample_rate * 0.19).ceil() as usize + 1;
        let mut fx = Self {
            settings: FxSettings::default(),
            sample_rate,
            delay_l: vec![0.0; max_delay],
            delay_r: vec![0.0; max_delay],
            delay_pos: 0,
            delay_samples: 1,
            flanger_l: vec![0.0; max_flanger],
            flanger_r: vec![0.0; max_flanger],
            flanger_pos: 0,
            flanger_phase: 0.0,
            chorus_l: vec![0.0; max_chorus],
            chorus_r: vec![0.0; max_chorus],
            chorus_pos: 0,
            chorus_phase: 0.0,
            rev_l: vec![0.0; reverb_len],
            rev_r: vec![0.0; reverb_len],
            rev_pos: [0; 4],
            rev_len: [1; 4],
        };
        fx.recompute_cached_lengths();
        fx
    }

    pub fn set_settings(&mut self, settings: FxSettings) {
        self.settings = settings;
        self.recompute_cached_lengths();
    }

    pub fn process(&mut self, left: f32, right: f32) -> (f32, f32) {
        if !self.settings.active() {
            return (left, right);
        }
        let (left, right) = self.process_flanger(left, right);
        let (left, right) = self.process_chorus(left, right);
        let (left, right) = self.process_pingpong(left, right);
        self.process_reverb(left, right)
    }

    fn recompute_cached_lengths(&mut self) {
        self.delay_samples = (self.settings.delay_time_secs * self.sample_rate)
            .round()
            .clamp(1.0, (self.delay_l.len() - 1) as f32) as usize;
        for (i, tap) in [0.029, 0.041, 0.053, 0.071].iter().enumerate() {
            self.rev_len[i] = ((self.sample_rate * tap).round() as usize)
                .clamp(1, self.rev_l.len().saturating_sub(1).max(1));
            self.rev_pos[i] %= self.rev_len[i];
        }
    }

    fn process_flanger(&mut self, left: f32, right: f32) -> (f32, f32) {
        if !self.settings.flanger_enabled {
            return (left, right);
        }
        let lfo = 0.5 + 0.5 * self.flanger_phase.sin();
        let base = self.settings.flanger_delay_ms * self.sample_rate * 0.001;
        let sweep = base * self.settings.flanger_depth;
        let delay =
            (base + sweep * (lfo - 0.5) * 2.0).clamp(1.0, self.flanger_l.len() as f32 - 2.0);
        let wet_l = read_delay(&self.flanger_l, self.flanger_pos, delay);
        let wet_r = read_delay(&self.flanger_r, self.flanger_pos, delay);
        self.flanger_l[self.flanger_pos] =
            (left + wet_l * self.settings.flanger_feedback).clamp(-1.0, 1.0);
        self.flanger_r[self.flanger_pos] =
            (right + wet_r * self.settings.flanger_feedback).clamp(-1.0, 1.0);
        self.flanger_pos = (self.flanger_pos + 1) % self.flanger_l.len();
        self.flanger_phase = advance_lfo(
            self.flanger_phase,
            self.settings.flanger_rate_hz,
            self.sample_rate,
        );
        let dry = 1.0 - self.settings.flanger_mix;
        (
            (left * dry + wet_l * self.settings.flanger_mix).clamp(-1.0, 1.0),
            (right * dry + wet_r * self.settings.flanger_mix).clamp(-1.0, 1.0),
        )
    }

    fn process_chorus(&mut self, left: f32, right: f32) -> (f32, f32) {
        if !self.settings.chorus_enabled {
            return (left, right);
        }
        let lfo_l = 0.5 + 0.5 * self.chorus_phase.sin();
        let lfo_r = 0.5 + 0.5 * (self.chorus_phase + std::f32::consts::PI * 0.67).sin();
        let base = self.settings.chorus_delay_ms * self.sample_rate * 0.001;
        let sweep = base * self.settings.chorus_depth * 0.75;
        let delay_l =
            (base + sweep * (lfo_l - 0.5) * 2.0).clamp(1.0, self.chorus_l.len() as f32 - 2.0);
        let delay_r =
            (base + sweep * (lfo_r - 0.5) * 2.0).clamp(1.0, self.chorus_r.len() as f32 - 2.0);
        let wet_l = read_delay(&self.chorus_l, self.chorus_pos, delay_l);
        let wet_r = read_delay(&self.chorus_r, self.chorus_pos, delay_r);
        self.chorus_l[self.chorus_pos] = left;
        self.chorus_r[self.chorus_pos] = right;
        self.chorus_pos = (self.chorus_pos + 1) % self.chorus_l.len();
        self.chorus_phase = advance_lfo(
            self.chorus_phase,
            self.settings.chorus_rate_hz,
            self.sample_rate,
        );
        let dry = 1.0 - self.settings.chorus_mix;
        (
            (left * dry + wet_l * self.settings.chorus_mix).clamp(-1.0, 1.0),
            (right * dry + wet_r * self.settings.chorus_mix).clamp(-1.0, 1.0),
        )
    }

    fn process_pingpong(&mut self, left: f32, right: f32) -> (f32, f32) {
        if !self.settings.delay_enabled {
            return (left, right);
        }
        let read = (self.delay_pos + self.delay_l.len() - self.delay_samples) % self.delay_l.len();
        let dl = self.delay_l[read];
        let dr = self.delay_r[read];
        self.delay_l[self.delay_pos] = (left + dr * self.settings.delay_feedback).clamp(-1.0, 1.0);
        self.delay_r[self.delay_pos] = (right + dl * self.settings.delay_feedback).clamp(-1.0, 1.0);
        self.delay_pos = (self.delay_pos + 1) % self.delay_l.len();
        (
            (left + dl * self.settings.delay_mix).clamp(-1.0, 1.0),
            (right + dr * self.settings.delay_mix).clamp(-1.0, 1.0),
        )
    }

    fn process_reverb(&mut self, left: f32, right: f32) -> (f32, f32) {
        if !self.settings.reverb_enabled {
            return (left, right);
        }
        let mut wet_l = 0.0;
        let mut wet_r = 0.0;
        for i in 0..2 {
            let len = self.rev_len[i];
            let pos = self.rev_pos[i];
            let fb_l = self.rev_l[pos];
            let fb_r = self.rev_r[pos];
            wet_l += fb_l;
            wet_r += fb_r;
            self.rev_l[pos] = (left + fb_r * self.settings.reverb_decay).clamp(-1.0, 1.0);
            self.rev_r[pos] = (right + fb_l * self.settings.reverb_decay).clamp(-1.0, 1.0);
            self.rev_pos[i] = if pos + 1 >= len { 0 } else { pos + 1 };
        }
        wet_l *= 0.5;
        wet_r *= 0.5;
        let dry = 1.0 - self.settings.reverb_mix;
        (
            (left * dry + wet_l * self.settings.reverb_mix).clamp(-1.0, 1.0),
            (right * dry + wet_r * self.settings.reverb_mix).clamp(-1.0, 1.0),
        )
    }
}

fn advance_lfo(phase: f32, rate_hz: f32, sample_rate: f32) -> f32 {
    let next = phase + std::f32::consts::TAU * rate_hz / sample_rate.max(1.0);
    if next >= std::f32::consts::TAU {
        next - std::f32::consts::TAU
    } else {
        next
    }
}

fn read_delay(buf: &[f32], write_pos: usize, delay_samples: f32) -> f32 {
    let len = buf.len();
    let read = write_pos as f32 - delay_samples;
    let read = if read < 0.0 { read + len as f32 } else { read };
    let i0 = read.floor() as usize % len;
    let i1 = (i0 + 1) % len;
    let frac = read - read.floor();
    buf[i0] * (1.0 - frac) + buf[i1] * frac
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modulation_effects_produce_delayed_output() {
        let mut settings = FxSettings::default();
        settings.flanger_enabled = true;
        settings.chorus_enabled = true;
        let mut fx = FxProcessor::new(48_000.0);
        fx.set_settings(settings);

        let mut peak = 0.0f32;
        for sample in 0..2_000 {
            let input = if sample == 0 { 1.0 } else { 0.0 };
            let (left, right) = fx.process(input, input);
            if sample > 10 {
                peak = peak.max(left.abs()).max(right.abs());
            }
        }

        assert!(peak > 0.001);
    }

    #[test]
    fn cabinet_is_bypassed_or_convolves_without_allocating() {
        let mut cabinet = CabinetProcessor::new(48_000.0);
        let mut settings = FxSettings::default();
        assert_eq!(cabinet.process(0.25, settings), 0.25);

        settings.cabinet_enabled = true;
        settings.cabinet_mix = 1.0;
        let first = cabinet.process(1.0, settings);
        let tail = cabinet.process(0.0, settings);
        assert!(first.is_finite() && first.abs() > 0.01);
        assert!(tail.is_finite() && tail.abs() > 0.0001);
    }
}
