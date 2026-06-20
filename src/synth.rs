// synth.rs — shared voice synthesis, PRNG, and melody evolution

use crate::sequencer::{Bar, SubdivEvent};
use std::sync::atomic::{AtomicU64, Ordering};

// ── PRNG ─────────────────────────────────────────────────────────────────────

static RNG: AtomicU64 = AtomicU64::new(0xdeadbeef_cafef00d);

pub fn next_u64() -> u64 {
    let s = RNG
        .fetch_add(6_364_136_223_846_793_005, Ordering::Relaxed)
        .wrapping_mul(2_862_933_555_777_941_757)
        .wrapping_add(3_037_000_493);
    RNG.store(s, Ordering::Relaxed);
    s
}
pub fn rand_f32() -> f32 {
    (next_u64() >> 33) as f32 / (1u32 << 31) as f32 - 1.0
}
pub fn rand_f32_01() -> f32 {
    (next_u64() >> 33) as f32 / (u32::MAX as f32)
}
pub fn rand_bool(p: f32) -> bool {
    rand_f32_01() < p
}
pub fn rand_range(lo: usize, hi: usize) -> usize {
    if lo >= hi {
        lo
    } else {
        lo + (next_u64() as usize % (hi - lo))
    }
}

// ── Group-level degree expansion ──────────────────────────────────────────────

pub fn zigzag_walk(n_groups: usize, peak: usize) -> Vec<usize> {
    let peak = peak.min(4);
    if n_groups == 0 {
        return vec![0];
    }
    let mut w = vec![0usize; n_groups + 1];
    for i in 1..n_groups {
        w[i] = if i % 2 == 1 { peak } else { (peak / 2).max(1) };
    }
    if n_groups == 1 {
        w[1] = peak;
    }
    *w.last_mut().unwrap() = 0;
    w
}

pub fn expand_degrees(waypoints: &[usize], groups: &[u8]) -> Vec<usize> {
    let mut result = Vec::new();
    for (gi, &g) in groups.iter().enumerate() {
        let from = waypoints.get(gi).copied().unwrap_or(0);
        let to = waypoints.get(gi + 1).copied().unwrap_or(from);
        result.push(from);
        for s in 1..g as usize {
            let t = s as f64 / g as f64;
            let d = from as f64 + t * (to as f64 - from as f64);
            result.push((d.round() as usize).min(7));
        }
    }
    result
}

// ── Evolution ─────────────────────────────────────────────────────────────────

pub fn evolve_bar(bar: &mut Bar, is_last_bar: bool) {
    let n = bar.group_degrees.len();
    let n_freq = bar.frequencies.len();
    if n < 2 || n_freq == 0 {
        return;
    }

    let mid_freq = (n_freq / 2).max(1);

    let r = rand_f32_01();
    if r < 0.08 {
        let peak = rand_range(mid_freq.saturating_sub(1), (mid_freq + 2).min(n_freq - 1));
        bar.group_degrees = zigzag_walk(bar.groups.len(), peak);
    } else if r < 0.20 {
        for i in 1..(n.saturating_sub(1)) {
            if bar.group_degrees[i] > 0 {
                bar.group_degrees[i] = (bar.group_degrees[i] + 1).min(n_freq - 1);
            }
        }
    } else {
        for i in 1..(n.saturating_sub(1)) {
            if bar.group_degrees[i] > 0 && rand_bool(0.15) {
                let shift: i32 = if rand_bool(0.5) { 1 } else { -1 };
                bar.group_degrees[i] =
                    (bar.group_degrees[i] as i32 + shift).clamp(1, n_freq as i32 - 1) as usize;
            }
        }
    }

    if let Some(f) = bar.group_degrees.first_mut() {
        *f = 0;
    }
    if let Some(l) = bar.group_degrees.last_mut() {
        *l = 0;
    }

    if is_last_bar && n >= 3 && rand_bool(0.50) {
        bar.group_degrees[n - 2] = mid_freq;
    }

    bar.degrees = expand_degrees(&bar.group_degrees, &bar.groups);
    bar.recompute_events();
}

