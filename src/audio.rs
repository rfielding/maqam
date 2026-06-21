// audio.rs — three-level hierarchy: subdivision → group → phrase
//
// Level 1: every subdivision fires a melody note (degree walk)
// Level 2: kicks land on structural group degrees (higher-level melody)
// Level 3: phrase start = root note rings long (highest-level melody)
//          phrase end   = turnaround accent (highest-level rhythm marker)

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::Receiver;

use crate::sequencer::{AudioCmd, ControlSpec, Phrase, SubdivEvent};
use crate::synth::{
    evolve_bar, spawn_phrase_start, spawn_sub_bass, spawn_voices, Milestone, Voice, VoiceKind,
};
use crate::vcf::{MoogLadder, VcfSettings, VcfTarget};

// ── Playback state ────────────────────────────────────────────────────────────

struct BarState {
    #[allow(dead_code)]
    bar_samples: usize,
    subdiv_samples: f64,
    bar_pos: usize,
    last_subdiv: Option<usize>,
}

struct PlayingPhrase {
    phrase: Phrase,
    bar_states: Vec<BarState>,
    #[allow(dead_code)]
    current_bar: usize,
    plays_done: usize,
}

impl PlayingPhrase {
    fn new(phrase: Phrase, sr: f64, bpm: f64) -> Self {
        let bar_states = make_bar_states(&phrase, sr, bpm);
        PlayingPhrase {
            phrase,
            bar_states,
            current_bar: 0,
            plays_done: 0,
        }
    }

    fn rebuild(&mut self, sr: f64, bpm: f64) {
        self.bar_states = make_bar_states(&self.phrase, sr, bpm);
    }

    /// Reset to the very beginning of this phrase (plays_done=0, bar_pos=0).
    fn reset(&mut self) {
        self.plays_done = 0;
        for bs in self.bar_states.iter_mut() {
            bs.bar_pos = 0;
            bs.last_subdiv = None;
        }
    }
}

#[derive(Clone, Copy)]
enum PendingControl {
    SetBpm(f64),
    SetSustain(f64),
    SetVcf(VcfSettings),
}

fn make_bar_states(phrase: &Phrase, sr: f64, bpm: f64) -> Vec<BarState> {
    let subdiv_secs = 60.0 / (bpm * 2.0);
    std::iter::once(&phrase.bar)
        .map(|bar| {
            let subdiv_samples = sr * subdiv_secs;
            let bar_samples = (subdiv_samples * bar.total_subdivs as f64).round() as usize;
            BarState {
                bar_samples: bar_samples.max(1),
                subdiv_samples,
                bar_pos: 0,
                last_subdiv: None,
            }
        })
        .collect()
}

// ── Public entry point ────────────────────────────────────────────────────────

