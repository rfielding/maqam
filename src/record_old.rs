#![allow(dead_code)]

// record_old.rs — offline render to MP4

use std::collections::HashMap;
use std::io::Write;
use std::process::{Command, Stdio};

use crate::fx::{FxProcessor, FxSettings};
use crate::sequencer::{ControlSpec, Phrase};
use crate::synth::{evolve_bar, spawn_phrase_start, spawn_voices, Milestone, Voice, VoiceKind};
use crate::vcf::{MoogLadder, VcfBank, VcfSettings, VcfTarget};

const SR: f64 = 44100.0;
const RENDER_COOP_INTERVAL_SAMPLES: usize = 4096;
const ASS_MONO_FONT: &str = "DejaVu Sans Mono";

type HighlightRangeKey = (u8, i32, i32, i32, i32, &'static str);
type JumpCounterSnapshot = HashMap<usize, (usize, usize)>;

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
                "video rendering requires ffmpeg on your PATH; install ffmpeg, or add it to PATH, then run m again"
            )
        }
        Err(err) => Err(err.into()),
    }
}

fn ensure_ffmpeg_available() -> anyhow::Result<()> {
    match Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(status) if status.success() => Ok(()),
        Ok(_) => anyhow::bail!(
            "ffmpeg is installed but did not run successfully; run ffmpeg -version in your shell, fix that error, then run m again"
        ),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => anyhow::bail!(
            "video rendering requires ffmpeg on your PATH; install ffmpeg, or add it to PATH, then run m again"
        ),
        Err(err) => anyhow::bail!(
            "could not check ffmpeg: {err}; make sure ffmpeg runs from your shell, then run m again"
        ),
    }
}

