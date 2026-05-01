// synth.rs — shared voice synthesis, PRNG, and melody evolution
// Used by both audio.rs (real-time) and record.rs (offline).

use std::sync::atomic::{AtomicU64, Ordering};
use crate::sequencer::{Bar, SubdivEvent};

// ── PRNG ─────────────────────────────────────────────────────────────────────

static RNG: AtomicU64 = AtomicU64::new(0xdeadbeef_cafef00d);

pub fn next_u64() -> u64 {
    let s = RNG.fetch_add(6_364_136_223_846_793_005, Ordering::Relaxed)
              .wrapping_mul(2_862_933_555_777_941_757)
              .wrapping_add(3_037_000_493);
    RNG.store(s, Ordering::Relaxed);
    s
}
pub fn rand_f32()          -> f32  { (next_u64() >> 33) as f32 / (1u32 << 31) as f32 - 1.0 }
pub fn rand_f32_01()       -> f32  { (next_u64() >> 33) as f32 / (u32::MAX as f32) }
pub fn rand_bool(p: f32)   -> bool { rand_f32_01() < p }
pub fn rand_range(lo: usize, hi: usize) -> usize {
    if lo >= hi { lo } else { lo + (next_u64() as usize % (hi - lo)) }
}

// ── Group-level degree expansion ──────────────────────────────────────────────

/// Zigzag walk: n_groups+1 waypoints alternating 0 / peak / 0 / peak …
/// Always starts and ends at 0 (tonic). Single group: [0, peak] so snares
/// ascend; next cycle's first kick brings it back to tonic.
pub fn zigzag_walk(n_groups: usize, peak: usize) -> Vec<usize> {
    let peak = peak.min(4);
    if n_groups == 0 { return vec![0]; }
    let mut w = vec![0usize; n_groups + 1];
    // interior waypoints alternate: odd indices → peak, even indices → peak/2
    for i in 1..n_groups {
        w[i] = if i % 2 == 1 { peak } else { (peak / 2).max(1) };
    }
    // single group special case: endpoint is peak so snares actually go somewhere
    if n_groups == 1 { w[1] = peak; }
    // last waypoint is always 0 — the turnaround lands on tonic
    *w.last_mut().unwrap() = 0;
    w
}

