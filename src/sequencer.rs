// sequencer.rs — phrase/bar data structures and audio commands
//
// Comma-separated ajnas form ONE combined scale (stacked, not sequential).
// The melody walks through all frequencies of all ajnas together.

use crate::tuning::{Maqam, Pitch};
use crate::synth::{expand_degrees, zigzag_walk};

// ── Bar event ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub enum SubdivEvent {
    Kick(f64),
    Snare(f64),
}

// ── Bar ───────────────────────────────────────────────────────────────────────
// One bar represents the full combined scale of all ajnas in a phrase.

#[derive(Debug, Clone)]
pub struct Bar {
    pub root:          Pitch,          // original pitch (for display)
    pub root_hz:       f64,            // actual 5-limit snapped root Hz
    #[allow(dead_code)]
    pub maqam:         Maqam,          // first jins maqam — for display
    pub maqam_names:   Vec<String>,    // all jins names — for display
    pub groups:        Vec<u8>,        // rhythm groups
    pub frequencies:   Vec<f64>,       // combined JI scale, sorted ascending
    pub group_degrees: Vec<usize>,     // waypoints (n_groups+1), index into frequencies
    pub degrees:       Vec<usize>,     // per-subdivision index into frequencies
    pub events:        Vec<SubdivEvent>,
    pub total_subdivs: usize,
}

impl Bar {
    pub fn recompute_events(&mut self) {
        self.degrees = expand_degrees(&self.group_degrees, &self.groups);
        self.events  = events_from_freqs(&self.degrees, &self.frequencies, &self.groups);
    }

    pub fn rhythm_display(&self) -> String {
        self.events.iter().map(|e| match e {
            SubdivEvent::Kick(_)  => 'X',
            SubdivEvent::Snare(_) => '.',
        }).collect()
    }
}

// ── Phrase ────────────────────────────────────────────────────────────────────

/// A jump instruction stored as a sequence entry.
#[derive(Debug, Clone)]
pub struct JumpSpec {
    pub to_pos: usize,
    pub times:  usize,
}

#[derive(Debug, Clone)]
pub struct Phrase {
    pub id:     usize,
    pub src:    String,
    pub bar:    Bar,
    pub repeat: usize,
    /// If Some, this is a control-flow entry (no audio).
    pub jump:   Option<JumpSpec>,
}

impl Phrase {
    pub fn rhythm_display(&self) -> String { self.bar.rhythm_display() }
    #[allow(dead_code)]
    pub fn is_jump(&self) -> bool { self.jump.is_some() }
}

/// Build a jump-entry phrase (no audio — pure sequencer control flow).
pub fn build_jump_entry(id: usize, to_pos: usize, times: usize) -> Phrase {
    use crate::tuning::{Maqam, Pitch};
    // Empty bar — zero subdivisions, never played
    let bar = Bar {
        root: Pitch { letter: 'd', accidental: 0, octave: 4 },
        root_hz: 293.6648,
        maqam: Maqam::Nahawand,
        maqam_names: vec![],
        groups: vec![],
        frequencies: vec![],
        group_degrees: vec![],
        degrees: vec![],
        events: vec![],
        total_subdivs: 0,
    };
    Phrase {
        id,
        src: format!(">>{to_pos} x{times}"),   // ASCII: >>pos xtimes
        bar,
        repeat: 1,
        jump: Some(JumpSpec { to_pos, times }),
    }
}

// ── Builder ───────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct BarSpec {
    pub src:    String,
    pub root:   Pitch,
    pub maqam:  Maqam,
    pub groups: Vec<u8>,
}