// ── Voice ─────────────────────────────────────────────────────────────────────

pub enum VoiceKind {
    FloorTom,
    Snare,
    Crash,
    PhraseChange,
    MelodyFm,
    SubBass,
    Rimshot,
    Accent,
}

pub struct Voice {
    pub kind: VoiceKind,
    pub age: usize,
    pub freq: f64,
    pub phase: f64,
    pub mod_phase: f64,
    pub sustain_secs: f64,
    pub gain_override: Option<f32>,
    pub pan: f32,
    pub release_frames: Option<usize>,
    pub done: bool,
}

impl Voice {
    #[allow(dead_code)]
    pub fn floor_tom() -> Self {
        Self::mk(VoiceKind::FloorTom, 40.0, 0.0)
    }
    #[allow(dead_code)]
    pub fn kick() -> Self {
        Self::mk(VoiceKind::FloorTom, 40.0, 0.0)
    }
    pub fn snare() -> Self {
        Self::mk(VoiceKind::Snare, 0.0, 0.0)
    }
    pub fn melody(hz: f64, sus: f64) -> Self {
        Self::mk(VoiceKind::MelodyFm, hz, sus)
    }
    pub fn melody_gain(hz: f64, sus: f64, gain: f32) -> Self {
        let mut v = Self::mk(VoiceKind::MelodyFm, hz, sus);
        v.gain_override = Some(gain);
        v
    }
    #[allow(dead_code)]
    pub fn accent(hz: f64) -> Self {
        Self::mk(VoiceKind::Accent, hz, 0.0)
    }

    fn mk(kind: VoiceKind, freq: f64, sustain_secs: f64) -> Self {
        Voice {
            kind,
            age: 0,
            freq,
            phase: 0.0,
            mod_phase: 0.0,
            sustain_secs,
            gain_override: None,
            pan: 0.0,
            release_frames: None,
            done: false,
        }
    }

