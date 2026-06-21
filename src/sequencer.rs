// sequencer.rs — phrase/bar data structures and audio commands
//
// Comma-separated ajnas form ONE combined scale (stacked, not sequential).
// The melody walks through all frequencies of all ajnas together.

use crate::synth::{expand_degrees, zigzag_walk};
use crate::tuning::{snap_to_oud_lattice, Maqam, Pitch};
use crate::vcf::VcfSettings;

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
    #[allow(dead_code)]
    pub root: Pitch, // original pitch (for display)
    pub root_hz: f64, // actual 5-limit snapped root Hz
    #[allow(dead_code)]
    pub maqam: Maqam, // first jins maqam — for display
    #[allow(dead_code)]
    pub maqam_names: Vec<String>, // all jins names — kept for debugging
    pub ratio_strs: Vec<String>, // actual JI ratios per jins — for display
    pub jins_boundaries: Vec<usize>, // scale-index handoffs between stacked ajnas
    pub groups: Vec<u8>, // rhythm groups
    pub frequencies: Vec<f64>, // combined JI scale, sorted ascending
    pub group_degrees: Vec<usize>, // waypoints (n_groups+1), index into frequencies
    pub degrees: Vec<usize>, // per-subdivision index into frequencies
    pub events: Vec<SubdivEvent>,
    pub total_subdivs: usize,
}

impl Bar {
    pub fn recompute_events(&mut self) {
        self.degrees = expand_degrees(&self.group_degrees, &self.groups);
        self.events = events_from_freqs(&self.degrees, &self.frequencies, &self.groups);
    }

    pub fn rhythm_display(&self) -> String {
        self.events
            .iter()
            .map(|e| match e {
                SubdivEvent::Kick(_) => 'X',
                SubdivEvent::Snare(_) => '.',
            })
            .collect()
    }
}

// ── Phrase ────────────────────────────────────────────────────────────────────

/// A jump instruction stored as a sequence entry.
#[derive(Debug, Clone)]
pub struct JumpSpec {
    pub target_id: usize, // stable phrase id of the jump target
    pub times: usize,
}

#[derive(Debug, Clone, Copy)]
pub enum ControlSpec {
    SetBpm(f64),
    SetSustain(f64),
    SetVcf(VcfSettings),
}

#[derive(Debug, Clone)]
pub struct Phrase {
    pub id: usize,
    pub src: String,
    pub bar: Bar,
    pub repeat: usize,
    /// If Some, this is a control-flow entry (no audio).
    pub jump: Option<JumpSpec>,
    /// If Some, this is a settings/timeline entry (no audio).
    pub control: Option<ControlSpec>,
}

impl Phrase {
    pub fn rhythm_display(&self) -> String {
        self.bar.rhythm_display()
    }
    #[allow(dead_code)]
    pub fn is_jump(&self) -> bool {
        self.jump.is_some()
    }
}

fn empty_bar() -> Bar {
    Bar {
        root: Pitch {
            letter: 'd',
            accidental: 0,
            octave: 4,
        },
        root_hz: 293.6648,
        maqam: Maqam::new("Nahawand"),
        maqam_names: vec![],
        ratio_strs: vec![],
        jins_boundaries: vec![],
        groups: vec![],
        frequencies: vec![],
        group_degrees: vec![],
        degrees: vec![],
        events: vec![],
        total_subdivs: 0,
    }
}

/// Build a jump-entry phrase (no audio — pure sequencer control flow).
pub fn build_jump_entry(id: usize, target_id: usize, times: usize) -> Phrase {
    Phrase {
        id,
        src: format!("j {target_id} {times}"),
        bar: empty_bar(),
        repeat: 1,
        jump: Some(JumpSpec { target_id, times }),
        control: None,
    }
}

pub fn build_control_entry(id: usize, src: String, control: ControlSpec) -> Phrase {
    Phrase {
        id,
        src,
        bar: empty_bar(),
        repeat: 1,
        jump: None,
        control: Some(control),
    }
}

// ── Builder ───────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct BarSpec {
    pub src: String,
    pub root: Pitch,
    pub maqam: Maqam,
    pub groups: Vec<u8>,
}