fn snap_to_5limit_from_d(nominal: f64) -> f64 {
    let raw_ratio = nominal / D_REFERENCE_HZ;
    let octaves   = raw_ratio.log2().floor() as i32;
    let nom_red   = raw_ratio / 2f64.powi(octaves);
    let threshold = 25.0f64 / 1200.0;  // one syntonic comma

    // Pass 1: 3-smooth (Pythagorean) — prefer these since oud strings are
    // tuned in perfect fourths. Notes reachable via 4/3 chains from D
    // should use Pythagorean tuning to resonate with open strings.
    let three_sm: Vec<u32> = (1u32..=512).filter(|&n| is_3smooth(n)).collect();
    let mut best_hz   = nominal;
    let mut best_dist = f64::MAX;
    for &p in &three_sm {
        for &q in &three_sm {
            if num_gcd(p, q) != 1 { continue; }
            let ratio   = p as f64 / q as f64;
            let exp     = ratio.log2().floor() as i32;
            let reduced = ratio / 2f64.powi(exp);
            let dist    = (reduced / nom_red).log2().abs();
            if dist < best_dist {
                best_dist = dist;
                best_hz   = D_REFERENCE_HZ * ratio * 2f64.powi(octaves - exp);
            }
            if best_dist < 1.0 / 1200.0 { return best_hz; }
        }
    }
    if best_dist < threshold { return best_hz; }  // good Pythagorean match found

    // Pass 2: 5-smooth fallback — for neutral/chromatic intervals that
    // cannot be expressed simply in Pythagorean tuning.
    let five_sm: Vec<u32> = (1u32..=64).filter(|&n| is_5smooth(n)).collect();
    for &p in &five_sm {
        for &q in &five_sm {
            if num_gcd(p, q) != 1 { continue; }
            let ratio   = p as f64 / q as f64;
            let exp     = ratio.log2().floor() as i32;
            let reduced = ratio / 2f64.powi(exp);
            let dist    = (reduced / nom_red).log2().abs();
            if dist < best_dist {
                best_dist = dist;
                best_hz   = D_REFERENCE_HZ * ratio * 2f64.powi(octaves - exp);
            }
            if best_dist < 1.0 / 1200.0 { return best_hz; }
        }
    }
    best_hz
}

fn num_gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { num_gcd(b, a % b) }
}

/// True if n has no prime factors other than 2, 3 (Pythagorean).
fn is_3smooth(mut n: u32) -> bool {
    for p in [2u32, 3] { while n % p == 0 { n /= p; } }
    n == 1
}

/// True if n has no prime factors other than 2, 3, 5.
fn is_5smooth(mut n: u32) -> bool {
    for p in [2u32, 3, 5] {
        while n % p == 0 { n /= p; }
    }
    n == 1
}

/// D4 as the universal JI reference pitch.
/// All notes are expressed as D × (5-limit ratio).
const D_REFERENCE_HZ: f64 = 293.6648_f64;  // D4 exact ET