    pub fn sample(&mut self, sr: f64) -> f32 {
        let dt = 1.0 / sr;
        let t = self.age as f64 * dt;

        let (osc, amp, fin): (f32, f32, bool) = match self.kind {
            VoiceKind::FloorTom => {
                let freq = self.freq * (1.0 + 1.8 * (-t * 12.0).exp());
                self.phase += freq * dt;
                let osc = (self.phase * std::f64::consts::TAU).sin() as f32;
                let amp = (-t * 7.0).exp() as f32;
                (osc, amp, t > 0.55)
            }
            VoiceKind::Rimshot => {
                let freq = self.freq * (1.0 + 1.8 * (-t * 12.0).exp()) * 0.25;
                self.phase += freq * dt;
                let osc = (self.phase * std::f64::consts::TAU).sin() as f32;
                let amp = (-t * 7.0).exp() as f32;
                (osc, amp, t > 0.55)
            }
            VoiceKind::Crash => {
                let shimmer_freq = self.freq * 10.0;
                self.phase += shimmer_freq * dt;
                let shimmer = (self.phase * std::f64::consts::TAU).sin() as f32;
                let noise = rand_f32();
                let osc = noise * 0.7 + shimmer * 0.3;
                let amp = if t < 0.004 {
                    (t / 0.004) as f32
                } else {
                    (-t * 5.0).exp() as f32
                };
                (osc, amp, t > 0.80)
            }
            VoiceKind::PhraseChange => {
                let freq = self.freq * 3.0 * (1.0 + 1.5 * (-t * 20.0).exp());
                self.phase += freq * dt;
                let osc = (self.phase * std::f64::consts::TAU).sin() as f32;
                let amp = (-t * 14.0).exp() as f32;
                (osc, amp, t > 0.25)
            }
            VoiceKind::Snare => (rand_f32(), (-t * 28.0).exp() as f32, t > 0.14),
            VoiceKind::Accent => {
                let noise = rand_f32();
                let shimmer_freq = self.freq * 6.0;
                self.phase += shimmer_freq * dt;
                let shimmer = (self.phase * std::f64::consts::TAU).sin() as f32;
                let osc = noise * 0.75 + shimmer * 0.25;
                let amp = if t < 0.003 {
                    (t / 0.003) as f32
                } else if t < 0.04 {
                    (1.0 - (t - 0.003) / 0.037) as f32 * 0.9 + 0.1
                } else if t < 0.18 {
                    (0.1 * (1.0 - (t - 0.04) / 0.14)).max(0.0) as f32
                } else {
                    0.0
                };
                (osc, amp, t > 0.20)
            }
            VoiceKind::SubBass => {
                let sus = self.sustain_secs;
                self.phase += self.freq * dt;
                let osc = (self.phase * std::f64::consts::TAU).sin() as f32;
                let amp = if t < 0.10 {
                    (t / 0.10) as f32
                } else if t < sus {
                    1.0f32
                } else if t < sus + 0.20 {
                    (1.0 - (t - sus) / 0.20).max(0.0) as f32
                } else {
                    0.0
                };
                (osc, amp, t > sus + 0.25)
            }
            VoiceKind::MelodyFm => {
                let sus = self.sustain_secs;
                self.phase += self.freq * dt;
                let p = self.phase * std::f64::consts::TAU;
                let osc = (p.sin()
                    + p.mul_add(2.0, 0.0).sin() * 0.25
                    + p.mul_add(3.0, 0.0).sin() * 0.10) as f32
                    / 1.35;
                let amp = if t < 0.015 {
                    (t / 0.015) as f32
                } else if t < 0.20 {
                    (1.0 - (t - 0.015) / 0.185 * 0.5) as f32
                } else if t < sus {
                    0.50
                } else if t < sus + 0.5 {
                    (0.5 * (1.0 - (t - sus) / 0.5)).max(0.0) as f32
                } else {
                    0.0
                };
                (osc, amp, t > sus + 0.55)
            }
        };

        self.age += 1;
        if fin {
            self.done = true;
        }

        let release_gain = if let Some(rf) = self.release_frames {
            if rf == 0 {
                self.done = true;
                0.0
            } else {
                let g = (rf as f32 / 441.0).min(1.0);
                self.release_frames = Some(rf.saturating_sub(1));
                g
            }
        } else {
            1.0
        };

        let gain: f32 = self.gain_override.unwrap_or_else(|| match self.kind {
            VoiceKind::FloorTom => 0.65,
            VoiceKind::Snare => 0.28,
            VoiceKind::Rimshot => 0.45,
            VoiceKind::Crash => 0.42,
            VoiceKind::PhraseChange => 0.50,
            VoiceKind::MelodyFm => 0.20,
            VoiceKind::Accent => 0.35,
            VoiceKind::SubBass => 0.50,
        });
        (osc * amp * gain * release_gain).clamp(-1.0, 1.0)
    }
}

// ── Milestone ─────────────────────────────────────────────────────────────────

/// Structural event this subdivision marks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Milestone {
    None,
    Turnaround,         // last beat of last repeat, same phrase loops next
    CrossPhraseWarning, // last beat of last repeat, different phrase comes next
    PhraseStart,
    PhraseChange,
}

// ── Voice spawning ────────────────────────────────────────────────────────────

fn snap_to_scale(hz: f64, scale: &[f64]) -> f64 {
    if scale.is_empty() {
        return hz;
    }
    let mut best = hz;
    let mut best_dist = f64::MAX;
    for &f in scale {
        for octave in -2i32..=2 {
            let candidate = f * 2f64.powi(octave);
            let dist = (candidate / hz).log2().abs();
            if dist < best_dist {
                best_dist = dist;
                best = candidate;
            }
        }
    }
    best
}

