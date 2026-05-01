// audio.rs — three-level hierarchy: subdivision → group → phrase
//
// Level 1: every subdivision fires a melody note (degree walk)
// Level 2: kicks land on structural group degrees (higher-level melody)  
// Level 3: phrase start = root note rings long (highest-level melody)
//          phrase end   = turnaround accent (highest-level rhythm marker)

use crossbeam_channel::Receiver;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::sequencer::{AudioCmd, Phrase, SubdivEvent};
use crate::synth::{evolve_bar, spawn_phrase_start, spawn_sub_bass, spawn_voices, Voice};

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
                    AudioCmd::Clear => { phrases.clear(); voices.clear(); cur_phrase = 0; }
                }
            }

            // ── per-sample loop ────────────────────────────────────────────
            for frame in data.chunks_mut(ch) {

                // Tick the sequencer one sample; returns event + flags
                let (event, phrase_start, turnaround) = tick_sequencer(
                    &mut phrases, &mut cur_phrase, bpm, sr, sustain, &mut voices
                );

                // Level 3: phrase start → long root note (highest melody level)
                if phrase_start {
                    if let Some(pp) = phrases.get(cur_phrase) {
                        let root_hz = pp.phrase.bar.root.to_hz();
                        spawn_phrase_start(root_hz, sustain, &mut voices);
                        // Sub-bass: hold the root for the full phrase duration
                        let subdiv_secs  = 60.0 / (bpm * 2.0);
                        let phrase_secs  = pp.phrase.bar.total_subdivs as f64
                                         * subdiv_secs
                                         * pp.phrase.repeat as f64;
                        spawn_sub_bass(root_hz, phrase_secs, &mut voices);
                    }
                }

                // Level 1 & 2: subdivision event → melody + chord tones
                if let Some(ev) = event {
                    spawn_voices(ev, sustain, &mut voices, turnaround);
                }

                let mix: f32 = (voices.iter_mut()
                    .map(|v| v.sample(sr))
                    .sum::<f32>() * vol)
                    .clamp(-1.0, 1.0);

                for s in frame.iter_mut() { *s = mix; }
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
    phrases:    &mut Vec<PlayingPhrase>,
    cur_phrase: &mut usize,
    _bpm:       f64,
    _sr:        f64,
    _sustain:   f64,
    _voices:    &mut Vec<Voice>,
) -> (Option<SubdivEvent>, bool, bool) {
    if phrases.is_empty() { return (None, false, false); }
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
    let turnaround     = is_last_play && is_last_subdiv;

    let mut phrase_start = false;
    let ev = if bs.last_subdiv != Some(curr) {
        bs.last_subdiv = Some(curr);
        if pp.plays_done == 0 && curr == 0 { phrase_start = true; }
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
            pp.plays_done = 0;
            *cur_phrase   = (*cur_phrase + 1) % phrases.len();
        }
    }

    (ev, phrase_start, turnaround)
}
