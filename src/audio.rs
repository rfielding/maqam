// audio.rs — three-level hierarchy: subdivision → group → phrase
//
// Level 1: every subdivision fires a melody note (degree walk)
// Level 2: kicks land on structural group degrees (higher-level melody)  
// Level 3: phrase start = root note rings long (highest-level melody)
//          phrase end   = turnaround accent (highest-level rhythm marker)

use crossbeam_channel::Receiver;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::sequencer::{AudioCmd, Phrase, SubdivEvent};
use crate::synth::{evolve_bar, spawn_phrase_start, spawn_sub_bass, spawn_voices, Milestone, Voice};

// ── Playback state ────────────────────────────────────────────────────────────

struct BarState {
    #[allow(dead_code)]
    bar_samples:    usize,
    subdiv_samples: f64,
    bar_pos:        usize,
    last_subdiv:    Option<usize>,
}

struct PlayingPhrase {
    phrase:      Phrase,
    bar_states:  Vec<BarState>,
    #[allow(dead_code)]
    current_bar: usize,
    plays_done:  usize,
}

impl PlayingPhrase {
    fn new(phrase: Phrase, sr: f64, bpm: f64) -> Self {
        let bar_states = make_bar_states(&phrase, sr, bpm);
        PlayingPhrase { phrase, bar_states, current_bar: 0, plays_done: 0 }
    }

    fn rebuild(&mut self, sr: f64, bpm: f64) {
        self.bar_states = make_bar_states(&self.phrase, sr, bpm);
    }
}

fn make_bar_states(phrase: &Phrase, sr: f64, bpm: f64) -> Vec<BarState> {
    let subdiv_secs = 60.0 / (bpm * 2.0);
    std::iter::once(&phrase.bar).map(|bar| {
        let subdiv_samples = sr * subdiv_secs;
        let bar_samples    = (subdiv_samples * bar.total_subdivs as f64).round() as usize;
        BarState { bar_samples: bar_samples.max(1), subdiv_samples,
                   bar_pos: 0, last_subdiv: None }
    }).collect()
}

// ── Public entry point ────────────────────────────────────────────────────────