pub fn spawn_voices(
    event: SubdivEvent,
    sustain: f64,
    voices: &mut Vec<Voice>,
    milestone: Milestone,
    scale: &[f64],
    root_hz: f64,
    subdiv_secs: f64,
) {
    // Milestone sounds fire regardless of whether the event is a kick or snare.
    // Previously these were inside the Kick arm, so rhythms ending on '.' (snare)
    // never triggered the turnaround/warning sounds.
    match milestone {
        Milestone::Turnaround => {
            // Same phrase repeating — double kick, no leading tone
            let mut tom2 = Voice::mk(VoiceKind::FloorTom, 35.0, 0.2);
            tom2.pan = 0.0;
            voices.push(tom2);
            // Yeden ... use a fourth down
            let lt_hz = root_hz * 0.5 * 3.0 / 4.0;
            // we should never be using absolute time units. it must be in terms of ticks
            let mut lt = Voice::mk(VoiceKind::SubBass, lt_hz, subdiv_secs * 0.5);
            lt.gain_override = Some(0.45);
            lt.pan = 0.0;
            voices.push(lt);
        }
        Milestone::CrossPhraseWarning => {
            // repeat
            let mut tom1 = Voice::mk(VoiceKind::FloorTom, 60.0, 0.25);
            tom1.pan = 0.0;
            voices.push(tom1);
        }
        Milestone::PhraseStart => {
            let mut v = Voice::mk(VoiceKind::Crash, 400.0, 0.3);
            v.pan = 0.0;
            voices.push(v);
        }
        Milestone::PhraseChange => {
            let mut v = Voice::mk(VoiceKind::PhraseChange, 60.0, 0.2);
            v.pan = 0.0;
            voices.push(v);
        }
        Milestone::None => {}
    }

    match event {
        SubdivEvent::Kick(hz) => {
            let mut tom = Voice::mk(VoiceKind::FloorTom, 40.0, 0.25);
            tom.pan = 0.0;
            voices.push(tom);
            voices.push(panned(Voice::melody(hz, sustain)));
            let octave2 = snap_to_scale(root_hz / 8.0, scale);
            let octave = snap_to_scale(root_hz / 4.0, scale);
            voices.push(panned(Voice::melody_gain(octave2, sustain * 0.85, 0.1)));
            voices.push(panned(Voice::melody_gain(octave, sustain * 0.20, 0.1)));
            // Root bass on every kick, one octave down, held one subdivision
            let bass_freq = root_hz * 0.5;
            // again... no absolute time should be used. we must use ticks
            let mut bass = Voice::mk(VoiceKind::SubBass, bass_freq, subdiv_secs * 0.95);
            bass.gain_override = Some(0.15);
            bass.pan = 0.0;
            voices.push(bass);
        }
        SubdivEvent::Snare(hz) => {
            voices.push(panned(Voice::snare()));
            voices.push(panned(Voice::melody(hz, sustain)));
            let fifth = snap_to_scale(hz * 1.5, scale);
            voices.push(panned(Voice::melody_gain(fifth, sustain * 0.50, 0.06)));
        }
    }
}

fn panned(mut v: Voice) -> Voice {
    v.pan = (rand_f32_01() - 0.5) * 1.8;
    v
}

#[allow(dead_code)]
pub fn spawn_arp_voice(hz: f64, sustain: f64, voices: &mut Vec<Voice>) {
    voices.push(Voice::melody_gain(hz, sustain, 0.12));
}

pub fn spawn_phrase_start(hz: f64, sustain: f64, voices: &mut Vec<Voice>) {
    voices.push(Voice::melody_gain(hz, sustain * 2.5, 0.22));
    voices.push(Voice::melody_gain(hz * 0.5, sustain * 2.0, 0.14));
}

pub fn spawn_sub_bass(root_hz: f64, phrase_secs: f64, voices: &mut Vec<Voice>) {
    let octaves: &[(f64, f32)] = &[
        (1.0, 0.04),
        (0.5, 0.10),
        (0.25, 0.15),
        (0.125, 0.11),
        (0.0625, 0.06),
        (0.03125, 0.02),
    ];
    for &(factor, gain) in octaves {
        let freq = root_hz * factor;
        if freq < 8.0 {
            break;
        }
        let mut v = Voice::mk(VoiceKind::SubBass, freq, phrase_secs);
        v.gain_override = Some(gain);
        voices.push(v);
    }
}
