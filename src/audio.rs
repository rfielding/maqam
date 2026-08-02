// audio.rs — three-level hierarchy: subdivision → group → phrase
//
// Level 1: every subdivision fires a melody note (degree walk)
// Level 2: kicks land on structural group degrees (higher-level melody)
// Level 3: phrase start = root note rings long (highest-level melody)
//          phrase end   = turnaround accent (highest-level rhythm marker)

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::Receiver;
use std::sync::Arc;

use crate::command::{
    NamInput, SympatheticChange, SympatheticHarmony, SympatheticTarget, VcfChange,
};
use crate::fx::{FxProcessor, FxSettings};
use crate::sequencer::{AudioCmd, ControlSpec, Phrase, SubdivEvent};
use crate::sympathetics::SympatheticStrings;
use crate::synth::{
    evolve_bar, spawn_phrase_start, spawn_sub_bass, spawn_voices, Milestone, Voice, VoiceKind,
};

use crate::vcf::{MoogLadder, VcfBank, VcfSettings, VcfTarget};

// ── Playback state ────────────────────────────────────────────────────────────

struct BarState {
    subdiv_samples: f64,
    bar_pos: usize,
    last_subdiv: Option<usize>,
}

struct PlayingPhrase {
    phrase: Phrase,
    bar_states: Vec<BarState>,
    plays_done: usize,
}

impl PlayingPhrase {
    fn new(phrase: Phrase, sr: f64, bpm: f64) -> Self {
        let bar_states = make_bar_states(&phrase, sr, bpm);
        PlayingPhrase {
            phrase,
            bar_states,
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
    Bpm(f64),
    Sustain(f64),
    Vcf(VcfChange),
    Fx(crate::command::FxChange),
    NamEnabled(bool),
    NamGain(f32),
    NamInput(NamInput),
    Sympathetics(bool),
    SympatheticDecay(f32),
    SympatheticGain(f32),
    Sympathetic(SympatheticChange),
}

#[derive(Clone, Copy)]
struct SympatheticPartitionSettings {
    enabled: bool,
    amount: f32,
}

struct SympatheticPartition {
    strings: SympatheticStrings,
    settings: SympatheticPartitionSettings,
}

impl SympatheticPartition {
    fn new(sr: f32, amount: f32) -> Self {
        Self {
            strings: SympatheticStrings::new(sr),
            settings: SympatheticPartitionSettings {
                enabled: true,
                amount,
            },
        }
    }
}

struct SympatheticBank {
    mic: SympatheticPartition,
    kanun: SympatheticPartition,
    bass: SympatheticPartition,
    drums: SympatheticPartition,
    target_frequencies: Vec<f64>,
    interval_ratio: f64,
    harmony: Option<SympatheticHarmony>,
}

impl SympatheticBank {
    fn new(sr: f32) -> Self {
        Self {
            mic: SympatheticPartition::new(sr, 1.0),
            kanun: SympatheticPartition::new(sr, 0.0),
            bass: SympatheticPartition::new(sr, 0.0),
            drums: SympatheticPartition::new(sr, 0.0),
            target_frequencies: Vec::new(),
            interval_ratio: 1.0,
            harmony: None,
        }
    }

    fn set_targets(&mut self, frequencies: &[f64]) {
        self.target_frequencies = frequencies.to_vec();
        self.apply_targets();
    }

    fn apply_targets(&mut self) {
        if let Some(harmony) = self.harmony {
            let weighted = self.weighted_targets(harmony);
            self.mic.strings.set_weighted_targets(&weighted);
            self.kanun.strings.set_weighted_targets(&weighted);
            self.bass.strings.set_weighted_targets(&weighted);
            self.drums.strings.set_weighted_targets(&weighted);
        } else {
            let shifted: Vec<f64> = self
                .target_frequencies
                .iter()
                .map(|frequency| frequency * self.interval_ratio)
                .collect();
            self.mic.strings.set_targets(&shifted);
            self.kanun.strings.set_targets(&shifted);
            self.bass.strings.set_targets(&shifted);
            self.drums.strings.set_targets(&shifted);
        }
    }

    #[contracts::debug_requires(harmony.len > 0, "weighted harmony has at least one component")]
    #[contracts::debug_ensures(
        ret.iter().all(|(_, weight)| *weight >= 0.0),
        "target weights are non-negative"
    )]
    fn weighted_targets(&self, harmony: SympatheticHarmony) -> Vec<(f64, f32)> {
        let total_weight: f32 = harmony
            .iter()
            .map(|component| component.weight.max(0.0))
            .sum::<f32>()
            .max(1.0);
        self.target_frequencies
            .iter()
            .flat_map(|frequency| {
                harmony.iter().map(move |component| {
                    (
                        frequency * component.ratio,
                        component.weight.max(0.0) / total_weight,
                    )
                })
            })
            .collect()
    }

    fn has_energy(&self) -> bool {
        self.mic.strings.has_energy()
            || self.kanun.strings.has_energy()
            || self.bass.strings.has_energy()
            || self.drums.strings.has_energy()
    }

