#![allow(dead_code)]

// record_old.rs — offline render to MP4

use std::collections::HashMap;
use std::io::Write;
use std::process::{Command, Stdio};

use crate::fx::{FxProcessor, FxSettings};
use crate::sequencer::{ControlSpec, Phrase};
use crate::synth::{
    evolve_bar, spawn_phrase_start, spawn_sub_bass, spawn_voices, Milestone, Voice, VoiceKind,
};
use crate::vcf::{MoogLadder, VcfBank, VcfSettings, VcfTarget};

const SR: f64 = 44100.0;
const RENDER_COOP_INTERVAL_SAMPLES: usize = 4096;

fn temp_path(name: &str) -> String {
    let mut p = std::env::temp_dir();
    p.push(name);
    p.to_string_lossy().replace('\\', "/")
}

fn ffmpeg_command() -> Command {
    #[cfg(unix)]
    {
        let mut cmd = Command::new("nice");
        cmd.args(["-n", "10", "ffmpeg"]);
        cmd
    }
    #[cfg(not(unix))]
    {
        Command::new("ffmpeg")
    }
}

fn ffmpeg_status(cmd: &mut Command) -> anyhow::Result<bool> {
    match cmd.status() {
        Ok(status) => Ok(status.success()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!(
                "video rendering requires ffmpeg on your PATH; install ffmpeg and try again"
            )
        }
        Err(err) => Err(err.into()),
    }
}

fn yield_to_audio_thread(rendered_samples: usize) {
    if rendered_samples != 0 && rendered_samples % RENDER_COOP_INTERVAL_SAMPLES == 0 {
        std::thread::sleep(std::time::Duration::from_micros(250));
    }
}

fn phrase_display_title(src: &str) -> String {
    let title = src
        .split(',')
        .filter_map(|part| {
            let tokens: Vec<&str> = part
                .split_whitespace()
                .filter(|token| {
                    !token.chars().all(|c| c.is_ascii_digit()) && !token.starts_with('r')
                })
                .collect();
            match tokens.as_slice() {
                [root, maqam, ..] => Some(format!(
                    "{} {}",
                    root.to_ascii_uppercase(),
                    maqam.to_ascii_uppercase()
                )),
                [maqam] => Some(maqam.to_ascii_uppercase()),
                _ => None,
            }
        })
        .collect::<Vec<_>>()
        .join("+");
    if title.is_empty() {
        src.split_whitespace()
            .filter(|token| !token.chars().all(|c| c.is_ascii_digit()))
            .take(3)
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_uppercase()
    } else {
        title
    }
}

#[derive(Clone, Copy)]
struct RenderOccurrence {
    phrase_idx: usize,
    snap_idx: usize,
    bpm: f64,
    sustain: f64,
    vcf: VcfBank,
    fx: FxSettings,
    arrived_via_jump: Option<usize>,
}
#[derive(Clone, Copy)]
struct RenderEntry {
    phrase_idx: usize,
    play_num: usize,
    snap_idx: usize,
    bpm: f64,
    sustain: f64,
    vcf: VcfBank,
    fx: FxSettings,
    arrived_via_jump: Option<usize>,
}

struct StereoFilter {
    left: MoogLadder,
    right: MoogLadder,
}

impl StereoFilter {
    fn new(sr: f32) -> Self {
        Self {
            left: MoogLadder::new(sr),
            right: MoogLadder::new(sr),
        }
    }

    fn set_settings(&mut self, settings: VcfSettings) {
        self.left.set_settings(settings);
        self.right.set_settings(settings);
    }

    fn update_settings(&mut self, settings: VcfSettings) {
        self.left.update_settings(settings);
        self.right.update_settings(settings);
    }

    fn reset(&mut self) {
        self.left.reset();
        self.right.reset();
    }

    fn process(&mut self, left: f32, right: f32) -> (f32, f32) {
        (self.left.process(left), self.right.process(right))
    }
}

struct FilterBank {
    all: StereoFilter,
    bass: StereoFilter,
    kanun: StereoFilter,
    kick: StereoFilter,
}

impl FilterBank {
    fn new(sr: f32) -> Self {
        Self {
            all: StereoFilter::new(sr),
            bass: StereoFilter::new(sr),
            kanun: StereoFilter::new(sr),
            kick: StereoFilter::new(sr),
        }
    }

    fn set_bank(&mut self, bank: VcfBank) {
        self.all.set_settings(bank.all);
        self.bass.set_settings(bank.bass);
        self.kanun.set_settings(bank.kanun);
        self.kick.set_settings(bank.kick);
    }

