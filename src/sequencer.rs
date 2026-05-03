// sequencer.rs — phrase/bar data structures and audio commands
//
// Comma-separated ajnas form ONE combined scale (stacked, not sequential).
// The melody walks through all frequencies of all ajnas together.

use crate::tuning::{snap_to_oud_lattice, Maqam, Pitch};
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

// snap_to_oud_lattice moved to tuning.rs






/// D4 as the universal JI reference pitch.
/// All notes are expressed as D × (5-limit ratio).

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

    // ── Build combined scale from tetrachord stacking ───────────────────────
    //
    // Rule: when combining N jins with commas, each jins is a TETRACHORD —
    // it contributes exactly its first 4 scale degrees (root + 3 intervals).
    // The last jins contributes all notes up to the octave of the first root.
    //
    // This mirrors Arabic maqam practice: each jins name describes a specific
    // 4-note colour; you stack colours to build a full scale.
    //
    // Single jins (no comma): all 8 degrees as usual.
    //
    // Dedup at 50 cents: if two jins contribute notes within a quarter-tone of
    // each other, the earlier-jins note wins. This prevents micro-intervals
    // (e.g. D×11/8=403 and G=391 are 54¢ apart — if both land in range, keep
    // the one from the earlier jins).

    let n_specs = specs.len();
    let roots_hz: Vec<f64> = specs.iter()
        .map(|s| snap_to_oud_lattice(s.root.to_hz()))
        .collect();
    let upper_bound = roots_hz[0] * 2.0;

    let dedup_thresh = if n_specs > 1 { 50.0_f64 / 1200.0 } else { 23.0_f64 / 1200.0 };

    let mut deduped: Vec<f64> = Vec::new();

    for (i, spec) in specs.iter().enumerate() {
        let root      = roots_hz[i];
        let is_last   = i + 1 == n_specs;
        let n_degrees = if n_specs == 1 { 8 } else if is_last { 8 } else { 4 };

        let next_root = if i + 1 < n_specs { Some(roots_hz[i + 1]) } else { None };

        let jins_freqs: Vec<f64> = spec.maqam.ratios()[..n_degrees]
            .iter()
            .map(|&(p, q)| root * p as f64 / q as f64)
            .filter(|&f| {
                if f > upper_bound * 1.001 { return false; }
                // Drop notes within 70 cents of the next jins's root —
                // they would create micro-interval clashes at the jins boundary.
                if let Some(nr) = next_root {
                    if (f / nr).log2().abs() < 70.0 / 1200.0 { return false; }
                }
                true
            })
            .collect();

        for f in jins_freqs {
            if deduped.iter().all(|&prev| (f / prev).log2().abs() > dedup_thresh) {
                deduped.push(f);
            }
        }
    }

    deduped.sort_by(|a, b| a.partial_cmp(b).unwrap());
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

    let root_hz_0 = snap_to_oud_lattice(specs[0].root.to_hz());
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
            // A group of length 1 is a pickup or accent — use snare, not kick.
            // A lone eighth note in e.g. 84421 would sound like a click as a kick;
            // a snare pop is much more musical.
            let ev = if g == 1 {
                SubdivEvent::Snare(hz)
            } else if i == 0 {
                SubdivEvent::Kick(hz)
            } else {
                SubdivEvent::Snare(hz)
            };
            events.push(ev);
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