fn yield_to_audio_thread(rendered_samples: usize) {
    if rendered_samples != 0 && rendered_samples % RENDER_COOP_INTERVAL_SAMPLES == 0 {
        std::thread::sleep(std::time::Duration::from_micros(250));
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
    vcf_generation: usize,
    fx_generation: usize,
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
    vcf_generation: usize,
    fx_generation: usize,
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
    mic: StereoFilter,
    bass: StereoFilter,
    kanun: StereoFilter,
    kick: StereoFilter,
    tanbura: StereoFilter,
}

impl FilterBank {
    fn new(sr: f32) -> Self {
        Self {
            all: StereoFilter::new(sr),
            mic: StereoFilter::new(sr),
            bass: StereoFilter::new(sr),
            kanun: StereoFilter::new(sr),
            kick: StereoFilter::new(sr),
            tanbura: StereoFilter::new(sr),
        }
    }

    fn set_bank(&mut self, bank: VcfBank) {
        self.all.set_settings(bank.all);
        self.mic.set_settings(bank.mic);
        self.bass.set_settings(bank.bass);
        self.kanun.set_settings(bank.kanun);
        self.kick.set_settings(bank.kick);
        self.tanbura.set_settings(bank.tanbura);
    }

    fn update_bank(&mut self, bank: VcfBank) {
        self.all.update_settings(bank.all);
        self.mic.update_settings(bank.mic);
        self.bass.update_settings(bank.bass);
        self.kanun.update_settings(bank.kanun);
        self.kick.update_settings(bank.kick);
        self.tanbura.update_settings(bank.tanbura);
    }

    fn reset(&mut self) {
        self.all.reset();
        self.mic.reset();
        self.bass.reset();
        self.kanun.reset();
        self.kick.reset();
        self.tanbura.reset();
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
        .iter()
        .copied()
        .map(|tick| ((tick.phrase_id, tick.score_tick), tick))
        .collect();
    let tick_counts: HashMap<usize, usize> = score
        .phrases
        .iter()
        .map(|phrase| (phrase.phrase_id, phrase.tick_count))
        .collect();
    let jump_cells = crate::carpet::jump_link_cells(phrases);
    // A long recording can revisit the same border cells thousands of times.
    // Giving every visit its own drawbox creates a deeply chained ffmpeg graph
    // which can crash with SIGBUS while being configured on macOS.  Keep one
    // drawbox per visual cell and combine all of its active time ranges.
    let mut ranges: HashMap<HighlightRangeKey, Vec<(f64, f64)>> = HashMap::new();
    let total_secs = full_seq
        .iter()
        .map(|entry| bar_samples_for(entry.phrase_idx, entry.bpm))
        .sum::<usize>() as f64
        / SR;
    // Base score cells are neutral. Dynamic phrase state is layered over them
    // below: next phrase first, then the current subdivision.
    for tick in &layout {
        let outer = if tick.is_kick { 6 } else { 4 };
        let inner = if tick.is_kick { 3 } else { 2 };
        let xo = tick.x.round() as i32 - outer / 2;
        let yo = tick.y.round() as i32 - outer / 2;
        let xi = tick.x.round() as i32 - inner / 2;
        let yi = tick.y.round() as i32 - inner / 2;
        ranges
            .entry((0, xo, yo, outer, outer, "0x666666@0.72"))
            .or_default()
            .push((0.0, total_secs));
        ranges
            .entry((1, xi, yi, inner, inner, "0xA0A0A0@0.88"))
            .or_default()
            .push((0.0, total_secs));
    }
    let mut sample = 0usize;
    for (entry_index, entry) in full_seq.iter().enumerate() {
        let phrase = &phrases[entry.phrase_idx];
        let subdiv_secs = 60.0 / (entry.bpm * 2.0);
        let bar_samples = bar_samples_for(entry.phrase_idx, entry.bpm);
        let entry_start = sample as f64 / SR;
        let entry_end = (sample + bar_samples) as f64 / SR - 0.0001;
        let next_phrase_idx = full_seq[entry_index + 1..]
            .iter()
            .find(|next| next.phrase_idx != entry.phrase_idx)
            .map(|next| next.phrase_idx);
        if let Some(next_phrase_idx) = next_phrase_idx {
            let next_id = phrases[next_phrase_idx].id;
            for tick in layout.iter().filter(|tick| tick.phrase_id == next_id) {
                let outer = if tick.is_kick { 6 } else { 4 };
                let inner = if tick.is_kick { 3 } else { 2 };
                let xo = tick.x.round() as i32 - outer / 2;
                let yo = tick.y.round() as i32 - outer / 2;
                let xi = tick.x.round() as i32 - inner / 2;
                let yi = tick.y.round() as i32 - inner / 2;
                ranges
                    .entry((2, xo, yo, outer, outer, "0x8080FF@0.42"))
                    .or_default()
                    .push((entry_start, entry_end));
                ranges
                    .entry((3, xi, yi, inner, inner, "0xD0D0FF@0.88"))
                    .or_default()
                    .push((entry_start, entry_end));
            }
        }
        let score_ticks = tick_counts.get(&phrase.id).copied().unwrap_or(1).max(1);
        for subdivision in 0..phrase.bar.events.len() {
            let score_tick = subdivision % score_ticks;
            let Some(layout) = positions.get(&(phrase.id, score_tick)) else {
                continue;
            };
            let start = sample as f64 / SR + subdivision as f64 * subdiv_secs;
            let end = start + subdiv_secs - 0.0001;
            let outer = if layout.is_kick { 6 } else { 4 };
            let inner = if layout.is_kick { 3 } else { 2 };
            let xo = layout.x.round() as i32 - outer / 2;
            let yo = layout.y.round() as i32 - outer / 2;
            let xi = layout.x.round() as i32 - inner / 2;
            let yi = layout.y.round() as i32 - inner / 2;
            ranges
                .entry((4, xo, yo, outer, outer, "0x44FF88@0.20"))
                .or_default()
                .push((start, end));
            ranges
                .entry((5, xi, yi, inner, inner, "0xD8FFAA@0.82"))
                .or_default()
                .push((start, end));
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
                        ranges
                            .entry((6, x, y, size, size, "0xD8B060@0.70"))
                            .or_default()
                            .push((start, end));
                    }
                }
            }
        }
        sample += bar_samples;
    }
    let mut grouped: Vec<_> = ranges.into_iter().collect();
    grouped.sort_by_key(|(style, _)| *style);
    grouped
        .into_iter()
        .flat_map(|((_layer, x, y, w, h, color), ranges)| {
            ranges
                .chunks(64)
                .map(|chunk| {
                    let enable = chunk
                        .iter()
                        .copied()
                        .map(|(start, end)| format!("between(t,{start:.6},{end:.6})"))
                        .collect::<Vec<_>>()
                        .join("+");
                    format!(
                        "drawbox=x={x}:y={y}:w={w}:h={h}:color={color}:t=fill:enable='{enable}'"
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn build_center_gosper_rotation_expr(
    full_seq: &[RenderEntry],
    cycle_count: usize,
    phrases: &[Phrase],
    bar_samples_for: &dyn Fn(usize, f64) -> usize,
) -> String {
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
    let entries_per_cycle = full_seq.len() / cycle_count.max(1);
    let cycle_entries = &full_seq[..entries_per_cycle.max(1).min(full_seq.len())];
    let cycle_samples: usize = cycle_entries
        .iter()
        .map(|entry| bar_samples_for(entry.phrase_idx, entry.bpm))
        .sum();
    let cycle_secs = cycle_samples as f64 / SR;
    let clock = if cycle_count > 1 && cycle_secs > 0.0 {
        format!("mod(t,{cycle_secs:.6})")
    } else {
        "t".to_string()
    };
    let mut terms = Vec::new();
    let mut sample = 0usize;
    for entry in cycle_entries {
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
            let angle = (((layout.start_t + layout.end_t) * 0.5) as f64) * std::f64::consts::TAU;
            terms.push(format!("{angle:.6}*between({clock},{start:.6},{end:.6})"));
        }
        sample += bar_samples;
    }
    while terms.len() > 1 {
        terms = terms
            .chunks(2)
            .map(|pair| match pair {
                [left, right] => format!("({left}+{right})"),
                [only] => only.clone(),
                _ => unreachable!(),
            })
            .collect();
    }
    terms.pop().unwrap_or_else(|| "0".to_string())
}

fn expand_one_cycle(
    phrases: &[Phrase],
    start_bpm: f64,
    start_sustain: f64,
    start_vcf: VcfBank,
    start_fx: FxSettings,
) -> (Vec<RenderOccurrence>, Vec<JumpCounterSnapshot>) {
    let mut out = Vec::new();
    let mut snapshots = Vec::new();
    let mut cur = 0usize;
    let mut jc: HashMap<usize, usize> = HashMap::new();
    let mut bpm = start_bpm;
    let mut sustain = start_sustain;
    let mut vcf = start_vcf;
    let mut fx = start_fx;
    let mut vcf_generation = 0usize;
    let mut fx_generation = 0usize;
    let mut arrived_via_jump = None;
    let max_items = phrases.len() * 512 + 1;
    while out.len() < max_items {
        if cur >= phrases.len() {
            break;
        }
        let phrase = &phrases[cur];
        if let Some(js) = &phrase.jump {
            let pid = phrase.id;
            let limit = js.times.max(1);
            let value = jc.entry(pid).or_insert(0);
            let incremented = value.saturating_add(1);
            if incremented < limit {
                *value = incremented;
                let target = phrases
                    .iter()
                    .position(|p| p.id == js.target_id)
                    .unwrap_or(0)
                    .min(phrases.len().saturating_sub(1));
                cur = target;
                arrived_via_jump = Some(pid);
            } else {
                *value = 0;
                cur += 1;
            }
            continue;
        }
        if let Some(ctrl) = phrase.control {
            match ctrl {
                ControlSpec::Stop => break,
                ControlSpec::SetBpm(v) => bpm = v,
                ControlSpec::SetSustain(v) => sustain = v,
                ControlSpec::SetVcf(v) => {
                    if let Ok(setting) = crate::command::apply_vcf_change(vcf, v) {
                        vcf.apply(setting);
                        vcf_generation += 1;
                    }
                }
                ControlSpec::SetFx(v) => {
                    if let Ok(setting) = crate::command::apply_fx_change(fx, v) {
                        fx = setting;
                        fx_generation += 1;
                    }
                }
                ControlSpec::SetSympathetics(_)
                | ControlSpec::SetSympatheticDecay(_)
                | ControlSpec::SetSympatheticGain(_)
                | ControlSpec::SetSympathetic(_)
                | ControlSpec::SetNamEnabled(_)
                | ControlSpec::SetNamGain(_)
                | ControlSpec::SetNamInput(_) => {}
            }
            cur += 1;
            continue;
        }
        let snap: HashMap<usize, (usize, usize)> = phrases
            .iter()
            .filter_map(|p| {
                p.jump.as_ref().map(|js| {
                    let pass = jc.get(&p.id).copied().unwrap_or(0);
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
            vcf_generation,
            fx_generation,
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
        return Err(anyhow::anyhow!(
            "nothing to record; add a phrase first, then run m again"
        ));
    }
    ensure_ffmpeg_available()?;
    let bar_samples_for = |idx: usize, bpm: f64| -> usize {
        let subdiv_secs = 60.0 / (bpm * 2.0);
        let subdiv_samples = SR * subdiv_secs;
        ((subdiv_samples * phrases[idx].bar.total_subdivs as f64).round() as usize).max(1)
    };
    let (one_cycle_seq, one_cycle_snaps) = expand_one_cycle(&phrases, bpm, sustain, vcf, fx);
    if one_cycle_seq.is_empty() {
        return Err(anyhow::anyhow!(
            "no musical phrases to render; add a non-control phrase first, then run m again"
        ));
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
                    vcf_generation: occ.vcf_generation,
                    fx_generation: occ.fx_generation,
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
    let mut render_vcf = full_seq.first().map(|entry| entry.vcf).unwrap_or(vcf);
    let mut render_fx = full_seq.first().map(|entry| entry.fx).unwrap_or(fx);
    let mut render_vcf_generation = full_seq
        .first()
        .map(|entry| entry.vcf_generation)
        .unwrap_or(0);
    let mut render_fx_generation = full_seq
        .first()
        .map(|entry| entry.fx_generation)
        .unwrap_or(0);
    filters.set_bank(render_vcf);
    fx_processor.set_settings(render_fx);
    for (seq_pos, entry) in full_seq.iter().copied().enumerate() {
        let phrase_idx = entry.phrase_idx;
        let play_num = entry.play_num;
        let bs = bar_samples_for(phrase_idx, entry.bpm);
        let is_first = play_num == 0;
        let repeats = phrases_v[phrase_idx].repeat.max(1);
        let subdiv_secs = 60.0 / (entry.bpm * 2.0);
        let subdiv_samples = SR * subdiv_secs;
        let sustain = entry.sustain;
        if entry.vcf_generation != render_vcf_generation {
            render_vcf = entry.vcf;
            render_vcf_generation = entry.vcf_generation;
            filters.set_bank(render_vcf);
        }
        if entry.fx_generation != render_fx_generation {
            render_fx = entry.fx;
            render_fx_generation = entry.fx_generation;
            fx_processor.set_settings(render_fx);
        }
        if is_first {
            let root_hz = phrases_v[phrase_idx].bar.root_hz;
            spawn_phrase_start(root_hz, sustain, &mut voices);
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
                        .is_some_and(|next| next.phrase_idx != phrase_idx);
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
                render_vcf.advance_tick();
                filters.update_bank(render_vcf);
                render_fx.advance_tick();
                fx_processor.set_settings(render_fx);
            }
            voices.retain(|v| !v.done);
            if voices.is_empty() {
                filters.reset();
                if render_fx.active() {
                    let (l, r) = fx_processor.process(0.0, 0.0);
                    let (l, r) = crate::analog::soft_clip_stereo(l, r);
                    let (l, r) = if render_vcf.all.enabled {
                        filters.all.process(l, r)
                    } else {
                        (l, r)
                    };
                    left_buf.push(l.clamp(-1.0, 1.0));
                    right_buf.push(r.clamp(-1.0, 1.0));
                } else {
                    left_buf.push(0.0);
                    right_buf.push(0.0);
                }
                continue;
            }
            let (mut dry_l, mut dry_r) = (0f32, 0f32);
            let (mut mic_l, mut mic_r) = (0f32, 0f32);
            let (mut bass_l, mut bass_r) = (0f32, 0f32);
            let (mut kanun_l, mut kanun_r) = (0f32, 0f32);
            let (mut kick_l, mut kick_r) = (0f32, 0f32);
            let (mut tanbura_l, mut tanbura_r) = (0f32, 0f32);
            for v in voices.iter_mut() {
                let setting = if render_vcf.all.enabled {
                    None
                } else {
                    vcf_target_for_kind(v.kind).and_then(|target| {
                        let setting = render_vcf.get(target);
                        setting.enabled.then_some(setting)
                    })
                };
                let s = v.sample_with_wave(
                    SR,
                    setting.and_then(|setting| {
                        (setting.target != VcfTarget::All)
                            .then_some(setting.wave)
                            .and_then(|wave| wave.oscillator())
                    }),
                );
                let angle = (v.pan + 1.0) * std::f32::consts::FRAC_PI_4;
                let l = s * angle.cos();
                let r = s * angle.sin();
                match setting.map(|setting| setting.target) {
                    Some(VcfTarget::All) => unreachable!("master VCF is applied after mix"),
                    Some(VcfTarget::Mic) => {
                        mic_l += l;
                        mic_r += r;
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
                    Some(VcfTarget::Tanbura) => {
                        tanbura_l += l;
                        tanbura_r += r;
                    }
                    None => {
                        dry_l += l;
                        dry_r += r;
                    }
                }
            }
            let (mut l, mut r) = (dry_l, dry_r);
            if !render_vcf.all.enabled {
                if render_vcf.mic.enabled {
                    let filtered = filters.mic.process(mic_l, mic_r);
                    l += filtered.0;
                    r += filtered.1;
                }
                if render_vcf.bass.enabled {
                    let filtered = filters.bass.process(bass_l, bass_r);
                    l += filtered.0;
                    r += filtered.1;
                }
                if render_vcf.kanun.enabled {
                    let filtered = filters.kanun.process(kanun_l, kanun_r);
                    l += filtered.0;
                    r += filtered.1;
                }
                if render_vcf.kick.enabled {
                    let filtered = filters.kick.process(kick_l, kick_r);
                    l += filtered.0;
                    r += filtered.1;
                }
                if render_vcf.tanbura.enabled {
                    let filtered = filters.tanbura.process(tanbura_l, tanbura_r);
                    l += filtered.0;
                    r += filtered.1;
                }
            }
            let (l, r) = if render_fx.active() {
                fx_processor.process(l, r)
            } else {
                (l, r)
            };
            let (l, r) = crate::analog::soft_clip_stereo(l, r);
            let (l, r) = if render_vcf.all.enabled {
                filters.all.process(l, r)
            } else {
                (l, r)
            };
            left_buf.push(l.clamp(-1.0, 1.0));
            right_buf.push(r.clamp(-1.0, 1.0));
            voices.retain(|v| !v.done);
        }
        crate::REC_SAMPLES_DONE.store(
            left_buf.len().min(render_samples),
            std::sync::atomic::Ordering::Relaxed,
        );
        evolve_bar(&mut phrases_v[phrase_idx].bar, true);
    }
    let tail_vcf = render_vcf;
    let tail_fx = render_fx;
    filters.set_bank(tail_vcf);
    fx_processor.set_settings(tail_fx);
    if let Some(first) = full_seq.first() {
        let root_hz = phrases_v[first.phrase_idx].bar.root_hz;
        spawn_phrase_start(root_hz, first.sustain, &mut voices);
    }
    for _ in 0..tail_samples {
        yield_to_audio_thread(left_buf.len());
        voices.retain(|v| !v.done);
        if voices.is_empty() {
            filters.reset();
            if tail_fx.active() {
                let (l, r) = fx_processor.process(0.0, 0.0);
                let (l, r) = crate::analog::soft_clip_stereo(l, r);
                let (l, r) = if tail_vcf.all.enabled {
                    filters.all.process(l, r)
                } else {
                    (l, r)
                };
                left_buf.push(l.clamp(-1.0, 1.0));
                right_buf.push(r.clamp(-1.0, 1.0));
            } else {
                left_buf.push(0.0);
                right_buf.push(0.0);
            }
            continue;
        }
        let (mut dry_l, mut dry_r) = (0f32, 0f32);
        let (mut mic_l, mut mic_r) = (0f32, 0f32);
        let (mut bass_l, mut bass_r) = (0f32, 0f32);
        let (mut kanun_l, mut kanun_r) = (0f32, 0f32);
        let (mut kick_l, mut kick_r) = (0f32, 0f32);
        let (mut tanbura_l, mut tanbura_r) = (0f32, 0f32);
        for v in voices.iter_mut() {
            let setting = if tail_vcf.all.enabled {
                None
            } else {
                vcf_target_for_kind(v.kind).and_then(|target| {
                    let setting = tail_vcf.get(target);
                    setting.enabled.then_some(setting)
                })
            };
            let s = v.sample_with_wave(
                SR,
                setting.and_then(|setting| {
                    (setting.target != VcfTarget::All)
                        .then_some(setting.wave)
                        .and_then(|wave| wave.oscillator())
                }),
            );
            let angle = (v.pan + 1.0) * std::f32::consts::FRAC_PI_4;
            let l = s * angle.cos();
            let r = s * angle.sin();
            match setting.map(|setting| setting.target) {
                Some(VcfTarget::All) => unreachable!("master VCF is applied after mix"),
                Some(VcfTarget::Mic) => {
                    mic_l += l;
                    mic_r += r;
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
                Some(VcfTarget::Tanbura) => {
                    tanbura_l += l;
                    tanbura_r += r;
                }
                None => {
                    dry_l += l;
                    dry_r += r;
                }
            }
        }
        let (mut l, mut r) = (dry_l, dry_r);
        if !tail_vcf.all.enabled {
            if tail_vcf.mic.enabled {
                let filtered = filters.mic.process(mic_l, mic_r);
                l += filtered.0;
                r += filtered.1;
            }
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
            if tail_vcf.tanbura.enabled {
                let filtered = filters.tanbura.process(tanbura_l, tanbura_r);
                l += filtered.0;
                r += filtered.1;
            }
        }
        let (l, r) = if tail_fx.active() {
            fx_processor.process(l, r)
        } else {
            (l, r)
        };
        let (l, r) = crate::analog::soft_clip_stereo(l, r);
        let (l, r) = if tail_vcf.all.enabled {
            filters.all.process(l, r)
        } else {
            (l, r)
        };
        left_buf.push(l.clamp(-1.0, 1.0));
        right_buf.push(r.clamp(-1.0, 1.0));
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
        writeln!(f, "WrapStyle: 2")?;
        writeln!(f, "[V4+ Styles]")?;
        writeln!(f,"Format: Name,Fontname,Fontsize,PrimaryColour,SecondaryColour,OutlineColour,BackColour,Bold,Italic,Underline,Strikeout,ScaleX,ScaleY,Spacing,Angle,BorderStyle,Outline,Shadow,Alignment,MarginL,MarginR,MarginV,Encoding")?;
        writeln!(f,"Style: Line,{ASS_MONO_FONT},24,&H00000000,&H00000000,&H00102004,&H00102004,-1,0,0,0,100,100,0,0,1,0,0,7,20,20,10,1")?;
        writeln!(f,"Style: URL,Arial,20,&H0078DD78,&H0078DD78,&H00102004,&H00102004,-1,0,0,0,110,102,0,0,1,3,1,1,20,20,38,1")?;
        writeln!(f,"Style: JumpCounter,{ASS_MONO_FONT},18,&H00909090,&H00909090,&H00102004,&H00102004,-1,0,0,0,100,100,0,0,1,3,1,5,0,0,0,1")?;
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
        // libass trims leading spaces and may collapse padding used for the
        // TUI's fixed columns.  ASS hard spaces preserve the exact row string.
        let preserve_row_spaces = |text: String| text.replace(' ', r"\h");
        let mut sample = 0usize;
        let phrase_positions: HashMap<usize, usize> = phrases
            .iter()
            .enumerate()
            .map(|(index, phrase)| (phrase.id, index))
            .collect();
        let text_jump_routes: Vec<(usize, usize, usize, usize)> = phrases
            .iter()
            .enumerate()
            .filter_map(|(source, phrase)| {
                let jump = phrase.jump.as_ref()?;
                let target = phrase_positions.get(&jump.target_id).copied()?;
                (target != source).then_some((target, source, phrase.id, jump.times))
            })
            .collect();
        let status_width = phrases.iter().fold("[settings]".len(), |width, phrase| {
            let total = phrase
                .jump
                .as_ref()
                .map_or(phrase.repeat.max(1), |jump| jump.times);
            width.max(format!("[{total}/{total}]").len())
        });
        let jump_counter_positions = crate::carpet::jump_counter_layout(&phrases);
        let final_phrase_idx = full_seq.last().map(|entry| entry.phrase_idx);
        let explicit_stop_idx = phrases
            .iter()
            .position(|phrase| matches!(phrase.control, Some(ControlSpec::Stop)));
        for (i, entry) in full_seq.iter().enumerate() {
            let phrase_idx = entry.phrase_idx;
            let stopping = i + 1 == full_seq.len();
            let next_phrase_idx = full_seq[i + 1..]
                .iter()
                .find(|next| next.phrase_idx != phrase_idx)
                .map(|next| next.phrase_idx)
                .or_else(|| stopping.then_some(explicit_stop_idx).flatten());
            let play_num = entry.play_num;
            let snap_idx = entry.snap_idx;
            let bs = bar_samples_for(phrase_idx, entry.bpm);
            let start_s = sample as f64 / SR;
            let end_s = (sample + bs) as f64 / SR;
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
            writeln!(
                f,
                "Dialogue: 2,{t0},{t1},Line,,0,0,0,,{{\\1c&H00A0FF70&}}{hdr}"
            )?;
            writeln!(
                f,
                "Dialogue: 2,{t0},{t1},URL,,0,0,0,,https://github.com/rfielding/maqam"
            )?;
            let subdiv_secs = 60.0 / (entry.bpm * 2.0);
            let line_h = 26usize;
            let mut margin_v = 30usize;
            let snap = one_cycle_snaps.get(snap_idx % one_cycle_snaps.len().max(1));
            for counter in &jump_counter_positions {
                let Some(jump) = phrases
                    .iter()
                    .find(|phrase| phrase.id == counter.jump_id)
                    .and_then(|phrase| phrase.jump.as_ref())
                else {
                    continue;
                };
                let (pass, total) = snap
                    .and_then(|state| state.get(&counter.jump_id))
                    .copied()
                    .unwrap_or((0, jump.times));
                let counter_text = format!(
                    "{{\\pos({:.0},{:.0})}}[{}/{}]",
                    counter.x,
                    counter.y,
                    pass.saturating_add(1).min(total.max(1)),
                    total
                );
                writeln!(
                    f,
                    "Dialogue: 1,{t0},{t1},JumpCounter,,0,0,0,,{counter_text}"
                )?;
            }
            let upcoming_jump_source = {
                let mut position = (phrase_idx + 1) % phrases.len().max(1);
                let mut found = None;
                for _ in 0..phrases.len() {
                    let phrase = &phrases[position];
                    if phrase.jump.is_some() {
                        found = Some(position);
                        break;
                    }
                    if phrase.control.is_none() {
                        break;
                    }
                    position = (position + 1) % phrases.len();
                }
                found
            };
            let display_order: Vec<usize> = (0..phrases.len()).collect();
            let display_positions: HashMap<usize, usize> = display_order
                .iter()
                .enumerate()
                .map(|(display_position, &original_position)| (original_position, display_position))
                .collect();
            let display_jump_routes: Vec<(usize, usize, usize, usize)> = text_jump_routes
                .iter()
                .map(|&(target, source, jump_id, times)| {
                    (
                        display_positions[&target],
                        display_positions[&source],
                        jump_id,
                        times,
                    )
                })
                .collect();
            let upcoming_jump_source =
                upcoming_jump_source.and_then(|source| display_positions.get(&source).copied());
            let jump_prefix_for = |display_pi: usize| -> String {
                display_jump_routes
                    .iter()
                    .map(|&(target, source, jump_id, times)| {
                        let on_path =
                            target.min(source) <= display_pi && display_pi <= target.max(source);
                        if !on_path {
                            return "    ";
                        }
                        let (pass, total) = snap
                            .and_then(|state| state.get(&jump_id))
                            .copied()
                            .unwrap_or((0, times));
                        let will_jump = Some(source) == upcoming_jump_source
                            && pass.saturating_add(1) < total.max(1);
                        if display_pi == target {
                            if target < source {
                                "┌──>"
                            } else {
                                "└──>"
                            }
                        } else if display_pi == source
                            && will_jump
                            && play_num + 1 >= phrases[phrase_idx].repeat.max(1)
                        {
                            "●   "
                        } else {
                            "│   "
                        }
                    })
                    .collect()
            };
            for (display_pi, &pi) in display_order.iter().enumerate() {
                let p = &phrases[pi];
                let active = p.jump.is_none() && pi == phrase_idx;
                let color = if active {
                    "{\\1c&H0000FF00&}"
                } else if Some(pi) == next_phrase_idx {
                    "{\\1c&H00FF8080&}"
                } else {
                    "{\\1c&H00909090&}"
                };
                // One isolated two-cell marker column.  Keep it outside the
                // jump lanes and counters so every following field remains on
                // the same monospaced tab regardless of state.
                let row_guard = "•";
                let leaving_current = play_num + 1 >= phrases[phrase_idx].repeat.max(1);
                let marker_head = if active {
                    '▶'
                } else if Some(pi) == next_phrase_idx {
                    if leaving_current {
                        '▸'
                    } else {
                        '▷'
                    }
                } else {
                    '·'
                };
                let marker = format!("{marker_head} ");
                let id = format!("{:>3}: ", p.id);
                let jump_prefix = jump_prefix_for(display_pi);
                if let Some(js) = &p.jump {
                    let (pass, total) = snap
                        .and_then(|s| s.get(&p.id))
                        .copied()
                        .unwrap_or((0, js.times));
                    let counter = format!(
                        "{:<status_width$} ",
                        format!(
                            "[{}{}{}]",
                            pass.saturating_add(1).min(total.max(1)),
                            "/",
                            total
                        )
                    );
                    let error = if phrase_positions.contains_key(&js.target_id) {
                        ""
                    } else {
                        "  [missing target]"
                    };
                    let text = format!(
                        "{row_guard}{color}{id}{marker}{jump_prefix}{counter}{}{error}",
                        p.display_src()
                    );
                    let text = preserve_row_spaces(text);
                    writeln!(f, "Dialogue: 2,{t0},{t1},Line,,0,0,{margin_v},,{text}")?;
                } else if p.control.is_some() {
                    let status = if matches!(p.control, Some(ControlSpec::Stop)) {
                        "[stop]"
                    } else {
                        "[settings]"
                    };
                    let text = format!(
                        "{row_guard}{color}{id}{marker}{jump_prefix}{:<status_width$} {}",
                        status,
                        p.display_src()
                    );
                    let text = preserve_row_spaces(text);
                    writeln!(f, "Dialogue: 2,{t0},{t1},Line,,0,0,{margin_v},,{text}")?;
                } else if active {
                    let label = p.display_src();
                    let ratios = p.pitch_ratios_display();
                    let rhythm_plain = p.bar.rhythm_display();
                    let ctr = format!("[{}/{}]", play_num + 1, p.repeat.max(1));
                    let n = p.bar.events.len().max(1);
                    for si in 0..n {
                        let ts0 = fmt_t(start_s + si as f64 * subdiv_secs);
                        let ts1 = fmt_t((start_s + (si + 1) as f64 * subdiv_secs).min(end_s));
                        let mut rhy = String::new();
                        for (ci, ch) in rhythm_plain.chars().enumerate() {
                            if ci == si {
                                rhy.push_str(&format!("{{\\1c&H00000000&\\3c&H00FFFFFF&\\bord6\\shad0}}{ch}{{\\1c&H0000FF00&\\3c&H00000000&\\bord0\\shad0}}"));
                            } else {
                                rhy.push(ch);
                            }
                        }
                        let body = if ratios.is_empty() {
                            format!("{ctr:<status_width$} {:<28} {}", label, rhy)
                        } else {
                            format!("{ctr:<status_width$} {:<28} {}  {}", label, rhy, ratios)
                        };
                        let text = format!("{row_guard}{color}{id}{marker}{jump_prefix}{body}");
                        let text = preserve_row_spaces(text);
                        writeln!(f, "Dialogue: 2,{ts0},{ts1},Line,,0,0,{margin_v},,{text}")?;
                    }
                    let phrase_end_s = start_s + n as f64 * subdiv_secs;
                    if phrase_end_s < end_s {
                        let ts0 = fmt_t(phrase_end_s);
                        let body = if ratios.is_empty() {
                            format!("{ctr:<status_width$} {:<28} {}", label, rhythm_plain)
                        } else {
                            format!(
                                "{ctr:<status_width$} {:<28} {}  {}",
                                label, rhythm_plain, ratios
                            )
                        };
                        let text = format!("{row_guard}{color}{id}{marker}{jump_prefix}{body}");
                        let text = preserve_row_spaces(text);
                        writeln!(f, "Dialogue: 2,{ts0},{t1},Line,,0,0,{margin_v},,{text}")?;
                    }
                } else {
                    let label = p.display_src();
                    let ratios = p.pitch_ratios_display();
                    let rhythm = p.bar.rhythm_display();
                    let total = p.repeat.max(1);
                    let ctr = format!("[1/{total}]");
                    let ctr = format!("{ctr:<status_width$}");
                    let body = if ratios.is_empty() {
                        format!("{ctr} {:<28} {}", label, rhythm)
                    } else {
                        format!("{ctr} {:<28} {}  {}", label, rhythm, ratios)
                    };
                    let text = format!("{row_guard}{color}{id}{marker}{jump_prefix}{body}");
                    let text = preserve_row_spaces(text);
                    writeln!(f, "Dialogue: 2,{t0},{t1},Line,,0,0,{margin_v},,{text}")?;
                }
                margin_v += line_h;
                if Some(pi) == final_phrase_idx && explicit_stop_idx.is_none() {
                    let stop_color = if stopping {
                        "{\\1c&H00FF8080&}"
                    } else {
                        "{\\1c&H00909090&}"
                    };
                    let stop_marker = if stopping { "▸ " } else { "· " };
                    let stop_lanes = jump_prefix_for(display_pi + 1);
                    let stop_status = format!("{:<status_width$} ", "[stop]");
                    let stop_text = preserve_row_spaces(format!(
                        "•{stop_color}---: {stop_marker}{stop_lanes}{stop_status}stop"
                    ));
                    writeln!(f, "Dialogue: 2,{t0},{t1},Line,,0,0,{margin_v},,{stop_text}")?;
                    margin_v += line_h;
                }
            }
            sample += bs;
        }
        // The renderer tail is a real virtual stop step.  It begins only
        // after sequence expansion has processed the final jump and gives
        // users a visible current command to stop on.
        let stop_start = fmt_t(sample as f64 / SR);
        let stop_end = fmt_t(total_secs);
        if sample as f64 / SR < total_secs {
            let stop_lanes = explicit_stop_idx.map_or_else(
                || "    ".repeat(text_jump_routes.len()),
                |stop_idx| {
                    text_jump_routes
                        .iter()
                        .map(|&(target, source, _jump_id, _times)| {
                            if target.min(source) <= stop_idx && stop_idx <= target.max(source) {
                                if stop_idx == target {
                                    if target < source {
                                        "┌──>"
                                    } else {
                                        "└──>"
                                    }
                                } else {
                                    "│   "
                                }
                            } else {
                                "    "
                            }
                        })
                        .collect::<String>()
                },
            );
            let stop_status = format!("{:<status_width$} ", "[stop]");
            let (stop_id, stop_source) = explicit_stop_idx
                .map(|index| {
                    (
                        format!("{:>3}: ", phrases[index].id),
                        phrases[index].display_src(),
                    )
                })
                .unwrap_or_else(|| ("---: ".into(), "stop".into()));
            let stop_text = preserve_row_spaces(format!(
                "•{{\\1c&H0000FF00&}}{stop_id}▶ {stop_lanes}{stop_status}{stop_source}"
            ));
            let stop_margin = 30 + (phrases.len() / 2) * 26;
            writeln!(
                f,
                "Dialogue: 2,{stop_start},{stop_end},Line,,0,0,{stop_margin},,{stop_text}"
            )?;
        }
        f.flush()?;
    }
    let result = (|| -> anyhow::Result<String> {
        let carpet_path = temp_path("maqam-carpet.ppm");
        crate::carpet::write_carpet_background(&carpet_path, &[], &phrases)?;
        let gosper_path = temp_path("maqam-gosper.ppm");
        crate::carpet::write_center_gosper_overlay(&gosper_path, &phrases)?;
        let jumps_path = temp_path("maqam-jumps.ppm");
        crate::carpet::write_jump_arrows_overlay(&jumps_path, &phrases)?;
        let tick_highlights = build_carpet_tick_highlights(&full_seq, &phrases, &bar_samples_for);
        let highlight_chain = if tick_highlights.is_empty() {
            "null".to_string()
        } else {
            tick_highlights.join(",")
        };
        let gosper_rotation =
            build_center_gosper_rotation_expr(&full_seq, cycles, &phrases, &bar_samples_for);
        let gosper_chain = format!(
            "[2:v]format=rgba,colorkey=0x000000:0.02:0.05,rotate='{gosper_rotation}':ow=iw:oh=ih:c=black@0[gosper];[3:v]format=rgba,colorkey=0x000000:0.003:0.02[jumps];[1:v][gosper]overlay=format=auto[carpet0];[carpet0][jumps]overlay=format=auto[carpet1];[carpet1]{highlight_chain}[carpet]"
        );
        let filter_with_subs=format!("{gosper_chain};[0:a]showwaves=s=1280x360:mode=cline:rate=30:colors=0x20140C,pad=1280:720:0:360:color=black,colorkey=0x000000:0.04:0.25,format=rgba,colorchannelmixer=aa=0.16[wv];[carpet][wv]overlay=format=auto[base];[base]subtitles={ass_path}[v]");
        let filter_plain=format!("{gosper_chain};[0:a]showwaves=s=1280x360:mode=cline:rate=30:colors=0x20140C,pad=1280:720:0:360:color=black,colorkey=0x000000:0.04:0.25,format=rgba,colorchannelmixer=aa=0.16[wv];[carpet][wv]overlay=format=auto[v]");
        let fscript_path = temp_path("maqam-filter.txt");
        std::fs::write(&fscript_path, &filter_with_subs)?;
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let out = format!("./maqam-{ts}.mp4");
        // ffmpeg creates its output before it has written the MP4's moov atom.
        // Encode to a hidden staging file so a failed or interrupted recording
        // is never presented to the user as a finished movie.
        let staged_out = format!("./.maqam-{ts}.partial.mp4");
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
                    "-loop",
                    "1",
                    "-framerate",
                    "30",
                    "-i",
                    &gosper_path,
                    "-loop",
                    "1",
                    "-framerate",
                    "30",
                    "-i",
                    &jumps_path,
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
                    &staged_out,
                ])
                .stdout(Stdio::null())
                .stderr(
                    std::fs::File::create(&log_path)
                        .map(Stdio::from)
                        .unwrap_or(Stdio::null()),
                ),
        )?;
        let encoded = if ok1 {
            true
        } else {
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
                        "-loop",
                        "1",
                        "-framerate",
                        "30",
                        "-i",
                        &gosper_path,
                        "-loop",
                        "1",
                        "-framerate",
                        "30",
                        "-i",
                        &jumps_path,
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
                        &staged_out,
                    ])
                    .stdout(Stdio::null())
                    .stderr(
                        std::fs::File::create(&log_path)
                            .map(Stdio::from)
                            .unwrap_or(Stdio::null()),
                    ),
            )?
        };
        if !encoded {
            let _ = std::fs::remove_file(&staged_out);
            anyhow::bail!(
                "ffmpeg failed to create MP4; read {log_path}, fix the ffmpeg error shown there, then run m again"
            );
        }
        std::fs::rename(&staged_out, &out)?;
        Ok(out)
    })();
    crate::REC_ACTIVE.store(false, std::sync::atomic::Ordering::Relaxed);
    crate::REC_SAMPLES_DONE.store(render_samples, std::sync::atomic::Ordering::Relaxed);
    result
}

fn vcf_target_for_kind(kind: VoiceKind) -> Option<VcfTarget> {
    match kind {
        VoiceKind::Bass => Some(VcfTarget::Bass),
        VoiceKind::MelodyFm => Some(VcfTarget::Kanun),
        VoiceKind::FloorTom | VoiceKind::HiHat | VoiceKind::Kick => Some(VcfTarget::Kick),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::FxSettings;
    use crate::vcf::VcfBank;

    #[test]
    fn empty_recording_error_tells_user_what_to_do() {
        let err = record_cycle(
            Vec::new(),
            120.0,
            1.25,
            VcfBank::default(),
            FxSettings::default(),
            1,
        )
        .unwrap_err()
        .to_string();

        assert_eq!(
            err,
            "nothing to record; add a phrase first, then run m again"
        );
    }

    #[test]
    fn missing_ffmpeg_error_tells_user_what_to_do() {
        let mut cmd = Command::new("maqam-live-definitely-missing-ffmpeg-test-binary");
        let err = ffmpeg_status(&mut cmd).unwrap_err().to_string();

        assert_eq!(
            err,
            "video rendering requires ffmpeg on your PATH; install ffmpeg, or add it to PATH, then run m again"
        );
    }
}
