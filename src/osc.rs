// src/osc.rs — simple oscillator waveforms for experiments

#[inline]
pub fn wrap_phase(phase: f32) -> f32 { phase - phase.floor() }

#[inline]
pub fn advance_phase(phase: &mut f32, freq_hz: f32, sample_rate: f32) {
    *phase = wrap_phase(*phase + freq_hz / sample_rate.max(1.0));
}

#[inline]
pub fn sin_wave(phase: f32) -> f32 { (std::f32::consts::TAU * wrap_phase(phase)).sin() }

#[inline]
pub fn saw_wave(phase: f32) -> f32 { 2.0 * wrap_phase(phase) - 1.0 }

#[inline]
pub fn tri_wave(phase: f32) -> f32 { 1.0 - 4.0 * (wrap_phase(phase) - 0.5).abs() }

#[inline]
pub fn sq_wave(phase: f32) -> f32 { if wrap_phase(phase) < 0.5 { 1.0 } else { -1.0 } }

#[inline]
pub fn pulse_wave(phase: f32, duty: f32) -> f32 {
    if wrap_phase(phase) < duty.clamp(0.01, 0.99) { 1.0 } else { -1.0 }
}

#[derive(Clone, Copy, Debug)]
pub enum Waveform { Sin, Saw, Tri, Sq, Pulse { duty: f32 } }

impl Waveform {
    #[inline]
    pub fn sample(self, phase: f32) -> f32 {
        match self {
            Waveform::Sin => sin_wave(phase),
            Waveform::Saw => saw_wave(phase),
            Waveform::Tri => tri_wave(phase),
            Waveform::Sq => sq_wave(phase),
            Waveform::Pulse { duty } => pulse_wave(phase, duty),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Osc {
    pub phase: f32,
    pub freq_hz: f32,
    pub sample_rate: f32,
    pub waveform: Waveform,
}

impl Osc {
    pub fn new(sample_rate: f32, freq_hz: f32, waveform: Waveform) -> Self {
        Self { phase: 0.0, freq_hz, sample_rate: sample_rate.max(1.0), waveform }
    }
    pub fn set_freq_hz(&mut self, freq_hz: f32) { self.freq_hz = freq_hz.max(0.0); }
    pub fn set_sample_rate(&mut self, sample_rate: f32) { self.sample_rate = sample_rate.max(1.0); }
    pub fn set_waveform(&mut self, waveform: Waveform) { self.waveform = waveform; }
    #[inline]
    pub fn next(&mut self) -> f32 {
        let y = self.waveform.sample(self.phase);
        advance_phase(&mut self.phase, self.freq_hz, self.sample_rate);
        y
    }
}