pub fn build_phrase(
    phrase_id:   usize,
    src:         String,
    specs:       Vec<BarSpec>,
    peak_degree: usize,
    repeat:      usize,
) -> Phrase {
    assert!(!specs.is_empty());

    // ── Tetrachord stacking ───────────────────────────────────────────────
    // Each jins contributes notes only within its pitch range:
    //   jins i (0..n-2): notes in [root_i, root_{i+1})
    //   jins n-1 (last): notes in [root_{n-1}, root_0 * 2]  (the octave)
    //
    // This is the traditional maqam model — jins stack by range, not by
    // merging full octave scales. Prevents chromatic clashes like Eb/E♮.
    //   d kurd [D,A) + a kurd [A,D'] = D Phrygian ✓
    //   d nah  [D,A) + a kurd [A,D'] = D natural minor ✓

    let n_specs = specs.len();
    let roots_hz: Vec<f64> = specs.iter()
        .map(|s| snap_to_5limit_from_d(s.root.to_hz()))
        .collect();
    let upper_bound = roots_hz[0] * 2.0;

    let mut deduped: Vec<f64> = Vec::new();
    for (i, spec) in specs.iter().enumerate() {
        let root    = roots_hz[i];
        let ceiling = if i + 1 < n_specs { roots_hz[i + 1] } else { upper_bound };
        let eps     = 1.001_f64;

        let mut jins_freqs: Vec<f64> = spec.maqam.ratios().iter()
            .map(|&(p, q)| root * p as f64 / q as f64)
            .collect();
        jins_freqs.sort_by(|a, b| a.partial_cmp(b).unwrap());

        for f in jins_freqs {
            if f > ceiling * eps { continue; }
            // Dedup within 23 cents — first jins wins
            if deduped.iter().all(|&prev| (f / prev).log2().abs() > 23.0 / 1200.0) {
                deduped.push(f);
            }
        }
    }
    deduped.sort_by(|a, b| a.partial_cmp(b).unwrap());
    // Remove duplicate octave if both root and root*2 ended up in the scale
    if deduped.len() > 1 {
        let first = deduped[0];
        if (deduped.last().unwrap() / first - 2.0).abs() < 0.001 {
            // Keep the octave — it's a valid note
        }
    }
    // ── Use the groups from the last spec that has explicit rhythm ────────
    let groups = specs.last().map(|s| s.groups.clone()).unwrap_or_else(|| vec![4]);
    let n_groups = groups.len();

    // Peak index into combined scale — target dominant-ish position
    let n_freqs = deduped.len();
    let peak    = (peak_degree * n_freqs / 8).clamp(2, n_freqs.saturating_sub(1));
    let group_degrees = zigzag_walk(n_groups, peak);

    let degrees      = expand_degrees(&group_degrees, &groups);
    let total_subdivs = degrees.len();
    let events       = events_from_freqs(&degrees, &deduped, &groups);

    let maqam_names: Vec<String> = specs.iter()
        .map(|s| s.maqam.name().to_string())
        .collect();

    let root_hz_0 = snap_to_5limit_from_d(specs[0].root.to_hz());
    let bar = Bar {
        root:         specs[0].root,
        root_hz:      root_hz_0,
        maqam:        specs[0].maqam,
        maqam_names,
        groups,
        frequencies:  deduped,
        group_degrees,
        degrees,
        events,
        total_subdivs,
    };

    Phrase { id: phrase_id, src, bar, repeat, jump: None }
}

// ── Event computation ─────────────────────────────────────────────────────────

pub fn events_from_freqs(
    degrees:     &[usize],
    frequencies: &[f64],
    groups:      &[u8],
) -> Vec<SubdivEvent> {
    let n = frequencies.len();
    if n == 0 { return vec![]; }
    let mut events = Vec::with_capacity(degrees.len());
    let mut pos = 0usize;
    for &g in groups {
        for i in 0..g as usize {
            let hz = frequencies[degrees[pos].min(n - 1)];
            events.push(if i == 0 { SubdivEvent::Kick(hz) } else { SubdivEvent::Snare(hz) });
            pos += 1;
        }
    }
    events
}

// ── Helpers (kept for evolve_bar reset path) ──────────────────────────────────

#[allow(dead_code)]
pub fn melody_walk(n: usize, peak: usize) -> Vec<usize> {
    if n == 0 { return vec![]; }
    if n == 1 { return vec![0]; }
    let peak = peak.min(7);
    let mid  = n / 2;
    let mut walk = Vec::with_capacity(n);
    for i in 0..n {
        let d = if i <= mid {
            if mid == 0 { 0 } else { (i * peak + mid / 2) / mid }
        } else {
            let rest = n - 1 - i;
            let down = n - 1 - mid;
            if down == 0 { 0 } else { (rest * peak + down / 2) / down }
        };
        walk.push(d.min(7));
    }
    if let Some(l) = walk.last_mut() { *l = 0; }
    walk
}

// ── Audio commands ────────────────────────────────────────────────────────────

pub enum AudioCmd {
    AddPhrase(Phrase),
    RemovePhrase(usize),
    SetBpm(f64),
    SetSustain(f64),
    Clear,
    SetVol(f32),
    SetPaused(bool),
    SetCurPhrase(usize),
}