pub fn build_phrase(
    phrase_id: usize,
    src: String,
    specs: Vec<BarSpec>,
    peak_degree: usize,
    repeat: usize,
) -> Phrase {
    assert!(!specs.is_empty());
    let root_hz_0 = snap_to_oud_lattice(specs[0].root.to_hz());

    // ── Tetrachord stacking ───────────────────────────────────────────────
    //
    // Each jins contributes exactly its lower tetrachord (4 degrees: root +
    // 3 intervals), EXCEPT the last jins in a multi-spec phrase which
    // contributes all 8 degrees so its upper portion fills out to the octave.
    //
    // Single-spec ("d bayati 332"): only 4 degrees — melody stays inside the
    // jins.  To get a full maqam scale, name both ajnas explicitly:
    //   "d bayati, a nahawand 332"
    //
    // Dedup at 50 cents: if two ajnas contribute notes within a quarter-tone
    // of each other, the earlier-jins note wins.

    let n_specs = specs.len();
    let roots_hz: Vec<f64> = specs
        .iter()
        .map(|s| snap_to_oud_lattice(s.root.to_hz()))
        .collect();
    let upper_bound = roots_hz[0] * 2.0;
    let dedup_thresh = if n_specs > 1 {
        50.0_f64 / 1200.0
    } else {
        23.0_f64 / 1200.0
    };

    let mut deduped: Vec<(f64, usize)> = Vec::new();

    for (i, spec) in specs.iter().enumerate() {
        let root = roots_hz[i];
        let next_root = if i + 1 < n_specs {
            Some(roots_hz[i + 1])
        } else {
            None
        };

        // ratios() length IS the jins size — no n_degrees needed.
        // The octave filter handles upper-jins notes that exceed the octave.
        let jins_freqs: Vec<f64> = spec
            .maqam
            .ratios()
            .iter()
            .map(|&(p, q)| root * p as f64 / q as f64)
            .filter(|&f| {
                if f > upper_bound * 1.001 {
                    return false;
                }
                if let Some(nr) = next_root {
                    if (f / nr).log2().abs() < 70.0 / 1200.0 {
                        return false;
                    }
                }
                true
            })
            .collect();

        for f in jins_freqs {
            if deduped
                .iter()
                .all(|&(prev, _)| (f / prev).log2().abs() > dedup_thresh)
            {
                deduped.push((f, i));
            }
        }
    }

    deduped.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    if let Some(tonic_idx) = deduped
        .iter()
        .position(|&(f, _)| (f / root_hz_0).log2().abs() < 1.0 / 1200.0)
    {
        // Keep the tonic as degree 0 so lower leading tones like 8/9 wrap to the
        // end of the scale instead of redefining the melodic center.
        deduped.rotate_left(tonic_idx);
    }

    let frequencies: Vec<f64> = deduped.iter().map(|&(f, _)| f).collect();
    let mut jins_boundaries = Vec::new();
    if !deduped.is_empty() {
        let mut prev_owner = deduped[0].1;
        for (idx, &(_, owner)) in deduped.iter().enumerate().skip(1) {
            if owner != prev_owner {
                jins_boundaries.push(idx);
                prev_owner = owner;
            }
        }
    }

    let groups = specs
        .last()
        .map(|s| s.groups.clone())
        .unwrap_or_else(|| vec![4]);
    let n_groups = groups.len();
    let n_freqs = frequencies.len();
    let peak = (peak_degree * n_freqs / 8).clamp(2, n_freqs.saturating_sub(1));
    let group_degrees = zigzag_walk(n_groups, peak);

    let degrees = expand_degrees(&group_degrees, &groups);
    let total_subdivs = degrees.len();
    let events = events_from_freqs(&degrees, &frequencies, &groups);

    let maqam_names: Vec<String> = specs.iter().map(|s| s.maqam.name().to_string()).collect();

    // ratio_strs: one entry per jins spec, e.g. "1/1 12/11 32/27 4/3 3/2"
    // Derived directly from ratios() so it reflects whatever tuning.rs defines.
    let ratio_strs: Vec<String> = specs
        .iter()
        .map(|s| {
            s.maqam
                .ratios()
                .iter()
                .map(|&(p, q)| format!("{}/{}", p, q))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();

    let bar = Bar {
        root: specs[0].root,
        root_hz: root_hz_0,
        maqam: specs[0].maqam.clone(),
        maqam_names,
        ratio_strs,
        jins_boundaries,
        groups,
        frequencies,
        group_degrees,
        degrees,
        events,
        total_subdivs,
    };

    Phrase {
        id: phrase_id,
        src,
        bar,
        repeat,
        jump: None,
        control: None,
    }
}

// ── Event computation ─────────────────────────────────────────────────────────

pub fn events_from_freqs(
    degrees: &[usize],
    frequencies: &[f64],
    groups: &[u8],
) -> Vec<SubdivEvent> {
    let n = frequencies.len();
    if n == 0 {
        return vec![];
    }
    let mut events = Vec::with_capacity(degrees.len());
    let mut pos = 0usize;
    for &g in groups {
        for i in 0..g as usize {
            let hz = frequencies[degrees[pos].min(n - 1)];
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

// ── Helpers ───────────────────────────────────────────────────────────────────

#[allow(dead_code)]
pub fn melody_walk(n: usize, peak: usize) -> Vec<usize> {
    if n == 0 {
        return vec![];
    }
    if n == 1 {
        return vec![0];
    }
    let peak = peak.min(7);
    let mid = n / 2;
    let mut walk = Vec::with_capacity(n);
    for i in 0..n {
        let d = if i <= mid {
            if mid == 0 {
                0
            } else {
                (i * peak + mid / 2) / mid
            }
        } else {
            let rest = n - 1 - i;
            let down = n - 1 - mid;
            if down == 0 {
                0
            } else {
                (rest * peak + down / 2) / down
            }
        };
        walk.push(d.min(7));
    }
    if let Some(l) = walk.last_mut() {
        *l = 0;
    }
    walk
}

// ── Audio commands ────────────────────────────────────────────────────────────

pub enum AudioCmd {
    AddPhrase(Phrase),
    RemovePhrase(usize),
    InsertPhrase { pos: usize, phrase: Phrase },
    ReplacePhrase(Phrase),
    Rotate,
    SetBpm(f64),
    SetSustain(f64),
    SetVcf(VcfSettings),
    Clear,
    SetVol(f32),
    SetPaused(bool),
    SetCurPhrase(usize),
}