pub fn start_audio(rx: Receiver<AudioCmd>) -> anyhow::Result<cpal::Stream> {
    let host   = cpal::default_host();
    let device = host.default_output_device()
        .ok_or_else(|| anyhow::anyhow!("no audio output device"))?;
    let cfg    = device.default_output_config()?;
    let sr     = cfg.sample_rate().0 as f64;
    let ch     = cfg.channels() as usize;

    let mut phrases:    Vec<PlayingPhrase> = Vec::new();
    let mut voices:     Vec<Voice>         = Vec::new();
    let mut cur_phrase: usize              = 0;
    let mut bpm         = 120.0f64;
    let mut sustain     = 1.25f64;
    let mut vol         = 1.0f32;
    let mut paused      = false;
    // Per-entry jump counters: phrase.id → remaining jumps
    let mut jump_counters: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();

    let stream = device.build_output_stream(
        &cfg.into(),
        move |data: &mut [f32], _| {

            // ── drain commands ─────────────────────────────────────────────
            while let Ok(cmd) = rx.try_recv() {
                match cmd {
                    AudioCmd::AddPhrase(p) => {
                        phrases.push(PlayingPhrase::new(p, sr, bpm));
                    }
                    AudioCmd::RemovePhrase(id) => {
                        let pid = phrases.get(cur_phrase).map(|p| p.phrase.id);
                        phrases.retain(|p| p.phrase.id != id);
                        if phrases.is_empty() { cur_phrase = 0; }
                        else if let Some(pid) = pid {
                            cur_phrase = phrases.iter()
                                .position(|p| p.phrase.id == pid).unwrap_or(0);
                        }
                    }
                    AudioCmd::SetBpm(b) => {
                        bpm = b;
                        for pp in phrases.iter_mut() { pp.rebuild(sr, bpm); }
                    }
                    AudioCmd::SetSustain(s) => { sustain = s; }
                    AudioCmd::SetVol(v)     => { vol = v; }
                    AudioCmd::SetPaused(p)  => { paused = p; }
                    AudioCmd::SetCurPhrase(pos) => {
                        if pos < phrases.len() {
                            cur_phrase = pos;
                            jump_counters.clear();
                            crate::CUR_PHRASE.store(cur_phrase, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                    AudioCmd::Clear => { phrases.clear(); voices.clear(); cur_phrase = 0; jump_counters.clear();
                        if let Ok(mut jc) = crate::jump_counters().try_lock() { jc.clear(); } }
                }
            }

            // ── per-sample loop ────────────────────────────────────────────
            for frame in data.chunks_mut(ch) {

                // Tick the sequencer one sample; returns event + flags
                let (event, milestone) = tick_sequencer(
                    &mut phrases, &mut cur_phrase, bpm, sr, sustain, &mut voices,
                    &mut jump_counters
                );

                // Level 3: phrase start → long root note (highest melody level)
                if milestone == Milestone::PhraseStart && !paused {
                    if let Some(pp) = phrases.get(cur_phrase) {
                        let root_hz = pp.phrase.bar.root_hz;
                        spawn_phrase_start(root_hz, sustain, &mut voices);
                        // Sub-bass: hold the root for the full phrase duration
                        let subdiv_secs  = 60.0 / (bpm * 2.0);
                        let phrase_secs  = (pp.phrase.bar.total_subdivs as f64
                                         * subdiv_secs
                                         * pp.phrase.repeat as f64)
                                         .min(3.0);  // cap to avoid bleeding into next phrase
                        spawn_sub_bass(root_hz, phrase_secs, &mut voices);
                    }
                }

                // Level 1 & 2: subdivision event → melody + chord tones
                if let Some(ev) = event {
                    if !paused {
                        let scale = phrases.get(cur_phrase)
                        .map(|pp| pp.phrase.bar.frequencies.clone())
                        .unwrap_or_default();
                    spawn_voices(ev, sustain, &mut voices, milestone, &scale);
                    }
                }

                // Stereo mix: equal-power pan law.
                // Sub-bass and kick stay at pan=0 (center).
                let (mut left, mut right) = (0f32, 0f32);
                for v in voices.iter_mut() {
                    let s    = v.sample(sr);
                    // angle ∈ [π/8 .. 3π/8] for pan ∈ [-0.5 .. +0.5]
                    let angle = (v.pan + 1.0) * std::f32::consts::FRAC_PI_4;
                    left  += s * angle.cos();
                    right += s * angle.sin();
                }
                left  = (left  * vol).clamp(-1.0, 1.0);
                right = (right * vol).clamp(-1.0, 1.0);

                if frame.len() >= 2 {
                    frame[0] = left;
                    frame[1] = right;
                } else {
                    frame[0] = (left + right) * 0.5;
                }
            }

            voices.retain(|v| !v.done);
        },
        |err| eprintln!("audio error: {err}"),
        None,
    )?;

    stream.play()?;
    Ok(stream)
}

/// Advance the sequencer by one sample.
/// Returns (Option<event>, phrase_start, turnaround).
fn tick_sequencer(
    phrases:       &mut Vec<PlayingPhrase>,
    cur_phrase:    &mut usize,
    _bpm:          f64,
    _sr:           f64,
    _sustain:      f64,
    _voices:       &mut Vec<Voice>,
    jump_counters: &mut std::collections::HashMap<usize, usize>,
) -> (Option<SubdivEvent>, Milestone) {
    if phrases.is_empty() { return (None, Milestone::None); }

    // Skip over jump entries — execute them immediately
    let max_iter = phrases.len() + 1;
    for _ in 0..max_iter {
        if *cur_phrase >= phrases.len() { *cur_phrase = 0; }
        let (pid, jump) = {
            let p = &phrases[*cur_phrase].phrase;
            (p.id, p.jump.clone())
        };
        if let Some(js) = jump {
            // Counter starts at times-1: phrase already played once before
            // we reached the jump, so times=3 means 2 more jumps = 3 plays total.
            let remaining = jump_counters.entry(pid).or_insert(js.times.saturating_sub(1));
            crate::CUR_JUMP_REM.store(*remaining, std::sync::atomic::Ordering::Relaxed);
            if *remaining > 0 {
                *remaining -= 1;
                let target   = js.to_pos.min(phrases.len() - 1);
                let jump_pos = *cur_phrase;
                // Reset only entries strictly BETWEEN target and this jump (inner loops).
                // Do NOT reset this entry itself — that would reinitialize and loop forever.
                let ids: Vec<usize> = phrases[target..jump_pos].iter()
                    .filter_map(|pp| pp.phrase.jump.as_ref().map(|_| pp.phrase.id))
                    .collect();
                for id in ids { jump_counters.remove(&id); }
                *cur_phrase = target;
                crate::CUR_PHRASE.store(*cur_phrase, std::sync::atomic::Ordering::Relaxed);
            } else {
                // Exhausted — fall through, reset counter for next outer loop pass
                jump_counters.remove(&pid);
                *cur_phrase += 1;
                if *cur_phrase >= phrases.len() { *cur_phrase = 0; }
                crate::CUR_PHRASE.store(*cur_phrase, std::sync::atomic::Ordering::Relaxed);
            }
            // Publish updated counters to UI after jump state changes
            if let Ok(mut jc) = crate::jump_counters().try_lock() {
                *jc = jump_counters.clone();
            }
            continue;
        }
        break;
    }

    if *cur_phrase >= phrases.len() { *cur_phrase = 0; }
    let pp  = &mut phrases[*cur_phrase];
    let bar = &pp.phrase.bar;
    let bs  = &mut pp.bar_states[0];

    let curr = if bar.total_subdivs > 0 {
        ((bs.bar_pos as f64 / bs.subdiv_samples) as usize)
            .min(bar.total_subdivs - 1)
    } else { 0 };

    let is_last_play   = pp.plays_done + 1 >= pp.phrase.repeat;
    let is_last_subdiv = curr + 1 >= bar.total_subdivs;

    let mut milestone = Milestone::None;
    let ev = if bs.last_subdiv != Some(curr) {
        bs.last_subdiv = Some(curr);
        if pp.plays_done == 0 && curr == 0 {
            milestone = Milestone::PhraseStart;
        } else if is_last_play && is_last_subdiv {
            milestone = Milestone::Turnaround;
        }
        crate::CUR_SUBDIV.store(curr, std::sync::atomic::Ordering::Relaxed);
        crate::CUR_PLAYS.store(pp.plays_done, std::sync::atomic::Ordering::Relaxed);
        bar.events.get(curr).copied()
    } else {
        None
    };

    bs.bar_pos += 1;
    let bar_samples = (bs.subdiv_samples * bar.total_subdivs as f64).round() as usize;
    if bs.bar_pos >= bar_samples.max(1) {
        bs.bar_pos     = 0;
        bs.last_subdiv = None;
        pp.plays_done += 1;
        evolve_bar(&mut pp.phrase.bar, true);
        if pp.plays_done >= pp.phrase.repeat {
            pp.plays_done  = 0;
            let prev       = *cur_phrase;
            *cur_phrase    = (*cur_phrase + 1) % phrases.len();
            crate::CUR_PHRASE.store(*cur_phrase, std::sync::atomic::Ordering::Relaxed);
            if *cur_phrase != prev && milestone == Milestone::None {
                milestone = Milestone::PhraseChange;
            }
        }
    }

    (ev, milestone)
}
