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
    pub root:          Pitch,          // first jins root — for phrase_start voice
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

#[derive(Debug, Clone)]
pub struct Phrase {
    pub id:     usize,
    pub src:    String,
    pub bar:    Bar,     // single bar — the combined scale
    pub repeat: usize,
}

impl Phrase {
    pub fn rhythm_display(&self) -> String { self.bar.rhythm_display() }
}

// ── Builder ───────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct BarSpec {
    pub src:    String,
    pub root:   Pitch,
    pub maqam:  Maqam,
    pub groups: Vec<u8>,
}

pub fn build_phrase(
    phrase_id:   usize,
    src:         String,
    specs:       Vec<BarSpec>,
    peak_degree: usize,
    repeat:      usize,
) -> Phrase {
    assert!(!specs.is_empty());

    // ── Build combined JI scale — snap each jins root to coincidence point ──
    //
    // Maqams stack ajnas at coincidence points: the root of each subsequent
    // jins must be the SAME frequency as the nearest note already computed in
    // JI from the first jins.  We cannot use equal-tempered pitch for
    // subsequent roots — that drifts by up to a syntonic comma (81/80 = 21.5¢).
    //
    // Algorithm:
    //   1. Compute first jins from its stated root (equal-tempered is fine here).
    //   2. For each subsequent jins, find the note in the already-built scale
    //      closest to the jins's nominal equal-tempered root.
    //   3. Use THAT frequency as the actual root — preserving JI coherence.
    //   4. Merge and deduplicate within 4 cents (tighter than syntonic comma).

    let mut deduped: Vec<f64> = Vec::new();

    for (i, spec) in specs.iter().enumerate() {
        let actual_root_hz = if i == 0 {
            spec.root.to_hz()
        } else {
            // Snap to nearest already-built frequency
            let nominal = spec.root.to_hz();
            // Search across octaves: the coincidence might be in any octave.
            // Compare log2 distances, wrap into single octave for matching.
            deduped.iter().copied().min_by(|&a, &b| {
                let da = (nominal / a).log2().abs().min((nominal * 2.0 / a).log2().abs())
                                                    .min((nominal / (a * 2.0)).log2().abs());
                let db = (nominal / b).log2().abs().min((nominal * 2.0 / b).log2().abs())
                                                    .min((nominal / (b * 2.0)).log2().abs());
                da.partial_cmp(&db).unwrap()
            }).unwrap_or(nominal)
        };

        // Build this jins's frequencies from the snapped root
        let mut jins_freqs: Vec<f64> = spec.maqam.ratios().iter()
            .map(|&(p, q)| actual_root_hz * p as f64 / q as f64)
            .collect();
        jins_freqs.sort_by(|a, b| a.partial_cmp(b).unwrap());

        // Merge into deduped (within 4 cents — tighter than syntonic comma)
        for f in jins_freqs {
            if deduped.iter().all(|&prev| (f / prev).log2().abs() > 4.0 / 1200.0) {
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

    let bar = Bar {
        root:         specs[0].root,
        maqam:        specs[0].maqam,
        maqam_names,
        groups,
        frequencies:  deduped,
        group_degrees,
        degrees,
        events,
        total_subdivs,
    };

    Phrase { id: phrase_id, src, bar, repeat }
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
}
