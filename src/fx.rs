#[derive(Clone, Copy, Debug)]
pub struct FxSettings {
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
}

impl Default for FxSettings {
    fn default() -> Self {
        Self {
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
        }
    }
}

impl FxSettings {
    pub fn active(self) -> bool {
        self.reverb_enabled || self.delay_enabled
    }

    pub fn advance_tick(&mut self) {
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
    rev_l: Vec<f32>,
    rev_r: Vec<f32>,
    rev_pos: [usize; 4],
    rev_len: [usize; 4],
}

impl FxProcessor {
    pub fn new(sample_rate: f32) -> Self {
        let sample_rate = sample_rate.max(1.0);
        let max_delay = (sample_rate * 2.0).ceil() as usize + 1;
        let reverb_len = (sample_rate * 0.19).ceil() as usize + 1;
        let mut fx = Self {
            settings: FxSettings::default(),
            sample_rate,
            delay_l: vec![0.0; max_delay],
            delay_r: vec![0.0; max_delay],
            delay_pos: 0,
            delay_samples: 1,
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