    fn process(
        &mut self,
        master_enabled: bool,
        mic_input: f32,
        kanun_input: f32,
        bass_input: f32,
        drums_input: f32,
    ) -> f32 {
        let mic = process_sym_partition(&mut self.mic, master_enabled, mic_input);
        let kanun = process_sym_partition(&mut self.kanun, master_enabled, kanun_input);
        let bass = process_sym_partition(&mut self.bass, master_enabled, bass_input);
        let drums = process_sym_partition(&mut self.drums, master_enabled, drums_input);
        mic + kanun + bass + drums
    }

    fn apply_change(&mut self, change: SympatheticChange, master_enabled: &mut bool) {
        if change.target.is_none() {
            if let Some(value) = change.enabled {
                *master_enabled = value;
            }
        }

        match change.target.unwrap_or(SympatheticTarget::All) {
            SympatheticTarget::All => {
                for target in [
                    SympatheticTarget::Mic,
                    SympatheticTarget::Kanun,
                    SympatheticTarget::Bass,
                    SympatheticTarget::Drums,
                ] {
                    self.apply_partition_change(target, change);
                }
            }
            target => self.apply_partition_change(target, change),
        }

        if let Some(value) = change.mic {
            self.mic.settings.amount = value;
        }
        if let Some(value) = change.kanun {
            self.kanun.settings.amount = value;
        }
        if let Some(value) = change.bass {
            self.bass.settings.amount = value;
        }
        if let Some(value) = change.drums {
            self.drums.settings.amount = value;
        }
        if let Some(ratio) = change.interval_ratio {
            self.interval_ratio = ratio;
            self.harmony = None;
            self.apply_targets();
        }
        if let Some(harmony) = change.harmony {
            self.harmony = Some(harmony);
            self.apply_targets();
        }
    }

    fn apply_partition_change(&mut self, target: SympatheticTarget, change: SympatheticChange) {
        let partition = match target {
            SympatheticTarget::All => return,
            SympatheticTarget::Mic => &mut self.mic,
            SympatheticTarget::Kanun => &mut self.kanun,
            SympatheticTarget::Bass => &mut self.bass,
            SympatheticTarget::Drums => &mut self.drums,
        };
        if change.target.is_some() {
            if let Some(value) = change.enabled {
                partition.settings.enabled = value;
            }
        }
        if let Some(value) = change.decay {
            partition.strings.set_decay(value);
        }
        if let Some(value) = change.gain {
            partition.strings.set_input_gain(value);
        }
        if let Some(value) = change.amount {
            partition.settings.amount = value;
        }
    }
}