    fn update_bank(&mut self, bank: VcfBank) {
        self.all.update_settings(bank.all);
        self.bass.update_settings(bank.bass);
        self.kanun.update_settings(bank.kanun);
        self.kick.update_settings(bank.kick);
    }

    fn reset(&mut self) {
        self.all.reset();
        self.bass.reset();
        self.kanun.reset();
        self.kick.reset();
    }
}

fn build_carpet_tick_highlights(
    full_seq: &[RenderEntry],
    phrases: &[Phrase],
    bar_samples_for: &dyn Fn(usize, f64) -> usize,
) -> Vec<String> {
    let score = crate::carpet::WeaveScore::from_phrases(phrases);
    let layout = crate::carpet::score_border_layout(&score);
    let positions: HashMap<(usize, usize), crate::carpet::BorderTickLayout> = layout
        .into_iter()
        .map(|tick| ((tick.phrase_id, tick.score_tick), tick))
        .collect();
    let tick_counts: HashMap<usize, usize> = score
        .phrases
        .iter()
        .map(|phrase| (phrase.phrase_id, phrase.tick_count))
        .collect();
    let jump_cells = crate::carpet::jump_link_cells(phrases);
    let mut parts = Vec::new();
    let mut sample = 0usize;
    for entry in full_seq {
        let phrase = &phrases[entry.phrase_idx];
        let subdiv_secs = 60.0 / (entry.bpm * 2.0);
        let bar_samples = bar_samples_for(entry.phrase_idx, entry.bpm);
        let score_ticks = tick_counts.get(&phrase.id).copied().unwrap_or(1).max(1);
        for subdivision in 0..phrase.bar.events.len() {
            let score_tick = subdivision % score_ticks;
            let Some(layout) = positions.get(&(phrase.id, score_tick)) else {
                continue;
            };
            let start = sample as f64 / SR + subdivision as f64 * subdiv_secs;
            let end = start + subdiv_secs - 0.0001;
            let outer = if layout.is_kick { 36 } else { 26 };
            let inner = if layout.is_kick { 18 } else { 13 };
            let xo = layout.x.round() as i32 - outer / 2;
            let yo = layout.y.round() as i32 - outer / 2;
            let xi = layout.x.round() as i32 - inner / 2;
            let yi = layout.y.round() as i32 - inner / 2;
            parts.push(format!("drawbox=x={xo}:y={yo}:w={outer}:h={outer}:color=0x44FF88@0.20:t=fill:enable='between(t,{start:.6},{end:.6})',drawbox=x={xi}:y={yi}:w={inner}:h={inner}:color=0xD8FFAA@0.82:t=fill:enable='between(t,{start:.6},{end:.6})'"));
            if subdivision == 0 {
                if let Some(jump_id) = entry.arrived_via_jump {
                    for (ji, cell) in jump_cells
                        .iter()
                        .filter(|cell| cell.jump_id == jump_id)
                        .enumerate()
                    {
                        if ji % 7 != 0 {
                            continue;
                        }
                        let size = (cell.size + 3).max(8);
                        let x = cell.x.round() as i32 - size / 2;
                        let y = cell.y.round() as i32 - size / 2;
                        parts.push(format!("drawbox=x={x}:y={y}:w={size}:h={size}:color=0xD8B060@0.70:t=fill:enable='between(t,{start:.6},{end:.6})'"));
                    }
                }
            }
        }
        sample += bar_samples;
    }
    parts
}