pub fn start_audio(rx: Receiver<AudioCmd>) -> anyhow::Result<cpal::Stream> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| anyhow::anyhow!("no audio output device"))?;
    let cfg = device.default_output_config()?;
    let sr = cfg.sample_rate().0 as f64;
    let ch = cfg.channels() as usize;

    let mut phrases: Vec<PlayingPhrase> = Vec::new();
    let mut voices: Vec<Voice> = Vec::new();
    let mut cur_phrase: usize = 0;
    let mut bpm = 120.0f64;
    let mut sustain = 1.25f64;
    let mut vol = 1.0f32;
    let mut vcf = VcfSettings::default();
    let mut vcf_l = MoogLadder::new(sr as f32);
    let mut vcf_r = MoogLadder::new(sr as f32);
    let mut paused = false;
    let mut jump_counters: std::collections::HashMap<usize, usize> =
        std::collections::HashMap::new();

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
                        if phrases.is_empty() {
                            cur_phrase = 0;
                        } else if let Some(pid) = pid {
                            cur_phrase =
                                phrases.iter().position(|p| p.phrase.id == pid).unwrap_or(0);
                        }
                    }
                    AudioCmd::SetBpm(b) => {
                        bpm = b;
                        for pp in phrases.iter_mut() {
                            pp.rebuild(sr, bpm);
                        }
                    }
                    AudioCmd::SetSustain(s) => {
                        sustain = s;
                    }
                    AudioCmd::SetVcf(v) => {
                        vcf = v;
                        vcf_l.set_settings(v);
                        vcf_r.set_settings(v);
                    }
                    AudioCmd::SetVol(v) => {
                        vol = v;
                    }
                    AudioCmd::SetPaused(p) => {
                        paused = p;
                    }
                    AudioCmd::SetCurPhrase(pos) => {
                        if pos < phrases.len() {
                            cur_phrase = pos;
                            // Reset the target phrase to its very beginning
                            phrases[pos].reset();
                            jump_counters.clear();
                            if let Ok(mut jc) = crate::jump_counters().try_lock() {
                                jc.clear();
                            }
                            crate::CUR_PHRASE
                                .store(cur_phrase, std::sync::atomic::Ordering::Relaxed);
                            crate::CUR_SUBDIV.store(0, std::sync::atomic::Ordering::Relaxed);
                            crate::CUR_PLAYS.store(0, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                    AudioCmd::ReplacePhrase(p) => {
                        if let Some(pp) = phrases.iter_mut().find(|pp| pp.phrase.id == p.id) {
                            pp.phrase.src = p.src;
                            pp.phrase.bar = p.bar;
                            pp.phrase.repeat = p.repeat;
                            pp.phrase.jump = p.jump;
                            pp.phrase.control = p.control;
                            pp.rebuild(sr, bpm);
                        }
                    }
                    AudioCmd::InsertPhrase { pos, phrase } => {
                        let pp = PlayingPhrase::new(phrase, sr, bpm);
                        let insert_pos = pos.min(phrases.len());
                        phrases.insert(insert_pos, pp);
                        if insert_pos <= cur_phrase && cur_phrase + 1 < phrases.len() {
                            cur_phrase += 1;
                            crate::CUR_PHRASE
                                .store(cur_phrase, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                    AudioCmd::Rotate => {
                        if phrases.len() > 1 {
                            let playing_id = phrases.get(cur_phrase).map(|p| p.phrase.id);
                            let last = phrases.remove(phrases.len() - 1);
                            phrases.insert(0, last);
                            if let Some(pid) = playing_id {
                                cur_phrase =
                                    phrases.iter().position(|p| p.phrase.id == pid).unwrap_or(0);
                                crate::CUR_PHRASE
                                    .store(cur_phrase, std::sync::atomic::Ordering::Relaxed);
                            }
                        }
                    }
                    AudioCmd::Clear => {
                        phrases.clear();
                        voices.clear();
                        cur_phrase = 0;
                        jump_counters.clear();
                        if let Ok(mut jc) = crate::jump_counters().try_lock() {
                            jc.clear();
                        }
                    }
                }
            }

            // ── per-sample loop ────────────────────────────────────────────
            for frame in data.chunks_mut(ch) {
                let (event, milestone, pending_control) = tick_sequencer(
                    &mut phrases,
                    &mut cur_phrase,
                    sr,
                    &mut voices,
                    &mut jump_counters,
                );

                if let Some(ctrl) = pending_control {
                    match ctrl {
                        PendingControl::SetBpm(v) => {
                            bpm = v;
                            for pp in phrases.iter_mut() {
                                pp.rebuild(sr, bpm);
                            }
                        }
                        PendingControl::SetSustain(v) => {
                            sustain = v;
                        }
                        PendingControl::SetVcf(v) => {
                            vcf = v;
                            vcf_l.set_settings(v);
                            vcf_r.set_settings(v);
                        }
                    }
                }

                if milestone == Milestone::PhraseStart && !paused {
                    if let Some(pp) = phrases.get(cur_phrase) {
                        let root_hz = pp.phrase.bar.root_hz;
                        spawn_phrase_start(root_hz, sustain, &mut voices);
                        let subdiv_secs = 60.0 / (bpm * 2.0);
                        let phrase_secs = (pp.phrase.bar.total_subdivs as f64
                            * subdiv_secs
                            * pp.phrase.repeat as f64)
                            .min(3.0);
                        spawn_sub_bass(root_hz, phrase_secs, &mut voices);
                    }
                }

                if let Some(ev) = event {
                    if !paused {
                        let scale = phrases
                            .get(cur_phrase)
                            .map(|pp| pp.phrase.bar.frequencies.clone())
                            .unwrap_or_default();
                        let root_hz = phrases
                            .get(cur_phrase)
                            .map(|pp| pp.phrase.bar.root_hz)
                            .unwrap_or(0.0);
                        let subdiv_secs = 60.0 / (bpm * 2.0);
                        spawn_voices(
                            ev,
                            sustain,
                            &mut voices,
                            milestone,
                            &scale,
                            root_hz,
                            subdiv_secs,
                        );
                    }
                }

                voices.retain(|v| !v.done);
                if voices.is_empty() {
                    vcf_l.reset();
                    vcf_r.reset();
                    for sample in frame.iter_mut() {
                        *sample = 0.0;
                    }
                    continue;
                }

                let (mut dry_left, mut dry_right) = (0f32, 0f32);
                let (mut vcf_left, mut vcf_right) = (0f32, 0f32);
                for v in voices.iter_mut() {
                    let routed_to_vcf = vcf.enabled && vcf_matches(vcf.target, v.kind);
                    let s = v.sample_with_wave(sr, routed_to_vcf.then_some(vcf.wave));
                    let angle = (v.pan + 1.0) * std::f32::consts::FRAC_PI_4;
                    let left = s * angle.cos();
                    let right = s * angle.sin();
                    if routed_to_vcf {
                        vcf_left += left;
                        vcf_right += right;
                    } else {
                        dry_left += left;
                        dry_right += right;
                    }
                }
                let mut left = if vcf.enabled {
                    dry_left + vcf_l.process(vcf_left)
                } else {
                    dry_left
                };
                let mut right = if vcf.enabled {
                    dry_right + vcf_r.process(vcf_right)
                } else {
                    dry_right
                };
                left = (left * vol).clamp(-1.0, 1.0);
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

fn tick_sequencer(
    phrases: &mut Vec<PlayingPhrase>,
    cur_phrase: &mut usize,
    _sr: f64,
    _voices: &mut Vec<Voice>,
    jump_counters: &mut std::collections::HashMap<usize, usize>,
) -> (Option<SubdivEvent>, Milestone, Option<PendingControl>) {
    if phrases.is_empty() {
        return (None, Milestone::None, None);
    }

    let max_iter = phrases.len() + 1;
    for _ in 0..max_iter {
        if *cur_phrase >= phrases.len() {
            *cur_phrase = 0;
        }
        let (pid, jump) = {
            let p = &phrases[*cur_phrase].phrase;
            (p.id, p.jump.clone())
        };
        if let Some(js) = jump {
            let remaining = jump_counters
                .entry(pid)
                .or_insert(js.times.saturating_sub(1));
            crate::CUR_JUMP_REM.store(*remaining, std::sync::atomic::Ordering::Relaxed);
            if *remaining > 0 {
                *remaining -= 1;
                let target = phrases
                    .iter()
                    .position(|p| p.phrase.id == js.target_id)
                    .unwrap_or(0)
                    .min(phrases.len().saturating_sub(1));
                let jump_pos = *cur_phrase;
                let ids: Vec<usize> = if target < jump_pos {
                    phrases[target..jump_pos]
                        .iter()
                        .filter_map(|pp| pp.phrase.jump.as_ref().map(|_| pp.phrase.id))
                        .collect()
                } else {
                    vec![]
                };
                for id in ids {
                    jump_counters.remove(&id);
                }
                *cur_phrase = target;
                crate::CUR_PHRASE.store(*cur_phrase, std::sync::atomic::Ordering::Relaxed);
            } else {
                jump_counters.remove(&pid);
                *cur_phrase += 1;
                if *cur_phrase >= phrases.len() {
                    *cur_phrase = 0;
                }
                crate::CUR_PHRASE.store(*cur_phrase, std::sync::atomic::Ordering::Relaxed);
            }
            if let Ok(mut jc) = crate::jump_counters().try_lock() {
                *jc = jump_counters.clone();
            }
            continue;
        }
        let control = phrases[*cur_phrase].phrase.control;
        if let Some(ctrl) = control {
            let pending = match ctrl {
                ControlSpec::SetBpm(v) => PendingControl::SetBpm(v),
                ControlSpec::SetSustain(v) => PendingControl::SetSustain(v),
                ControlSpec::SetVcf(v) => PendingControl::SetVcf(v),
            };
            *cur_phrase += 1;
            if *cur_phrase >= phrases.len() {
                *cur_phrase = 0;
            }
            crate::CUR_PHRASE.store(*cur_phrase, std::sync::atomic::Ordering::Relaxed);
            return (None, Milestone::None, Some(pending));
        }
        break;
    }

    if *cur_phrase >= phrases.len() {
        *cur_phrase = 0;
    }

    // Look ahead: find the next musical phrase after this one completes,
    // skipping over any jump entries. Used to set CrossPhraseWarning milestone.
    // Look ahead: simulate what the sequencer will actually do next.
    // Must check jump counters — a live jump loops back (same phrase),
    // an exhausted jump falls through to the next musical phrase.
    let next_is_different = {
        let curr_id = phrases[*cur_phrase].phrase.id;
        let n = phrases.len();
        let mut pos = (*cur_phrase + 1) % n;
        let mut result = false;
        for _ in 0..n {
            let p = &phrases[pos].phrase;
            if let Some(js) = &p.jump {
                let remaining = jump_counters
                    .get(&p.id)
                    .copied()
                    .unwrap_or_else(|| js.times.saturating_sub(1));
                if remaining > 0 {
                    let target = phrases
                        .iter()
                        .position(|pp| pp.phrase.id == js.target_id)
                        .unwrap_or(0);
                    result = phrases[target].phrase.id != curr_id;
                    break;
                }
                pos = (pos + 1) % n;
            } else if p.control.is_some() {
                pos = (pos + 1) % n;
            } else {
                result = p.id != curr_id;
                break;
            }
        }
        result
    };

    let pp = &mut phrases[*cur_phrase];
    let bar = &pp.phrase.bar;
    let bs = &mut pp.bar_states[0];

    let curr = if bar.total_subdivs > 0 {
        ((bs.bar_pos as f64 / bs.subdiv_samples) as usize).min(bar.total_subdivs - 1)
    } else {
        0
    };

    let is_last_play = pp.plays_done + 1 >= pp.phrase.repeat;
    let is_last_subdiv = curr + 1 >= bar.total_subdivs;

    let mut milestone = Milestone::None;
    let ev = if bs.last_subdiv != Some(curr) {
        bs.last_subdiv = Some(curr);
        if pp.plays_done == 0 && curr == 0 {
            milestone = Milestone::PhraseStart;
        } else if is_last_play && is_last_subdiv {
            milestone = if next_is_different {
                Milestone::Turnaround // half-vol kick: change is coming
            } else {
                Milestone::CrossPhraseWarning // rimshot: just looping
            };
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
        bs.bar_pos = 0;
        bs.last_subdiv = None;
        pp.plays_done += 1;
        evolve_bar(&mut pp.phrase.bar, true);
        if pp.plays_done >= pp.phrase.repeat {
            pp.plays_done = 0;
            let prev = *cur_phrase;
            *cur_phrase = (*cur_phrase + 1) % phrases.len();
            crate::CUR_PHRASE.store(*cur_phrase, std::sync::atomic::Ordering::Relaxed);
            if *cur_phrase != prev && milestone == Milestone::None {
                milestone = Milestone::PhraseChange;
            }
        }
    }

    (ev, milestone, None)
}

fn vcf_matches(target: VcfTarget, kind: VoiceKind) -> bool {
    match target {
        VcfTarget::All => true,
        VcfTarget::Bass => matches!(kind, VoiceKind::SubBass),
        VcfTarget::Kanun => matches!(kind, VoiceKind::MelodyFm),
        VcfTarget::Kick => matches!(kind, VoiceKind::FloorTom),
    }
}
