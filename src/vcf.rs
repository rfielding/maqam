// src/vcf.rs — Moog-ish resonant 4-pole low-pass VCF

#[derive(Clone, Copy, Debug)]
pub struct VcfSettings {
    pub enabled: bool,
    pub target: VcfTarget,
    pub cutoff_hz: f32,
    pub resonance: f32,
    pub drive: f32,
    pub cutoff_step_per_tick: f32,
    pub resonance_step_per_tick: f32,
    pub drive_step_per_tick: f32,
    pub wave: VcoWave,
}

#[derive(Clone, Copy, Debug)]
pub struct VcfBank {
    pub focus: VcfTarget,
    pub all: VcfSettings,
    pub bass: VcfSettings,
    pub kanun: VcfSettings,
    pub kick: VcfSettings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VcfTarget {
    All,
    Bass,
    Kanun,
    Kick,
}

impl VcfTarget {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "all" | "mix" | "master" => Some(Self::All),
            "bass" | "sub" | "subbass" => Some(Self::Bass),
            "kanun" | "qanun" | "melody" => Some(Self::Kanun),
            "kick" | "kicks" => Some(Self::Kick),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Bass => "bass",
            Self::Kanun => "kanun",
            Self::Kick => "kick",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VcoWave {
    Sin,
    Tri,
    Squ,
    Saw,
}

impl VcoWave {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "sin" | "sine" => Some(Self::Sin),
            "tri" | "triangle" => Some(Self::Tri),
            "squ" | "square" | "sq" => Some(Self::Squ),
            "saw" | "sawtooth" => Some(Self::Saw),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sin => "sin",
            Self::Tri => "tri",
            Self::Squ => "squ",
            Self::Saw => "saw",
        }
    }

    #[inline]
    pub fn sample(self, phase: f64) -> f32 {
        let p = phase - phase.floor();
        match self {
            Self::Sin => (p * std::f64::consts::TAU).sin() as f32,
            Self::Tri => (1.0 - 4.0 * (p - 0.5).abs()) as f32,
            Self::Squ => {
                if p < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            Self::Saw => (2.0 * p - 1.0) as f32,
        }
    }
}

impl Default for VcfSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            target: VcfTarget::All,
            cutoff_hz: 1200.0,
            resonance: 0.35,
            drive: 1.5,
            cutoff_step_per_tick: 0.0,
            resonance_step_per_tick: 0.0,
            drive_step_per_tick: 0.0,
            wave: VcoWave::Sin,
        }
    }
}

impl VcfSettings {
    pub fn for_target(target: VcfTarget) -> Self {
        Self {
            target,
            ..Self::default()
        }
    }
}

impl Default for VcfBank {
    fn default() -> Self {
        Self {
            focus: VcfTarget::All,
            all: VcfSettings::for_target(VcfTarget::All),
            bass: VcfSettings::for_target(VcfTarget::Bass),
            kanun: VcfSettings::for_target(VcfTarget::Kanun),
            kick: VcfSettings::for_target(VcfTarget::Kick),
        }
    }
}

impl VcfBank {
    pub fn get(self, target: VcfTarget) -> VcfSettings {
        match target {
            VcfTarget::All => self.all,
            VcfTarget::Bass => self.bass,
            VcfTarget::Kanun => self.kanun,
            VcfTarget::Kick => self.kick,
        }
    }

    pub fn apply(&mut self, mut setting: VcfSettings) {
        self.focus = setting.target;
        match setting.target {
            VcfTarget::All => {
                self.all = setting;
                if setting.enabled {
                    self.bass.enabled = false;
                    self.kanun.enabled = false;
                    self.kick.enabled = false;
                } else {
                    self.bass.enabled = false;
                    self.kanun.enabled = false;
                    self.kick.enabled = false;
                }
            }
            VcfTarget::Bass => {
                self.all.enabled = false;
                setting.target = VcfTarget::Bass;
                self.bass = setting;
            }
            VcfTarget::Kanun => {
                self.all.enabled = false;
                setting.target = VcfTarget::Kanun;
                self.kanun = setting;
            }
            VcfTarget::Kick => {
                self.all.enabled = false;
                setting.target = VcfTarget::Kick;
                self.kick = setting;
            }
        }
    }

    pub fn advance_tick(&mut self) {
        for target in [
            VcfTarget::All,
            VcfTarget::Bass,
            VcfTarget::Kanun,
            VcfTarget::Kick,
        ] {
            let mut setting = self.get(target);
            if setting.enabled {
                setting.cutoff_hz =
                    (setting.cutoff_hz + setting.cutoff_step_per_tick).clamp(10.0, 22_000.0);
                setting.resonance =
                    (setting.resonance + setting.resonance_step_per_tick).clamp(0.0, 0.98);
                setting.drive = (setting.drive + setting.drive_step_per_tick).clamp(0.1, 12.0);
                self.apply_to_slot(setting);
            }
        }
    }