fn expand_one_cycle(
    phrases: &[Phrase],
    start_bpm: f64,
    start_sustain: f64,
    start_vcf: VcfBank,
    start_fx: FxSettings,
) -> (Vec<RenderOccurrence>, Vec<HashMap<usize, (usize, usize)>>) {
    let mut out = Vec::new();
    let mut snapshots = Vec::new();
    let mut cur = 0usize;
    let mut jc: HashMap<usize, usize> = HashMap::new();
    let mut bpm = start_bpm;
    let mut sustain = start_sustain;
    let mut vcf = start_vcf;
    let mut fx = start_fx;
    let mut arrived_via_jump = None;
    let max_items = phrases.len() * 512 + 1;
    while out.len() < max_items {
        if cur >= phrases.len() {
            break;
        }
        let phrase = &phrases[cur];
        if let Some(js) = &phrase.jump {
            let pid = phrase.id;
            let remaining = jc.entry(pid).or_insert(js.times.saturating_sub(1));
            if *remaining > 0 {
                *remaining -= 1;
                let target = phrases
                    .iter()
                    .position(|p| p.id == js.target_id)
                    .unwrap_or(0)
                    .min(phrases.len().saturating_sub(1));
                let ids: Vec<usize> = if target < cur {
                    phrases[target..cur]
                        .iter()
                        .filter_map(|p| p.jump.as_ref().map(|_| p.id))
                        .collect()
                } else {
                    vec![]
                };
                for id in ids {
                    jc.remove(&id);
                }
                cur = target;
                arrived_via_jump = Some(pid);
            } else {
                jc.remove(&pid);
                cur += 1;
            }
            continue;
        }
        if let Some(ctrl) = phrase.control {
            match ctrl {
                ControlSpec::SetBpm(v) => bpm = v,
                ControlSpec::SetSustain(v) => sustain = v,
                ControlSpec::SetVcf(v) => {
                    if let Ok(setting) = crate::command::apply_vcf_change(vcf, v) {
                        vcf.apply(setting);
                    }
                }
                ControlSpec::SetFx(v) => {
                    if let Ok(setting) = crate::command::apply_fx_change(fx, v) {
                        fx = setting;
                    }
                }
            }
            cur += 1;
            continue;
        }
        let snap: HashMap<usize, (usize, usize)> = phrases
            .iter()
            .filter_map(|p| {
                p.jump.as_ref().map(|js| {
                    let remaining = jc.get(&p.id).copied().unwrap_or(js.times.saturating_sub(1));
                    let pass = js.times.saturating_sub(remaining);
                    (p.id, (pass, js.times))
                })
            })
            .collect();
        out.push(RenderOccurrence {
            phrase_idx: cur,
            snap_idx: snapshots.len(),
            bpm,
            sustain,
            vcf,
            fx,
            arrived_via_jump,
        });
        arrived_via_jump = None;
        snapshots.push(snap);
        cur += 1;
    }
    (out, snapshots)
}