/// Expand waypoints (n_groups+1 entries) to per-subdivision degrees.
/// Kick on each group start gets waypoints[gi].
/// Snares interpolate linearly toward waypoints[gi+1].
/// Direction changes at every kick — follows the rhythm.
pub fn expand_degrees(waypoints: &[usize], groups: &[u8]) -> Vec<usize> {
    let mut result = Vec::new();
    for (gi, &g) in groups.iter().enumerate() {
        let from = waypoints.get(gi).copied().unwrap_or(0);
        let to   = waypoints.get(gi + 1).copied().unwrap_or(from);
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

/// Called after each full phrase play.
/// `is_last_bar` — true for the last bar in a phrase (turnaround treatment).
pub fn evolve_bar(bar: &mut Bar, is_last_bar: bool) {
    let n      = bar.group_degrees.len();  // n_groups+1 waypoints
    let n_freq = bar.frequencies.len();
    if n < 2 || n_freq == 0 { return; }

    // Peak is the midpoint of the combined scale
    let mid_freq = (n_freq / 2).max(1);

    let r = rand_f32_01();
    if r < 0.08 {
        // Reset: fresh zigzag through combined scale
        let peak = rand_range(mid_freq.saturating_sub(1), (mid_freq + 2).min(n_freq - 1));
        bar.group_degrees = zigzag_walk(bar.groups.len(), peak);
    } else if r < 0.20 {
        // Fill: push peaks slightly higher in the combined scale
        for i in 1..(n.saturating_sub(1)) {
            if bar.group_degrees[i] > 0 {
                bar.group_degrees[i] = (bar.group_degrees[i] + 1).min(n_freq - 1);
            }
        }
    } else {
        // Gentle drift: nudge peaks ±1 in the scale
        for i in 1..(n.saturating_sub(1)) {
            if bar.group_degrees[i] > 0 && rand_bool(0.15) {
                let shift: i32 = if rand_bool(0.5) { 1 } else { -1 };
                bar.group_degrees[i] = (bar.group_degrees[i] as i32 + shift)
                    .clamp(1, n_freq as i32 - 1) as usize;
            }
        }
    }

    // Anchor: first and last waypoints always tonic (index 0)
    if let Some(f) = bar.group_degrees.first_mut() { *f = 0; }
    if let Some(l) = bar.group_degrees.last_mut()  { *l = 0; }

    // Turnaround: penultimate waypoint rises to midpoint before phrase end
    if is_last_bar && n >= 3 && rand_bool(0.50) {
        bar.group_degrees[n - 2] = mid_freq;
    }

    bar.degrees = expand_degrees(&bar.group_degrees, &bar.groups);
    bar.recompute_events();
}

// ── Voice ─────────────────────────────────────────────────────────────────────

pub enum VoiceKind { Kick, Snare, MelodyFm, Accent, SubBass }

pub struct Voice {
    pub kind:         VoiceKind,
    pub age:          usize,
    pub freq:         f64,
    pub phase:        f64,
    #[allow(dead_code)]
    pub mod_phase:    f64,
    pub sustain_secs: f64,
    pub gain_override: Option<f32>,   // overrides default per-kind gain
    pub pan:          f32,             // stereo position: -0.5 left … +0.5 right
    pub done:         bool,
}

impl Voice {
    pub fn kick()                    -> Self { Self::mk(VoiceKind::Kick,    80.0, 0.0) }
    pub fn snare()                   -> Self { Self::mk(VoiceKind::Snare,    0.0, 0.0) }
    pub fn melody(hz: f64, sus: f64) -> Self { Self::mk(VoiceKind::MelodyFm, hz, sus)  }
    pub fn melody_gain(hz: f64, sus: f64, gain: f32) -> Self {
        let mut v = Self::mk(VoiceKind::MelodyFm, hz, sus);
        v.gain_override = Some(gain);
        v
    }
    pub fn accent(hz: f64)           -> Self { Self::mk(VoiceKind::Accent,   hz, 0.0)  }

    fn mk(kind: VoiceKind, freq: f64, sustain_secs: f64) -> Self {
        Voice { kind, age: 0, freq, phase: 0.0, mod_phase: 0.0,
                sustain_secs, gain_override: None, pan: 0.0, done: false }
    }

    pub fn sample(&mut self, sr: f64) -> f32 {
        let dt = 1.0 / sr;
        let t  = self.age as f64 * dt;

        let (osc, amp, fin): (f32, f32, bool) = match self.kind {
            VoiceKind::Kick => {
                let freq = self.freq * (1.0 + 4.0 * (-t * 28.0).exp());
                self.phase += freq * dt;
                ((self.phase * std::f64::consts::TAU).sin() as f32,
                 (-t * 11.0).exp() as f32, t > 0.40)
            }
            VoiceKind::Snare => {
                (rand_f32(), (-t * 28.0).exp() as f32, t > 0.14)
            }
            VoiceKind::Accent => {
                // Short high-pitched click — marks the turnaround
                let freq = self.freq * 2.0;
                self.phase += freq * dt;
                let osc = (self.phase * std::f64::consts::TAU).sin() as f32;
                (osc, (-t * 40.0).exp() as f32, t > 0.08)
            }
            VoiceKind::SubBass => {
                // Pure sine, two octaves below root, slow attack, holds phrase
                let sus = self.sustain_secs;
                self.phase += self.freq * dt;
                let osc = (self.phase * std::f64::consts::TAU).sin() as f32;
                let amp = if t < 0.20 {
                    (t / 0.20) as f32          // 200ms attack
                } else if t < sus {
                    1.0f32
                } else if t < sus + 0.80 {
                    (1.0 - (t - sus) / 0.80).max(0.0) as f32
                } else {
                    0.0
                };
                (osc, amp, t > sus + 0.85)
            }
            VoiceKind::MelodyFm => {
                let sus = self.sustain_secs;
                // Additive synthesis with exact JI overtones — stays perfectly in tune.
                // Fundamental + 2:1 octave (soft) + 3:1 twelfth (very soft).
                // All ratios are JI so the only beating is between voices, which
                // is the beautiful JI shimmer we want.
                self.phase += self.freq * dt;
                let p = self.phase * std::f64::consts::TAU;
                let osc = (p.sin()                      // fundamental
                         + p.mul_add(2.0, 0.0).sin() * 0.25  // octave  2:1
                         + p.mul_add(3.0, 0.0).sin() * 0.10  // twelfth 3:1
                         ) as f32 / 1.35; // normalise

                let amp = if t < 0.015 { (t / 0.015) as f32 }
                    else if t < 0.20   { (1.0 - (t-0.015)/0.185*0.5) as f32 }
                    else if t < sus    { 0.50 }
                    else if t < sus+0.5{ (0.5*(1.0-(t-sus)/0.5)).max(0.0) as f32 }
                    else               { 0.0 };
                (osc, amp, t > sus + 0.55)
            }
        };

        self.age += 1;
        if fin { self.done = true; }
        let gain: f32 = self.gain_override.unwrap_or_else(|| match self.kind {
            VoiceKind::Kick     => 0.40,
            VoiceKind::Snare    => 0.16,
            VoiceKind::MelodyFm => 0.20,
            VoiceKind::Accent   => 0.35,
            VoiceKind::SubBass  => 0.30,
        });
        (osc * amp * gain).clamp(-1.0, 1.0)
    }
}

/// Spawn voices for an event; push into the provided vec.
pub fn spawn_voices(event: SubdivEvent, sustain: f64, voices: &mut Vec<Voice>, turnaround: bool) {
    match event {
        SubdivEvent::Kick(hz) => {
            voices.push(Voice::kick());
            voices.push(Voice::melody(hz, sustain));
            if turnaround { voices.push(Voice::accent(hz)); }
        }
        SubdivEvent::Snare(hz) => {
            voices.push(Voice::snare());
            voices.push(Voice::melody(hz, sustain));
        }
    }
}

/// Spawn a soft independent arpeggio voice (not tied to a rhythm event).
#[allow(dead_code)]
pub fn spawn_arp_voice(hz: f64, sustain: f64, voices: &mut Vec<Voice>) {
    voices.push(Voice::melody_gain(hz, sustain, 0.12));
}

/// Long root-note voice for phrase start — the highest-level melody event.
/// Marks "we are back at the beginning of the cycle."
pub fn spawn_phrase_start(hz: f64, sustain: f64, voices: &mut Vec<Voice>) {
    // Root at full gain, extra-long sustain — rings through the coming phrase
    voices.push(Voice::melody_gain(hz, sustain * 2.5, 0.22));
    // JI octave below (0.5) — ground the tonic in the bass
    voices.push(Voice::melody_gain(hz * 0.5, sustain * 2.0, 0.14));
}

/// Spawn a chain of sub-bass octaves — each one octave lower than the last,
/// at decreasing gain, until the frequency is inaudible.
/// The lowest partials add physical pressure even when below hearing threshold.
pub fn spawn_sub_bass(root_hz: f64, phrase_secs: f64, voices: &mut Vec<Voice>) {
    // Octave chain: 1× down to 1/32×.
    // Gains peak at /4 (two octaves below root) and taper in both directions.
    //   root×1  — soft presence in melody range
    //   root/2  — upper bass
    //   root/4  — main boom (loudest)
    //   root/8  — deep sub
    //   root/16 — infrasonic weight
    //   root/32 — pressure only
    let octaves: &[(f64, f32)] = &[
        (1.0,       0.04),
        (0.5,       0.10),
        (0.25,      0.15),   // peak — two octaves below root
        (0.125,     0.11),
        (0.0625,    0.06),
        (0.03125,   0.02),
    ];
    for &(factor, gain) in octaves {
        let freq = root_hz * factor;
        if freq < 8.0 { break; }   // below useful range even for speakers
        let mut v = Voice::mk(VoiceKind::SubBass, freq, phrase_secs);
        v.gain_override = Some(gain);
        voices.push(v);
    }
}