    fn apply_to_slot(&mut self, setting: VcfSettings) {
        match setting.target {
            VcfTarget::All => self.all = setting,
            VcfTarget::Bass => self.bass = setting,
            VcfTarget::Kanun => self.kanun = setting,
            VcfTarget::Kick => self.kick = setting,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MoogLadder {
    sample_rate: f32,
    cutoff_hz: f32,
    target_cutoff_hz: f32,
    resonance: f32,
    drive: f32,
    z1: f32,
    z2: f32,
    z3: f32,
    z4: f32,
    cutoff_smooth_coeff: f32,
}

impl MoogLadder {
    pub fn new(sample_rate: f32) -> Self {
        let mut f = Self {
            sample_rate: sample_rate.max(1.0),
            cutoff_hz: VcfSettings::default().cutoff_hz,
            target_cutoff_hz: VcfSettings::default().cutoff_hz,
            resonance: VcfSettings::default().resonance,
            drive: VcfSettings::default().drive,
            z1: 0.0,
            z2: 0.0,
            z3: 0.0,
            z4: 0.0,
            cutoff_smooth_coeff: 0.0,
        };
        f.recompute_smoothing();
        f
    }

    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
        self.z3 = 0.0;
        self.z4 = 0.0;
    }

    pub fn set_settings(&mut self, s: VcfSettings) {
        self.update_settings(s);
        self.reset();
    }

    pub fn update_settings(&mut self, s: VcfSettings) {
        self.set_cutoff_hz(s.cutoff_hz);
        self.set_resonance(s.resonance);
        self.set_drive(s.drive);
    }

    pub fn set_cutoff_hz(&mut self, hz: f32) {
        let nyquist_safe = self.sample_rate * 0.45;
        self.target_cutoff_hz = hz.clamp(10.0, nyquist_safe);
    }

    pub fn set_resonance(&mut self, resonance_0_to_1: f32) {
        self.resonance = resonance_0_to_1.clamp(0.0, 0.98);
    }

    pub fn set_drive(&mut self, drive: f32) {
        self.drive = drive.clamp(0.1, 12.0);
    }

    #[inline]
    pub fn process(&mut self, input: f32) -> f32 {
        self.cutoff_hz = self.target_cutoff_hz
            + self.cutoff_smooth_coeff * (self.cutoff_hz - self.target_cutoff_hz);

        let cutoff = self.cutoff_hz.clamp(10.0, self.sample_rate * 0.45);
        let g =
            (1.0 - (-std::f32::consts::TAU * cutoff / self.sample_rate).exp()).clamp(0.0001, 0.98);
        let k = self.resonance * 1.65;

        let x = (input - k * self.z4).clamp(-8.0, 8.0);
        let mut y = (x * self.drive).tanh();

        y = one_pole(y, &mut self.z1, g);
        y = soft_stage(y);
        y = one_pole(y, &mut self.z2, g);
        y = soft_stage(y);
        y = one_pole(y, &mut self.z3, g);
        y = soft_stage(y);
        y = one_pole(y, &mut self.z4, g);

        let makeup = 1.0 + 0.15 * self.resonance;
        (y * makeup).clamp(-1.0, 1.0)
    }

    fn recompute_smoothing(&mut self) {
        let tau_seconds = 0.005;
        self.cutoff_smooth_coeff = (-1.0 / (tau_seconds * self.sample_rate)).exp();
    }
}

#[inline]
fn one_pole(input: f32, z: &mut f32, g: f32) -> f32 {
    *z += g * (input - *z);
    *z *= 0.9995;
    *z
}

#[inline]
fn soft_stage(x: f32) -> f32 {
    x.tanh()
}

#[cfg(test)]
mod tests {
    use super::{MoogLadder, VcfSettings, VcfTarget, VcoWave};

    #[test]
    fn resonant_filter_decays_after_impulse() {
        let mut filter = MoogLadder::new(44_100.0);
        filter.set_settings(VcfSettings {
            enabled: true,
            target: VcfTarget::All,
            cutoff_hz: 900.0,
            resonance: 0.65,
            drive: 3.5,
            cutoff_step_per_tick: 0.0,
            resonance_step_per_tick: 0.0,
            drive_step_per_tick: 0.0,
            wave: VcoWave::Sin,
        });

        let mut max_tail = 0.0f32;
        let _ = filter.process(1.0);
        for i in 0..(44_100 * 3) {
            let y = filter.process(0.0).abs();
            if i > 44_100 {
                max_tail = max_tail.max(y);
            }
        }

        assert!(max_tail < 0.001, "filter tail did not decay: {max_tail}");
    }
}