#[allow(unused_variables)]
pub fn record_cycle(
    phrases: Vec<Phrase>,
    bpm: f64,
    sustain: f64,
    vcf: VcfBank,
    fx: FxSettings,
    cycle_repeat: usize,
) -> anyhow::Result<String> {
    if phrases.is_empty() {
        return Err(anyhow::anyhow!("nothing to record"));
    }
    let bar_samples_for = |idx: usize, bpm: f64| -> usize {
        let subdiv_secs = 60.0 / (bpm * 2.0);
        let subdiv_samples = SR * subdiv_secs;
        ((subdiv_samples * phrases[idx].bar.total_subdivs as f64).round() as usize).max(1)
    };
    let (one_cycle_seq, one_cycle_snaps) = expand_one_cycle(&phrases, bpm, sustain, vcf, fx);
    if one_cycle_seq.is_empty() {
        return Err(anyhow::anyhow!("no musical phrases to render"));
    }
    let cycles = cycle_repeat.max(1);
    let mut tail_sustain = sustain;
    let mut full_seq = Vec::new();
    for _ in 0..cycles {
        for occ in &one_cycle_seq {
            let idx = occ.phrase_idx;
            tail_sustain = occ.sustain;
            for play in 0..phrases[idx].repeat.max(1) {
                full_seq.push(RenderEntry {
                    phrase_idx: idx,
                    play_num: play,
                    snap_idx: occ.snap_idx,
                    bpm: occ.bpm,
                    sustain: occ.sustain,
                    vcf: occ.vcf,
                    fx: occ.fx,
                    arrived_via_jump: if play == 0 {
                        occ.arrived_via_jump
                    } else {
                        None
                    },
                });
            }
        }
    }
    let tail_samples = (SR * (tail_sustain + 1.0)) as usize;
    let render_samples = full_seq
        .iter()
        .map(|entry| bar_samples_for(entry.phrase_idx, entry.bpm))
        .sum::<usize>()
        + tail_samples;
    crate::REC_SAMPLES_TOTAL.store(render_samples, std::sync::atomic::Ordering::Relaxed);
    crate::REC_SAMPLES_DONE.store(0, std::sync::atomic::Ordering::Relaxed);
    crate::REC_ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);
    let mut phrases_v = phrases.to_vec();
    let mut voices: Vec<Voice> = Vec::new();
    let mut filters = FilterBank::new(SR as f32);
    let mut fx_processor = FxProcessor::new(SR as f32);
    let mut left_buf: Vec<f32> = Vec::new();
    let mut right_buf: Vec<f32> = Vec::new();
    for (seq_pos, mut entry) in full_seq.iter().copied().enumerate() {
        let phrase_idx = entry.phrase_idx;
        let play_num = entry.play_num;
        let bs = bar_samples_for(phrase_idx, entry.bpm);
        let is_first = play_num == 0;
        let repeats = phrases_v[phrase_idx].repeat.max(1);
        let subdiv_secs = 60.0 / (entry.bpm * 2.0);
        let subdiv_samples = SR * subdiv_secs;
        let sustain = entry.sustain;
        filters.set_bank(entry.vcf);
        fx_processor.set_settings(entry.fx);
        if is_first {
            let root_hz = phrases_v[phrase_idx].bar.root_hz;
            let phrase_secs =
                (phrases_v[phrase_idx].bar.total_subdivs as f64 * subdiv_secs * repeats as f64)
                    .min(3.0);
            spawn_phrase_start(root_hz, sustain, &mut voices);
            spawn_sub_bass(root_hz, phrase_secs, &mut voices);
        }
        let total_subdivs = phrases_v[phrase_idx].bar.total_subdivs;
        let mut bar_pos = 0usize;
        let mut last_subdiv = None;
        for _ in 0..bs {
            yield_to_audio_thread(left_buf.len());
            let ev = if total_subdivs > 0 {
                let curr = ((bar_pos as f64 / subdiv_samples) as usize).min(total_subdivs - 1);
                let ev = if last_subdiv != Some(curr) {
                    last_subdiv = Some(curr);
                    let is_last_play = play_num + 1 >= repeats;
                    let is_last_subdiv = curr + 1 >= total_subdivs;
                    let next_is_different = full_seq
                        .get(seq_pos + 1)
                        .map_or(false, |next| next.phrase_idx != phrase_idx);
                    let milestone = if is_first && curr == 0 {
                        Milestone::PhraseStart
                    } else if is_last_play && is_last_subdiv {
                        if next_is_different {
                            Milestone::Turnaround
                        } else {
                            Milestone::CrossPhraseWarning
                        }
                    } else {
                        Milestone::None
                    };
                    phrases_v[phrase_idx]
                        .bar
                        .events
                        .get(curr)
                        .copied()
                        .map(|e| (e, milestone))
                } else {
                    None
                };
                bar_pos += 1;
                ev
            } else {
                None
            };
            if let Some((ev, milestone)) = ev {
                let root_hz = phrases_v[phrase_idx].bar.root_hz;
                spawn_voices(
                    ev,
                    sustain,
                    &mut voices,
                    milestone,
                    &phrases_v[phrase_idx].bar.frequencies,
                    root_hz,
                    0.25,
                );
                entry.vcf.advance_tick();
                filters.update_bank(entry.vcf);
                entry.fx.advance_tick();
                fx_processor.set_settings(entry.fx);
            }
            voices.retain(|v| !v.done);
            if voices.is_empty() {
                filters.reset();
                if entry.fx.active() {
                    let (l, r) = fx_processor.process(0.0, 0.0);
                    left_buf.push(l);
                    right_buf.push(r);
                } else {
                    left_buf.push(0.0);
                    right_buf.push(0.0);
                }
                continue;
            }
            let (mut dry_l, mut dry_r) = (0f32, 0f32);
            let (mut all_l, mut all_r) = (0f32, 0f32);
            let (mut bass_l, mut bass_r) = (0f32, 0f32);
            let (mut kanun_l, mut kanun_r) = (0f32, 0f32);
            let (mut kick_l, mut kick_r) = (0f32, 0f32);
            for v in voices.iter_mut() {
                let setting = if entry.vcf.all.enabled {
                    Some(entry.vcf.all)
                } else {
                    vcf_target_for_kind(v.kind).and_then(|target| {
                        let setting = entry.vcf.get(target);
                        setting.enabled.then_some(setting)
                    })
                };
                let s = v.sample_with_wave(SR, setting.map(|setting| setting.wave));
                let angle = (v.pan + 1.0) * std::f32::consts::FRAC_PI_4;
                let l = s * angle.cos();
                let r = s * angle.sin();
                match setting.map(|setting| setting.target) {
                    Some(VcfTarget::All) => {
                        all_l += l;
                        all_r += r;
                    }
                    Some(VcfTarget::Bass) => {
                        bass_l += l;
                        bass_r += r;
                    }
                    Some(VcfTarget::Kanun) => {
                        kanun_l += l;
                        kanun_r += r;
                    }
                    Some(VcfTarget::Kick) => {
                        kick_l += l;
                        kick_r += r;
                    }
                    None => {
                        dry_l += l;
                        dry_r += r;
                    }
                }
            }
            let (mut l, mut r) = (dry_l, dry_r);
            if entry.vcf.all.enabled {
                let filtered = filters.all.process(all_l, all_r);
                l += filtered.0;
                r += filtered.1;
            } else {
                if entry.vcf.bass.enabled {
                    let filtered = filters.bass.process(bass_l, bass_r);
                    l += filtered.0;
                    r += filtered.1;
                }
                if entry.vcf.kanun.enabled {
                    let filtered = filters.kanun.process(kanun_l, kanun_r);
                    l += filtered.0;
                    r += filtered.1;
                }
                if entry.vcf.kick.enabled {
                    let filtered = filters.kick.process(kick_l, kick_r);
                    l += filtered.0;
                    r += filtered.1;
                }
            }
            let (l, r) = if entry.fx.active() {
                fx_processor.process(l.clamp(-1.0, 1.0), r.clamp(-1.0, 1.0))
            } else {
                (l.clamp(-1.0, 1.0), r.clamp(-1.0, 1.0))
            };
            left_buf.push(l);
            right_buf.push(r);
            voices.retain(|v| !v.done);
        }
        crate::REC_SAMPLES_DONE.store(
            left_buf.len().min(render_samples),
            std::sync::atomic::Ordering::Relaxed,
        );
        evolve_bar(&mut phrases_v[phrase_idx].bar, true);
    }
    let tail_vcf = full_seq.first().map(|entry| entry.vcf).unwrap_or(vcf);
    let tail_fx = full_seq.first().map(|entry| entry.fx).unwrap_or(fx);
    filters.set_bank(tail_vcf);
    fx_processor.set_settings(tail_fx);
    if let Some(first) = full_seq.first() {
        let root_hz = phrases_v[first.phrase_idx].bar.root_hz;
        spawn_phrase_start(root_hz, first.sustain, &mut voices);
        spawn_sub_bass(root_hz, first.sustain.min(2.0), &mut voices);
    }
    for _ in 0..tail_samples {
        yield_to_audio_thread(left_buf.len());
        voices.retain(|v| !v.done);
        if voices.is_empty() {
            filters.reset();
            if tail_fx.active() {
                let (l, r) = fx_processor.process(0.0, 0.0);
                left_buf.push(l);
                right_buf.push(r);
            } else {
                left_buf.push(0.0);
                right_buf.push(0.0);
            }
            continue;
        }
        let (mut dry_l, mut dry_r) = (0f32, 0f32);
        let (mut all_l, mut all_r) = (0f32, 0f32);
        let (mut bass_l, mut bass_r) = (0f32, 0f32);
        let (mut kanun_l, mut kanun_r) = (0f32, 0f32);
        let (mut kick_l, mut kick_r) = (0f32, 0f32);
        for v in voices.iter_mut() {
            let setting = if tail_vcf.all.enabled {
                Some(tail_vcf.all)
            } else {
                vcf_target_for_kind(v.kind).and_then(|target| {
                    let setting = tail_vcf.get(target);
                    setting.enabled.then_some(setting)
                })
            };
            let s = v.sample_with_wave(SR, setting.map(|setting| setting.wave));
            let angle = (v.pan + 1.0) * std::f32::consts::FRAC_PI_4;
            let l = s * angle.cos();
            let r = s * angle.sin();
            match setting.map(|setting| setting.target) {
                Some(VcfTarget::All) => {
                    all_l += l;
                    all_r += r;
                }
                Some(VcfTarget::Bass) => {
                    bass_l += l;
                    bass_r += r;
                }
                Some(VcfTarget::Kanun) => {
                    kanun_l += l;
                    kanun_r += r;
                }
                Some(VcfTarget::Kick) => {
                    kick_l += l;
                    kick_r += r;
                }
                None => {
                    dry_l += l;
                    dry_r += r;
                }
            }
        }
        let (mut l, mut r) = (dry_l, dry_r);
        if tail_vcf.all.enabled {
            let filtered = filters.all.process(all_l, all_r);
            l += filtered.0;
            r += filtered.1;
        } else {
            if tail_vcf.bass.enabled {
                let filtered = filters.bass.process(bass_l, bass_r);
                l += filtered.0;
                r += filtered.1;
            }
            if tail_vcf.kanun.enabled {
                let filtered = filters.kanun.process(kanun_l, kanun_r);
                l += filtered.0;
                r += filtered.1;
            }
            if tail_vcf.kick.enabled {
                let filtered = filters.kick.process(kick_l, kick_r);
                l += filtered.0;
                r += filtered.1;
            }
        }
        let (l, r) = if tail_fx.active() {
            fx_processor.process(l.clamp(-1.0, 1.0), r.clamp(-1.0, 1.0))
        } else {
            (l.clamp(-1.0, 1.0), r.clamp(-1.0, 1.0))
        };
        left_buf.push(l);
        right_buf.push(r);
        voices.retain(|v| !v.done);
    }
    let peak = left_buf
        .iter()
        .chain(right_buf.iter())
        .map(|s| s.abs())
        .fold(0f32, f32::max);
    let gain = if peak > 0.001 { 0.9 / peak } else { 1.0 };
    let wav_path_s = temp_path("maqam-live.wav");
    {
        let n = left_buf.len() as u32;
        let sr = SR as u32;
        let dl = n * 4;
        let mut f = std::fs::File::create(&wav_path_s)?;
        f.write_all(b"RIFF")?;
        f.write_all(&(36 + dl).to_le_bytes())?;
        f.write_all(b"WAVE")?;
        f.write_all(b"fmt ")?;
        f.write_all(&16u32.to_le_bytes())?;
        f.write_all(&1u16.to_le_bytes())?;
        f.write_all(&2u16.to_le_bytes())?;
        f.write_all(&sr.to_le_bytes())?;
        f.write_all(&(sr * 4).to_le_bytes())?;
        f.write_all(&4u16.to_le_bytes())?;
        f.write_all(&16u16.to_le_bytes())?;
        f.write_all(b"data")?;
        f.write_all(&dl.to_le_bytes())?;
        for i in 0..left_buf.len() {
            let l = (left_buf[i] * gain * 32767.0).clamp(-32768.0, 32767.0) as i16;
            let r = (right_buf[i] * gain * 32767.0).clamp(-32768.0, 32767.0) as i16;
            f.write_all(&l.to_le_bytes())?;
            f.write_all(&r.to_le_bytes())?;
        }
        f.flush()?;
        f.sync_all()?;
    }
    let wav_path = wav_path_s.as_str();
    let total_secs = left_buf.len() as f64 / SR;
    let ass_path_s = temp_path("maqam-live.ass");
    let ass_path = ass_path_s.as_str();
    {
        let mut f = std::fs::File::create(ass_path)?;
        writeln!(f, "[Script Info]")?;
        writeln!(f, "ScriptType: v4.00+")?;
        writeln!(f, "PlayResX: 1280")?;
        writeln!(f, "PlayResY: 720")?;
        writeln!(f, "WrapStyle: 0")?;
        writeln!(f, "[V4+ Styles]")?;
        writeln!(f,"Format: Name,Fontname,Fontsize,PrimaryColour,SecondaryColour,OutlineColour,BackColour,Bold,Italic,Underline,Strikeout,ScaleX,ScaleY,Spacing,Angle,BorderStyle,Outline,Shadow,Alignment,MarginL,MarginR,MarginV,Encoding")?;
        writeln!(f,"Style: Line,Arial,24,&H00A0FF70,&H00A0FF70,&H00102004,&H00102004,-1,0,0,0,112,104,0,0,1,4,1,7,20,20,10,1")?;
        writeln!(f,"Style: URL,Arial,20,&H0078DD78,&H0078DD78,&H00102004,&H00102004,-1,0,0,0,110,102,0,0,1,3,1,1,20,20,38,1")?;
        writeln!(f, "[Events]")?;
        writeln!(
            f,
            "Format: Layer,Start,End,Style,Name,MarginL,MarginR,MarginV,Effect,Text"
        )?;
        let one_len: usize = one_cycle_seq
            .iter()
            .map(|occ| phrases[occ.phrase_idx].repeat.max(1))
            .sum();
        let fmt_t = |s: f64| -> String {
            let hh = (s / 3600.0) as u32;
            let mm = ((s % 3600.0) / 60.0) as u32;
            let ss = (s % 60.0) as u32;
            let cs = ((s % 1.0) * 100.0) as u32;
            format!("{hh}:{mm:02}:{ss:02}.{cs:02}")
        };
        let mut sample = 0usize;
        for (i, entry) in full_seq.iter().enumerate() {
            let phrase_idx = entry.phrase_idx;
            let play_num = entry.play_num;
            let snap_idx = entry.snap_idx;
            let bs = bar_samples_for(phrase_idx, entry.bpm);
            let start_s = sample as f64 / SR;
            let end_s = if i + 1 < full_seq.len() {
                (sample + bs) as f64 / SR
            } else {
                total_secs
            };
            let t0 = fmt_t(start_s);
            let t1 = fmt_t(end_s);
            let cycle_num = if one_len > 0 { i / one_len } else { 0 };
            let cycle_disp = if cycles > 1 {
                format!("  cycle {}/{}", cycle_num + 1, cycles)
            } else {
                String::new()
            };
            let hdr = format!(
                "   bpm:{:<4} sus:{:.1}s{}",
                entry.bpm.round() as u32,
                entry.sustain,
                cycle_disp
            );
            writeln!(f, "Dialogue: 0,{t0},{t1},Line,,0,0,0,,{hdr}")?;
            writeln!(
                f,
                "Dialogue: 0,{t0},{t1},URL,,0,0,0,,https://github.com/rfielding/maqam"
            )?;
            let subdiv_secs = 60.0 / (entry.bpm * 2.0);
            let line_h = 26usize;
            let mut margin_v = 30usize;
            for (pi, p) in phrases.iter().enumerate() {
                let active = p.jump.is_none() && pi == phrase_idx;
                let id = format!("{:>3}", p.id);
                if let Some(js) = &p.jump {
                    let snap = one_cycle_snaps.get(snap_idx % one_cycle_snaps.len().max(1));
                    let (pass, total) = snap
                        .and_then(|s| s.get(&p.id))
                        .copied()
                        .unwrap_or((0, js.times));
                    let counter = format!("[{}/{}]", pass.min(total), total);
                    let text = format!("- {id}: {:<20} {}", p.src, counter);
                    writeln!(f, "Dialogue: 0,{t0},{t1},Line,,0,0,{margin_v},,{text}")?;
                } else if p.control.is_some() {
                    let text = format!("- {id}: {}", p.src);
                    writeln!(f, "Dialogue: 0,{t0},{t1},Line,,0,0,{margin_v},,{text}")?;
                } else if active {
                    let label = phrase_display_title(&p.src);
                    let rhythm_plain = p.bar.rhythm_display();
                    let maqam_str = p.bar.ratio_strs.join(" | ");
                    let ctr = format!("[{}/{}]", play_num + 1, p.repeat.max(1));
                    let n = p.bar.events.len().max(1);
                    for si in 0..n {
                        let ts0 = fmt_t(start_s + si as f64 * subdiv_secs);
                        let ts1 = fmt_t((start_s + (si + 1) as f64 * subdiv_secs).min(end_s));
                        let mut rhy = String::new();
                        for (ci, ch) in rhythm_plain.chars().enumerate() {
                            if ci == si {
                                rhy.push_str(&format!("{{\\1c&H00000000&\\3c&H00FFFFFF&\\bord6\\shad0}}{ch}{{\\1c&H0000FF00&\\3c&H00000000&\\bord2\\shad0}}"));
                            } else {
                                rhy.push(ch);
                            }
                        }
                        let pad = " ".repeat(10usize.saturating_sub(rhythm_plain.chars().count()));
                        let repeat_display = if si == 0 { ctr.as_str() } else { "" };
                        let body = format!(
                            "{:<20} {}{} {:<16} {:<7}",
                            label, rhy, pad, maqam_str, repeat_display
                        );
                        let text = format!("▶ {id}: {body}");
                        writeln!(f, "Dialogue: 0,{ts0},{ts1},Line,,0,0,{margin_v},,{text}")?;
                    }
                    let phrase_end_s = start_s + n as f64 * subdiv_secs;
                    if phrase_end_s < end_s {
                        let ts0 = fmt_t(phrase_end_s);
                        let body = format!(
                            "{:<20} {:<10} {:<16} {}",
                            label, rhythm_plain, maqam_str, ctr
                        );
                        let text = format!("> {id}: {body}");
                        writeln!(f, "Dialogue: 0,{ts0},{t1},Line,,0,0,{margin_v},,{text}")?;
                    }
                } else {
                    let label = phrase_display_title(&p.src);
                    let rhythm = p.bar.rhythm_display();
                    let maqam_str = p.bar.ratio_strs.join(" | ");
                    let ctr = format!("[1/{}]", p.repeat.max(1));
                    let body = format!("{:<20} {:<10} {:<16} {}", label, rhythm, maqam_str, ctr);
                    let text = format!("- {id}: {body}");
                    writeln!(f, "Dialogue: 0,{t0},{t1},Line,,0,0,{margin_v},,{text}")?;
                }
                margin_v += line_h;
            }
            sample += bs;
        }
        f.flush()?;
    }
    let result = (|| -> anyhow::Result<String> {
        let carpet_path = temp_path("maqam-carpet.ppm");
        crate::carpet::write_carpet_background(&carpet_path, &[], &phrases)?;
        let tick_highlights = build_carpet_tick_highlights(&full_seq, &phrases, &bar_samples_for);
        let highlight_chain = if tick_highlights.is_empty() {
            "null".to_string()
        } else {
            tick_highlights.join(",")
        };
        let filter_with_subs=format!("[1:v]{highlight_chain}[carpet];[0:a]showwaves=s=1280x360:mode=cline:rate=30:colors=0x20140C,pad=1280:720:0:360:color=black,colorkey=0x000000:0.04:0.25,format=rgba,colorchannelmixer=aa=0.16[wv];[carpet][wv]overlay=format=auto[base];[base]subtitles={ass_path}[v]");
        let filter_plain=format!("[1:v]{highlight_chain}[carpet];[0:a]showwaves=s=1280x360:mode=cline:rate=30:colors=0x20140C,pad=1280:720:0:360:color=black,colorkey=0x000000:0.04:0.25,format=rgba,colorchannelmixer=aa=0.16[wv];[carpet][wv]overlay=format=auto[v]");
        let fscript_path = temp_path("maqam-filter.txt");
        std::fs::write(&fscript_path, &filter_with_subs)?;
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let out = format!("./maqam-{ts}.mp4");
        let log_path = temp_path("maqam-ffmpeg.log");
        let ok1 = ffmpeg_status(
            ffmpeg_command()
                .args([
                    "-y",
                    "-i",
                    wav_path,
                    "-loop",
                    "1",
                    "-framerate",
                    "30",
                    "-i",
                    &carpet_path,
                    "-filter_complex_script",
                    &fscript_path,
                    "-map",
                    "[v]",
                    "-map",
                    "0:a",
                    "-c:v",
                    "libx264",
                    "-preset",
                    "veryfast",
                    "-threads",
                    "1",
                    "-crf",
                    "18",
                    "-pix_fmt",
                    "yuv420p",
                    "-movflags",
                    "+faststart",
                    "-c:a",
                    "aac",
                    "-b:a",
                    "320k",
                    "-r",
                    "30",
                    "-shortest",
                    &out,
                ])
                .stdout(Stdio::null())
                .stderr(
                    std::fs::File::create(&log_path)
                        .map(Stdio::from)
                        .unwrap_or(Stdio::null()),
                ),
        )?;
        if !ok1 {
            ffmpeg_status(
                ffmpeg_command()
                    .args([
                        "-y",
                        "-i",
                        wav_path,
                        "-loop",
                        "1",
                        "-framerate",
                        "30",
                        "-i",
                        &carpet_path,
                        "-filter_complex",
                        &filter_plain,
                        "-map",
                        "[v]",
                        "-map",
                        "0:a",
                        "-c:v",
                        "libx264",
                        "-preset",
                        "veryfast",
                        "-threads",
                        "1",
                        "-crf",
                        "18",
                        "-pix_fmt",
                        "yuv420p",
                        "-movflags",
                        "+faststart",
                        "-c:a",
                        "aac",
                        "-b:a",
                        "320k",
                        "-r",
                        "30",
                        "-shortest",
                        &out,
                    ])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null()),
            )?;
        }
        Ok(out)
    })();
    crate::REC_ACTIVE.store(false, std::sync::atomic::Ordering::Relaxed);
    crate::REC_SAMPLES_DONE.store(render_samples, std::sync::atomic::Ordering::Relaxed);
    result
}

fn vcf_target_for_kind(kind: VoiceKind) -> Option<VcfTarget> {
    match kind {
        VoiceKind::SubBass => Some(VcfTarget::Bass),
        VoiceKind::MelodyFm => Some(VcfTarget::Kanun),
        VoiceKind::FloorTom => Some(VcfTarget::Kick),
        _ => None,
    }
}