fn process_sym_partition(
    partition: &mut SympatheticPartition,
    master_enabled: bool,
    input: f32,
) -> f32 {
    let driven = if master_enabled && partition.settings.enabled {
        input * partition.settings.amount
    } else {
        0.0
    };
    partition.strings.process(driven)
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

    fn apply(&mut self, settings: VcfSettings) {
        match settings.target {
            VcfTarget::All => self.all.set_settings(settings),
            VcfTarget::Mic => self.mic.set_settings(settings),
            VcfTarget::Bass => self.bass.set_settings(settings),
            VcfTarget::Kanun => self.kanun.set_settings(settings),
            VcfTarget::Kick => self.kick.set_settings(settings),
            VcfTarget::Tanbura => self.tanbura.set_settings(settings),
        }
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

fn make_bar_states(phrase: &Phrase, sr: f64, bpm: f64) -> Vec<BarState> {
    let subdiv_secs = 60.0 / (bpm * 2.0);
    std::iter::once(&phrase.bar)
        .map(|_| {
            let subdiv_samples = sr * subdiv_secs;
            BarState {
                subdiv_samples,
                bar_pos: 0,
                last_subdiv: None,
            }
        })
        .collect()
}

#[derive(Clone, Copy, Debug)]
struct DcBlocker {
    x1: f32,
    y1: f32,
}

impl DcBlocker {
    fn new() -> Self {
        Self { x1: 0.0, y1: 0.0 }
    }

    fn reset(&mut self) {
        self.x1 = 0.0;
        self.y1 = 0.0;
    }

    fn process(&mut self, x: f32) -> f32 {
        let y = x - self.x1 + 0.995 * self.y1;
        self.x1 = x;
        self.y1 = y;
        y
    }
}

// ── Public entry point ────────────────────────────────────────────────────────

pub struct AudioStreams {
    _output: cpal::Stream,
    _input: Option<cpal::Stream>,
}

fn choose_output_config(
    device: &cpal::Device,
    preferred_sample_rate: Option<u32>,
) -> anyhow::Result<cpal::SupportedStreamConfig> {
    let default = device.default_output_config()?;
    let requested =
        std::env::var("MAQAM_SAMPLE_RATE")
            .ok()
            .and_then(|value| match value.parse::<u32>() {
                Ok(rate) => Some(rate),
                Err(_) => {
                    eprintln!(
                    "MAQAM_SAMPLE_RATE must be a number like 48000; using the audio device default"
                );
                    None
                }
            });

    if let Some(rate) = requested {
        if let Some(config) = supported_output_config_at_rate(device, rate) {
            return Ok(config);
        }
        eprintln!(
            "audio output device does not support {rate} Hz; use a supported MAQAM_SAMPLE_RATE or unset it"
        );
    }

    if requested.is_none() {
        if let Some(rate) = preferred_sample_rate.filter(|rate| *rate != default.sample_rate().0) {
            if let Some(config) = supported_output_config_at_rate(device, rate) {
                eprintln!("audio output: selecting {rate} Hz to match NAM model");
                return Ok(config);
            }
            eprintln!(
                "audio output device does not support NAM model rate {rate} Hz; using the audio device default"
            );
        }
    }

    Ok(default)
}

fn supported_output_config_at_rate(
    device: &cpal::Device,
    rate: u32,
) -> Option<cpal::SupportedStreamConfig> {
    let mut configs: Vec<_> = device.supported_output_configs().ok()?.collect();
    configs.sort_by_key(|config| {
        (
            config.sample_format() != cpal::SampleFormat::F32,
            config.channels() != 2,
        )
    });
    configs
        .into_iter()
        .find_map(|config| config.try_with_sample_rate(cpal::SampleRate(rate)))
}

fn supported_input_config_at_rate(
    device: &cpal::Device,
    rate: cpal::SampleRate,
) -> Option<cpal::SupportedStreamConfig> {
    device
        .supported_input_configs()
        .ok()?
        .find(|config| config.sample_format() == cpal::SampleFormat::F32)
        .and_then(|config| config.try_with_sample_rate(rate))
}

pub fn start_audio_with_preferred_sample_rate(
    rx: Receiver<AudioCmd>,
    preferred_sample_rate: Option<u32>,
) -> anyhow::Result<AudioStreams> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| anyhow::anyhow!("no audio output device"))?;
    let cfg = choose_output_config(&device, preferred_sample_rate)?;
    let sr = cfg.sample_rate().0 as f64;
    let ch = cfg.channels() as usize;
    eprintln!(
        "audio output: {} Hz, {} channels, {:?}",
        cfg.sample_rate().0,
        cfg.channels(),
        cfg.sample_format()
    );
    crate::AUDIO_OUTPUT_SAMPLE_RATE_HZ
        .store(cfg.sample_rate().0, std::sync::atomic::Ordering::Relaxed);
    crate::AUDIO_INPUT_SAMPLE_RATE_HZ.store(0, std::sync::atomic::Ordering::Relaxed);
    let latest_input = Arc::new([
        std::sync::atomic::AtomicU32::new(0.0f32.to_bits()),
        std::sync::atomic::AtomicU32::new(0.0f32.to_bits()),
    ]);
    let input_writer = Arc::clone(&latest_input);
    let (input_time_tx, input_time_rx) = crossbeam_channel::bounded::<cpal::StreamInstant>(64);
    let input_stream = host.default_input_device().and_then(|input_device| {
        let input_config = supported_input_config_at_rate(&input_device, cfg.sample_rate())
            .or_else(|| input_device.default_input_config().ok())?;
        if input_config.sample_format() != cpal::SampleFormat::F32 {
            return None;
        }
        if input_config.sample_rate() != cfg.sample_rate() {
            eprintln!(
                "input sample rate {} Hz does not match output {} Hz; live input/NAM input disabled",
                input_config.sample_rate().0,
                cfg.sample_rate().0
            );
            return None;
        }
        let input_channels = input_config.channels() as usize;
        crate::AUDIO_INPUT_SAMPLE_RATE_HZ.store(
            input_config.sample_rate().0,
            std::sync::atomic::Ordering::Relaxed,
        );
        let stream = input_device
            .build_input_stream(
                &input_config.into(),
                move |data: &[f32], info| {
                    let captured_at = info.timestamp().capture;
                    if let Some(frame) = data.chunks(input_channels.max(1)).last() {
                        let left = frame.first().copied().unwrap_or(0.0);
                        let right = frame.get(1).copied().unwrap_or(left);
                        input_writer[0]
                            .store(left.to_bits(), std::sync::atomic::Ordering::Relaxed);
                        input_writer[1]
                            .store(right.to_bits(), std::sync::atomic::Ordering::Relaxed);
                    }
                    let _ = input_time_tx.try_send(captured_at);
                },
                |_error| {},
                None,
            )
            .ok()?;
        stream.play().ok()?;
        Some(stream)
    });

    let mut phrases: Vec<PlayingPhrase> = Vec::new();
    let mut voices: Vec<Voice> = Vec::new();
    let mut cur_phrase: usize = 0;
    let mut bpm = 120.0f64;
    let mut sustain = 1.25f64;
    let mut vol = 1.0f32;
    let mut vcf = VcfBank::default();
    let mut vcf_filters = FilterBank::new(sr as f32);
    let mut fx = FxSettings::default();
    let mut fx_processor = FxProcessor::new(sr as f32);
    let mut paused = false;
    let mut jump_counters: std::collections::HashMap<usize, usize> =
        std::collections::HashMap::new();
    let mut pending_next_id: Option<usize> = None;
    let mut sympathetics_enabled = false;
    let mut sympathetics = SympatheticBank::new(sr as f32);
    let mut sympathetic_phrase_id = None;
    let mut nam_model: Option<nam_rs::Model> = None;
    let mut nam_enabled = true;
    let mut nam_gain = 0.05f32;
    let mut nam_input = NamInput::Stereo;
    let mut nam_dc_blocker = DcBlocker::new();
    let mut nam_warmup_samples = 0usize;
    let mut nam_fault_samples = 0usize;
    let mut latency_test: Option<crossbeam_channel::Sender<Result<f64, String>>> = None;
    let mut latency_ms_ema: Option<f64> = None;
    let mut meter_peaks = [0.0f32; 3];
    let mut last_metrics_publish = std::time::Instant::now();

    let stream = device.build_output_stream(
        &cfg.into(),
        move |data: &mut [f32], info| {
            let playback_at = info.timestamp().playback;
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
                    AudioCmd::SetVcfBank(v) => {
                        vcf = v;
                        vcf_filters.set_bank(v);
                    }
                    AudioCmd::SetVcf(change) => {
                        if let Ok(setting) = crate::command::apply_vcf_change(vcf, change) {
                            vcf.apply(setting);
                            vcf_filters.apply(setting);
                        }
                    }
                    AudioCmd::SetFxSettings(v) => {
                        fx = v;
                        fx_processor.set_settings(v);
                    }
                    AudioCmd::SetFx(change) => {
                        if let Ok(setting) = crate::command::apply_fx_change(fx, change) {
                            fx = setting;
                            fx_processor.set_settings(setting);
                        }
                    }
                    AudioCmd::SetNamModel(model) => {
                        nam_model = model;
                        nam_enabled = nam_model.is_some();
                        nam_dc_blocker.reset();
                        nam_warmup_samples =
                            nam_model.as_ref().map(|model| model.receptive_field()).unwrap_or(0);
                        nam_fault_samples = 0;
                        crate::clear_nam_error();
                        crate::NAM_MODEL_ACTIVE
                            .store(nam_enabled, std::sync::atomic::Ordering::Relaxed);
                        crate::NAM_STATUS.store(
                            if nam_enabled { 1 } else { 0 },
                            std::sync::atomic::Ordering::Relaxed,
                        );
                    }
                    AudioCmd::SetNamEnabled(enabled) => {
                        nam_enabled = enabled;
                        crate::clear_nam_error();
                        crate::NAM_MODEL_ACTIVE.store(
                            enabled && nam_model.is_some(),
                            std::sync::atomic::Ordering::Relaxed,
                        );
                        crate::NAM_STATUS.store(
                            if nam_model.is_none() {
                                0
                            } else if enabled {
                                1
                            } else {
                                5
                            },
                            std::sync::atomic::Ordering::Relaxed,
                        );
                    }
                    AudioCmd::SetNamGain(gain) => {
                        nam_gain = gain.clamp(0.0, 8.0);
                    }
                    AudioCmd::SetNamInput(route) => nam_input = route,
                    AudioCmd::MeasureInputLatency { input, result_tx } => {
                        let _ = input;
                        latency_test = Some(result_tx);
                    }
                    AudioCmd::SetVol(v) => {
                        vol = v;
                    }
                    AudioCmd::SetPaused(p) => {
                        paused = p;
                    }
                    AudioCmd::SetCurPhrase(pos) => {
                        if pos < phrases.len() {
                            pending_next_id = None;
                            cur_phrase = pos;
                            // Reset the target phrase to its very beginning
                            phrases[pos].reset();
                            crate::CUR_PHRASE
                                .store(cur_phrase, std::sync::atomic::Ordering::Relaxed);
                            crate::CUR_SUBDIV.store(0, std::sync::atomic::Ordering::Relaxed);
                            crate::CUR_PLAYS.store(0, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                    AudioCmd::QueueNextPhrase(id) => {
                        if let Some(pos) = phrases.iter().position(|p| p.phrase.id == id) {
                            pending_next_id = Some(id);
                            crate::EXIT_PHRASE.store(pos, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                    AudioCmd::SetSympathetics(enabled) => {
                        sympathetics_enabled = enabled;
                    }
                    AudioCmd::SetSympatheticDecay(decay) => {
                        sympathetics.apply_change(
                            SympatheticChange {
                                decay: Some(decay),
                                ..SympatheticChange::default()
                            },
                            &mut sympathetics_enabled,
                        );
                    }
                    AudioCmd::SetSympatheticGain(gain) => {
                        sympathetics.apply_change(
                            SympatheticChange {
                                gain: Some(gain),
                                ..SympatheticChange::default()
                            },
                            &mut sympathetics_enabled,
                        );
                    }
                    AudioCmd::SetSympathetic(change) => {
                        sympathetics.apply_change(change, &mut sympathetics_enabled);
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
                            let first = phrases.remove(0);
                            phrases.push(first);
                            if let Some(pid) = playing_id {
                                cur_phrase =
                                    phrases.iter().position(|p| p.phrase.id == pid).unwrap_or(0);
                                crate::CUR_PHRASE
                                    .store(cur_phrase, std::sync::atomic::Ordering::Relaxed);
                            }
                        }
                    }
                    AudioCmd::MovePhrase { id, down } => {
                        let playing_id = phrases.get(cur_phrase).map(|p| p.phrase.id);
                        if let Some(pos) = phrases.iter().position(|p| p.phrase.id == id) {
                            let other = if down {
                                pos.checked_add(1).filter(|&next| next < phrases.len())
                            } else {
                                pos.checked_sub(1)
                            };
                            if let Some(other) = other {
                                phrases.swap(pos, other);
                                if let Some(pid) = playing_id {
                                    cur_phrase = phrases
                                        .iter()
                                        .position(|p| p.phrase.id == pid)
                                        .unwrap_or(cur_phrase);
                                    crate::CUR_PHRASE
                                        .store(cur_phrase, std::sync::atomic::Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    AudioCmd::Clear => {
                        phrases.clear();
                        voices.clear();
                        cur_phrase = 0;
                        pending_next_id = None;
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
                    &mut jump_counters,
                    &mut pending_next_id,
                );

                if let Some(ctrl) = pending_control {
                    match ctrl {
                        PendingControl::Bpm(v) => {
                            bpm = v;
                            for pp in phrases.iter_mut() {
                                pp.rebuild(sr, bpm);
                            }
                        }
                        PendingControl::Sustain(v) => {
                            sustain = v;
                        }
                        PendingControl::Vcf(change) => {
                            if let Ok(setting) = crate::command::apply_vcf_change(vcf, change) {
                                vcf.apply(setting);
                                vcf_filters.apply(setting);
                            }
                        }
                        PendingControl::Fx(change) => {
                            if let Ok(setting) = crate::command::apply_fx_change(fx, change) {
                                fx = setting;
                                fx_processor.set_settings(setting);
                            }
                        }
                        PendingControl::NamEnabled(enabled) => nam_enabled = enabled,
                        PendingControl::NamGain(gain) => nam_gain = gain.clamp(0.0, 8.0),
                        PendingControl::NamInput(route) => nam_input = route,
                        PendingControl::Sympathetics(enabled) => {
                            sympathetics_enabled = enabled;
                        }
                        PendingControl::SympatheticDecay(decay) => {
                            sympathetics.apply_change(
                                SympatheticChange {
                                    decay: Some(decay),
                                    ..SympatheticChange::default()
                                },
                                &mut sympathetics_enabled,
                            );
                        }
                        PendingControl::SympatheticGain(gain) => {
                            sympathetics.apply_change(
                                SympatheticChange {
                                    gain: Some(gain),
                                    ..SympatheticChange::default()
                                },
                                &mut sympathetics_enabled,
                            );
                        }
                        PendingControl::Sympathetic(change) => {
                            sympathetics.apply_change(change, &mut sympathetics_enabled);
                        }
                    }
                }

                let current_phrase = phrases.get(cur_phrase);
                let current_phrase_id = current_phrase.map(|phrase| phrase.phrase.id);
                if current_phrase_id != sympathetic_phrase_id {
                    if let Some(phrase) = current_phrase {
                        sympathetics.set_targets(&phrase.phrase.bar.frequencies);
                    }
                    sympathetic_phrase_id = current_phrase_id;
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
                        vcf.advance_tick();
                        vcf_filters.update_bank(vcf);
                        fx.advance_tick();
                        fx_processor.set_settings(fx);
                    }
                }

                voices.retain(|v| !v.done);
                let input = [
                    f32::from_bits(latest_input[0].load(std::sync::atomic::Ordering::Relaxed)),
                    f32::from_bits(latest_input[1].load(std::sync::atomic::Ordering::Relaxed)),
                ];
                meter_peaks[0] = meter_peaks[0].max(input[0].abs());
                meter_peaks[1] = meter_peaks[1].max(input[1].abs());
                if let Ok(captured_at) = input_time_rx.try_recv() {
                    let measured_ms = playback_at
                        .duration_since(&captured_at)
                        .map(|duration| duration.as_secs_f64() * 1000.0);
                    if let Some(measured_ms) = measured_ms {
                        let smoothed = latency_ms_ema
                            .map_or(measured_ms, |previous| previous * 0.9 + measured_ms * 0.1);
                        latency_ms_ema = Some(smoothed);
                    }
                    if let Some(result_tx) = latency_test.take() {
                        let result = measured_ms.ok_or_else(|| {
                            "audio device timestamps use incompatible clocks".to_string()
                        });
                        let _ = result_tx.send(result);
                    }
                }
                let mut live_input = match nam_input {
                    NamInput::Left => input[0],
                    NamInput::Right => input[1],
                    NamInput::Stereo => (input[0] + input[1]) * 0.5,
                };
                if !live_input.is_finite() {
                    live_input = 0.0;
                }
                if nam_enabled {
                    if let Some(model) = nam_model.as_mut() {
                        let nam_in = (live_input * nam_gain).clamp(-1.0, 1.0);
                        let nam_out = model.process_sample(nam_in);
                        if nam_warmup_samples > 0 {
                            nam_warmup_samples -= 1;
                            live_input = 0.0;
                        } else if nam_out.is_finite() && nam_out.abs() <= 8.0 {
                            nam_fault_samples = 0;
                            let blocked = nam_dc_blocker.process(nam_out);
                            live_input = crate::analog::soft_clip(blocked * 0.5);
                        } else {
                            live_input = 0.0;
                            nam_fault_samples += 1;
                            if nam_fault_samples >= 64 {
                                nam_enabled = false;
                                crate::NAM_MODEL_ACTIVE
                                    .store(false, std::sync::atomic::Ordering::Relaxed);
                                crate::set_nam_error(
                                    "NAM output became unsafe; run `nam gain 0.02`, lower your input level, use headphones, or `nam off`",
                                );
                            }
                        }
                    }
                }
                meter_peaks[2] = meter_peaks[2].max(live_input.abs());
                if last_metrics_publish.elapsed() >= std::time::Duration::from_secs(10) {
                    if let Some(latency_ms) = latency_ms_ema {
                        let latency_us = (latency_ms * 1000.0).round().max(1.0) as u64;
                        crate::AUDIO_LATENCY_LEFT_US
                            .store(latency_us, std::sync::atomic::Ordering::Relaxed);
                        crate::AUDIO_LATENCY_RIGHT_US
                            .store(latency_us, std::sync::atomic::Ordering::Relaxed);
                    }
                    crate::INPUT_LEFT_LEVEL.store(
                        (meter_peaks[0].min(4.0) * 1_000_000.0) as u32,
                        std::sync::atomic::Ordering::Relaxed,
                    );
                    crate::INPUT_RIGHT_LEVEL.store(
                        (meter_peaks[1].min(4.0) * 1_000_000.0) as u32,
                        std::sync::atomic::Ordering::Relaxed,
                    );
                    crate::NAM_OUTPUT_LEVEL.store(
                        (meter_peaks[2].min(4.0) * 1_000_000.0) as u32,
                        std::sync::atomic::Ordering::Relaxed,
                    );
                    meter_peaks = [0.0; 3];
                    last_metrics_publish = std::time::Instant::now();
                }
                if voices.is_empty()
                    && !fx.active()
                    && !sympathetics_enabled
                    && !sympathetics.has_energy()
                    && !vcf.all.enabled
                    && !vcf.mic.enabled
                    && live_input.abs() < 1.0e-7
                {
                    vcf_filters.reset();
                    for sample in frame.iter_mut() {
                        *sample = 0.0;
                    }
                    continue;
                }

                let (mut dry_left, mut dry_right) = (0f32, 0f32);
                let (mut mic_left, mut mic_right) = (0f32, 0f32);
                let (mut bass_left, mut bass_right) = (0f32, 0f32);
                let (mut kanun_left, mut kanun_right) = (0f32, 0f32);
                let (mut kick_left, mut kick_right) = (0f32, 0f32);
                let (mut tanbura_left, mut tanbura_right) = (0f32, 0f32);
                let mut sym_bass_input = 0.0f32;
                let mut sym_kanun_input = 0.0f32;
                let mut sym_drums_input = 0.0f32;
                for v in voices.iter_mut() {
                    let setting = if vcf.all.enabled {
                        None
                    } else {
                        vcf_target_for_kind(v.kind).and_then(|target| {
                            let setting = vcf.get(target);
                            setting.enabled.then_some(setting)
                        })
                    };
                    let s = v.sample_with_wave(
                        sr,
                        setting.and_then(|setting| {
                            (setting.target != VcfTarget::All)
                                .then_some(setting.wave)
                                .and_then(|wave| wave.oscillator())
                        }),
                    );
                    let angle = (v.pan + 1.0) * std::f32::consts::FRAC_PI_4;
                    let left = s * angle.cos();
                    let right = s * angle.sin();
                    let mono = (left + right) * 0.5;
                    match v.kind {
                        VoiceKind::SubBass => sym_bass_input += mono,
                        VoiceKind::MelodyFm => sym_kanun_input += mono,
                        VoiceKind::FloorTom | VoiceKind::Snare | VoiceKind::Crash => {
                            sym_drums_input += mono
                        }
                        VoiceKind::PhraseChange => {}
                    }
                    match setting.map(|setting| setting.target) {
                        Some(VcfTarget::All) => unreachable!("master VCF is applied after mix"),
                        Some(VcfTarget::Mic) => {
                            dry_left += left;
                            dry_right += right;
                        }
                        Some(VcfTarget::Bass) => {
                            bass_left += left;
                            bass_right += right;
                        }
                        Some(VcfTarget::Kanun) => {
                            kanun_left += left;
                            kanun_right += right;
                        }
                        Some(VcfTarget::Kick) => {
                            kick_left += left;
                            kick_right += right;
                        }
                        Some(VcfTarget::Tanbura) => {
                            dry_left += left;
                            dry_right += right;
                        }
                        None => {
                            dry_left += left;
                            dry_right += right;
                        }
                    }
                }
                let sympathetic = if sympathetics_enabled {
                    sympathetics.process(
                        true,
                        live_input,
                        sym_kanun_input,
                        sym_bass_input,
                        sym_drums_input,
                    )
                } else {
                    // Disabling sym closes the bridge to new energy; already
                    // ringing strings still decay into the output.
                    sympathetics.process(false, 0.0, 0.0, 0.0, 0.0)
                };
                if vcf.all.enabled {
                    dry_left += live_input;
                    dry_right += live_input;
                } else if vcf.mic.enabled {
                    mic_left += live_input;
                    mic_right += live_input;
                } else {
                    dry_left += live_input;
                    dry_right += live_input;
                }
                if vcf.all.enabled {
                    dry_left += sympathetic;
                    dry_right += sympathetic;
                } else if vcf.tanbura.enabled {
                    tanbura_left += sympathetic;
                    tanbura_right += sympathetic;
                } else {
                    dry_left += sympathetic;
                    dry_right += sympathetic;
                }
                let (mut left, mut right) = (dry_left, dry_right);
                if !vcf.all.enabled {
                    if vcf.mic.enabled {
                        let filtered = vcf_filters.mic.process(mic_left, mic_right);
                        left += filtered.0;
                        right += filtered.1;
                    }
                    if vcf.bass.enabled {
                        let filtered = vcf_filters.bass.process(bass_left, bass_right);
                        left += filtered.0;
                        right += filtered.1;
                    }
                    if vcf.kanun.enabled {
                        let filtered = vcf_filters.kanun.process(kanun_left, kanun_right);
                        left += filtered.0;
                        right += filtered.1;
                    }
                    if vcf.kick.enabled {
                        let filtered = vcf_filters.kick.process(kick_left, kick_right);
                        left += filtered.0;
                        right += filtered.1;
                    }
                    if vcf.tanbura.enabled {
                        let filtered = vcf_filters.tanbura.process(tanbura_left, tanbura_right);
                        left += filtered.0;
                        right += filtered.1;
                    }
                }
                if fx.active() {
                    let processed = fx_processor.process(left, right);
                    left = processed.0;
                    right = processed.1;
                }
                let saturated = crate::analog::soft_clip_stereo(left * vol, right * vol);
                if vcf.all.enabled {
                    let filtered = vcf_filters.all.process(saturated.0, saturated.1);
                    left = filtered.0.clamp(-1.0, 1.0);
                    right = filtered.1.clamp(-1.0, 1.0);
                } else {
                    left = saturated.0.clamp(-1.0, 1.0);
                    right = saturated.1.clamp(-1.0, 1.0);
                }

                if frame.len() == 1 {
                    frame[0] = (left + right) * 0.5;
                } else {
                    for (idx, sample) in frame.iter_mut().enumerate() {
                        *sample = if idx % 2 == 0 { left } else { right };
                    }
                }
            }

            voices.retain(|v| !v.done);
        },
        |err| eprintln!("audio error: {err}"),
        None,
    )?;

    stream.play()?;
    Ok(AudioStreams {
        _output: stream,
        _input: input_stream,
    })
}

fn tick_sequencer(
    phrases: &mut [PlayingPhrase],
    cur_phrase: &mut usize,
    jump_counters: &mut std::collections::HashMap<usize, usize>,
    pending_next_id: &mut Option<usize>,
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
            let limit = js.times.max(1);
            let value = jump_counters.entry(pid).or_insert(0);
            let incremented = value.saturating_add(1);
            if incremented < limit {
                *value = incremented;
                crate::CUR_JUMP_VALUE.store(*value, std::sync::atomic::Ordering::Relaxed);
                let target = phrases
                    .iter()
                    .position(|p| p.phrase.id == js.target_id)
                    .unwrap_or(0)
                    .min(phrases.len().saturating_sub(1));
                *cur_phrase = target;
                crate::CUR_PHRASE.store(*cur_phrase, std::sync::atomic::Ordering::Relaxed);
            } else {
                *value = 0;
                crate::CUR_JUMP_VALUE.store(0, std::sync::atomic::Ordering::Relaxed);
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
            if matches!(ctrl, ControlSpec::Stop) {
                *cur_phrase = (*cur_phrase + 1) % phrases.len();
                crate::CUR_PHRASE.store(*cur_phrase, std::sync::atomic::Ordering::Relaxed);
                continue;
            }
            let pending = match ctrl {
                ControlSpec::Stop => unreachable!(),
                ControlSpec::SetBpm(v) => PendingControl::Bpm(v),
                ControlSpec::SetSustain(v) => PendingControl::Sustain(v),
                ControlSpec::SetVcf(v) => PendingControl::Vcf(v),
                ControlSpec::SetFx(v) => PendingControl::Fx(v),
                ControlSpec::SetNamEnabled(v) => PendingControl::NamEnabled(v),
                ControlSpec::SetNamGain(v) => PendingControl::NamGain(v),
                ControlSpec::SetNamInput(v) => PendingControl::NamInput(v),
                ControlSpec::SetSympathetics(v) => PendingControl::Sympathetics(v),
                ControlSpec::SetSympatheticDecay(v) => PendingControl::SympatheticDecay(v),
                ControlSpec::SetSympatheticGain(v) => PendingControl::SympatheticGain(v),
                ControlSpec::SetSympathetic(v) => PendingControl::Sympathetic(v),
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
    let computed_next = if let Some(id) = *pending_next_id {
        phrases.iter().position(|phrase| phrase.phrase.id == id)
    } else {
        let curr_id = phrases[*cur_phrase].phrase.id;
        let n = phrases.len();
        let mut pos = (*cur_phrase + 1) % n;
        let mut result = None;
        for _ in 0..n {
            let p = &phrases[pos].phrase;
            if let Some(js) = &p.jump {
                let value = jump_counters.get(&p.id).copied().unwrap_or(0);
                if value.saturating_add(1) < js.times.max(1) {
                    let target = phrases
                        .iter()
                        .position(|pp| pp.phrase.id == js.target_id)
                        .unwrap_or(0);
                    // A jump may target a control or another jump. Continue
                    // simulating until prediction reaches a musical phrase.
                    pos = target;
                    continue;
                }
                pos = (pos + 1) % n;
            } else if p.control.is_some() {
                pos = (pos + 1) % n;
            } else {
                result = Some(pos);
                break;
            }
        }
        result.filter(|&next| phrases[next].phrase.id != curr_id)
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
    // A phrase repeat is an actual self-loop.  Until its final play, the next
    // phrase is the current phrase; only [n/n] exposes the following score
    // entry (including any jump it resolves through).
    let next_phrase = if is_last_play {
        computed_next.unwrap_or(*cur_phrase)
    } else {
        *cur_phrase
    };
    crate::NEXT_PHRASE.store(next_phrase, std::sync::atomic::Ordering::Relaxed);
    crate::EXIT_PHRASE.store(
        computed_next.unwrap_or(*cur_phrase),
        std::sync::atomic::Ordering::Relaxed,
    );
    let next_is_different = next_phrase != *cur_phrase;

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
            // Publish the completed-count reset before following control or
            // jump entries are evaluated. Every phrase is entered at [0/n].
            crate::CUR_PLAYS.store(0, std::sync::atomic::Ordering::Relaxed);
            let prev = *cur_phrase;
            *cur_phrase = pending_next_id
                .take()
                .and_then(|id| phrases.iter().position(|phrase| phrase.phrase.id == id))
                .unwrap_or_else(|| (*cur_phrase + 1) % phrases.len());
            crate::CUR_PHRASE.store(*cur_phrase, std::sync::atomic::Ordering::Relaxed);
            if *cur_phrase != prev && milestone == Milestone::None {
                milestone = Milestone::PhraseChange;
            }
        }
    }

    (ev, milestone, None)
}

fn vcf_target_for_kind(kind: VoiceKind) -> Option<VcfTarget> {
    match kind {
        VoiceKind::SubBass => Some(VcfTarget::Bass),
        VoiceKind::MelodyFm => Some(VcfTarget::Kanun),
        VoiceKind::FloorTom => Some(VcfTarget::Kick),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{SympatheticHarmony, SympatheticHarmonyComponent};

    #[test]
    fn sympathetic_harmony_weights_are_normalized() {
        let mut bank = SympatheticBank::new(48_000.0);
        bank.set_targets(&[220.0]);
        let mut harmony = SympatheticHarmony::default();
        harmony
            .push(SympatheticHarmonyComponent {
                ratio: 1.0,
                weight: 0.50,
            })
            .unwrap();
        harmony
            .push(SympatheticHarmonyComponent {
                ratio: 6.0 / 5.0,
                weight: 0.25,
            })
            .unwrap();
        harmony
            .push(SympatheticHarmonyComponent {
                ratio: 3.0 / 2.0,
                weight: 0.25,
            })
            .unwrap();

        let weighted = bank.weighted_targets(harmony);

        assert_eq!(weighted.len(), 3);
        assert!((weighted[0].0 - 220.0).abs() < f64::EPSILON);
        assert!((weighted[0].1 - 0.50).abs() < f32::EPSILON);
        assert!((weighted[1].0 - 264.0).abs() < f64::EPSILON);
        assert!((weighted[1].1 - 0.25).abs() < f32::EPSILON);
        assert!((weighted[2].0 - 330.0).abs() < f64::EPSILON);
        assert!((weighted[2].1 - 0.25).abs() < f32::EPSILON);
        assert!(
            (weighted.iter().map(|(_, weight)| weight).sum::<f32>() - 1.0).abs() < f32::EPSILON
        );
    }
}
