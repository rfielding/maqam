// app.rs — application state, phrases as top-level units

use crate::command::{self, Cmd, JinsSpec, LlmProvider, NamCommand, NamInput, ValueChange};
use crate::fx::FxSettings;
use crate::record;
use crate::sequencer::{build_control_entry, build_phrase, AudioCmd, BarSpec, ControlSpec, Phrase};
use crate::tuning::Pitch;
use crate::vcf::{VcfBank, VcfSettings, VcfTarget, VcoWave};
use base64::Engine;
use crossbeam_channel::Sender;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};

// TONE3000 publishable OAuth client ID for maqam-live. Publishable keys are
// intentionally safe to ship in desktop/client applications; never embed the secret key.
const TONE3000_PUBLISHABLE_CLIENT_ID: &str = "t3k_pub__1tkc-W6fWSyBUgGJdHj-bqpnPtFesDA";

#[derive(Clone, Debug)]
pub struct NamDownloadProgress {
    pub name: String,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub load_after: bool,
}

enum NamDownloadEvent {
    Progress {
        downloaded: u64,
        total: Option<u64>,
    },
    Done {
        name: String,
        load_after: bool,
        cached: bool,
    },
}

#[derive(Clone, Debug)]
struct Tone3000Auth {
    access_token: String,
    refresh_token: Option<String>,
    expires_at: u64,
    client_id: String,
}

pub struct App {
    pub phrases: Vec<Phrase>,
    pub input: String,
    pub message: Option<String>,
    pub show_help: bool,
    pub show_jins: bool,
    pub help_scroll: u16,
    pub jins_scroll: u16,
    pub message_scroll: u16,
    pub bpm: f64,
    pub sustain: f64,
    pub vcf: VcfBank,
    pub fx: FxSettings,
    pub vol: f32,
    pub tune_to: Pitch,
    pub live_nam_commands: Vec<String>,
    pub paused: bool,
    pub should_quit: bool,
    pub last_recording: Option<String>,
    pub nam_download_progress: Option<NamDownloadProgress>,
    pub history: Vec<String>,
    pub history_pos: Option<usize>,
    pub saved_input: String,
    pub cursor_pos: usize,
    pub rec_rx: Option<crossbeam_channel::Receiver<Result<String, String>>>,
    nam_download_rx: Option<crossbeam_channel::Receiver<Result<NamDownloadEvent, String>>>,
    tone3000_auth_rx: Option<crossbeam_channel::Receiver<Result<Tone3000Auth, String>>>,
    nam_latency_rx: Option<crossbeam_channel::Receiver<Result<f64, String>>>,
    pending_tone3000_download: Option<(u64, String)>,
    llm_rx: Option<crossbeam_channel::Receiver<Result<LlmOutcome, String>>>,
    llm_history: Vec<LlmChatMessage>,
    session_path: Option<String>,
    pending_nam_slot: Option<String>,
    next_phrase_id: usize,
    last_rhythm: Vec<u8>,
    auditioning_jins: bool,
    audio_tx: Sender<AudioCmd>,
    /// Sender to push BPM updates into the clockout thread (None if not started)
    clockout_tx: Option<crossbeam_channel::Sender<f64>>,
}

impl App {
    pub fn new(audio_tx: Sender<AudioCmd>) -> Self {
        crate::tuning::reset_tuning_base();
        let mut app = App {
            phrases: Vec::new(),
            input: String::new(),
            message: Some("? for help".into()),
            show_help: false,
            show_jins: false,
            help_scroll: 0,
            jins_scroll: 0,
            message_scroll: 0,
            bpm: 120.0,
            sustain: 1.25,
            vcf: VcfBank::default(),
            fx: FxSettings::default(),
            vol: 1.0,
            tune_to: Pitch::parse("d").unwrap(),
            live_nam_commands: Vec::new(),
            paused: false,
            should_quit: false,
            last_recording: None,
            nam_download_progress: None,
            history: Vec::new(),
            history_pos: None,
            saved_input: String::new(),
            cursor_pos: 0,
            rec_rx: None,
            nam_download_rx: None,
            tone3000_auth_rx: None,
            nam_latency_rx: None,
            pending_tone3000_download: None,
            llm_rx: None,
            llm_history: Vec::new(),
            session_path: None,
            pending_nam_slot: None,
            next_phrase_id: 0,
            last_rhythm: vec![3, 3, 2],
            auditioning_jins: false,
            audio_tx,
            clockout_tx: None,
        };
        if let Err(err) = app.load_globals() {
            app.message = Some(format!("✗ {err}"));
        }
        let _ = app.audio_tx.send(AudioCmd::SetVol(app.vol));
        app
    }

    // ── History ───────────────────────────────────────────────────────────

    pub fn history_push(&mut self, cmd: &str) {
        let s = cmd.trim().to_string();
        if !s.is_empty() && self.history.last().map(|x| x.as_str()) != Some(&s) {
            self.history.push(s);
        }
        self.history_pos = None;
        self.saved_input.clear();
    }

    pub fn last_history(&self) -> Option<&str> {
        self.history.last().map(|s| s.as_str())
    }

    pub fn session_filename(&self) -> Option<&str> {
        self.session_path.as_deref().and_then(|path| {
            Path::new(path)
                .file_name()
                .and_then(|filename| filename.to_str())
        })
    }

    pub fn globals_filename(&self) -> String {
        self.globals_path()
            .file_name()
            .and_then(|filename| filename.to_str())
            .map(str::to_string)
            .unwrap_or_else(|| ".globals.ml".to_string())
    }

    pub fn history_up(&mut self) {
        if self.history.is_empty() {
            return;
        }
        match self.history_pos {
            None => {
                self.saved_input = self.input.clone();
                self.history_pos = Some(self.history.len() - 1);
            }
            Some(0) => {}
            Some(i) => {
                self.history_pos = Some(i - 1);
            }
        }
        if let Some(i) = self.history_pos {
            self.input = self.history[i].clone();
            self.cursor_pos = self.input.chars().count();
        }
    }

    pub fn history_down(&mut self) {
        match self.history_pos {
            None => {}
            Some(i) if i + 1 >= self.history.len() => {
                self.history_pos = None;
                self.input = self.saved_input.clone();
                self.cursor_pos = self.input.chars().count();
            }
            Some(i) => {
                self.history_pos = Some(i + 1);
                self.input = self.history[i + 1].clone();
                self.cursor_pos = self.input.chars().count();
            }
        }
    }

    // ── Cursor / line editing ─────────────────────────────────────────────

    pub fn cursor_left(&mut self) {
        if self.cursor_pos > 0 {
            self.cursor_pos -= 1;
        }
    }

    pub fn cursor_right(&mut self) {
        let n = self.input.chars().count();
        if self.cursor_pos < n {
            self.cursor_pos += 1;
        }
    }

    pub fn cursor_home(&mut self) {
        self.cursor_pos = 0;
    }

    pub fn cursor_end(&mut self) {
        self.cursor_pos = self.input.chars().count();
    }

    pub fn insert_char(&mut self, ch: char) {
        let byte_pos: usize = self
            .input
            .char_indices()
            .nth(self.cursor_pos)
            .map(|(b, _)| b)
            .unwrap_or(self.input.len());
        self.input.insert(byte_pos, ch);
        self.cursor_pos += 1;
    }

    pub fn backspace(&mut self) {
        if self.cursor_pos == 0 {
            return;
        }
        let byte_pos: usize = self
            .input
            .char_indices()
            .nth(self.cursor_pos - 1)
            .map(|(b, _)| b)
            .unwrap_or(self.input.len().saturating_sub(1));
        self.input.remove(byte_pos);
        self.cursor_pos -= 1;
    }

    pub fn delete_char(&mut self) {
        let n = self.input.chars().count();
        if self.cursor_pos >= n {
            return;
        }
        let byte_pos: usize = self
            .input
            .char_indices()
            .nth(self.cursor_pos)
            .map(|(b, _)| b)
            .unwrap_or(self.input.len());
        self.input.remove(byte_pos);
    }

    pub fn complete_input(&mut self) {
        if self.complete_edit_input() {
            return;
        }
        if self.complete_metadata_command_input() {
            return;
        }
        if self.complete_phrase_input() {
            return;
        }
        let Some((cmd, arg_start, partial)) = completion_target(&self.input) else {
            return;
        };
        let matches = mq_matches(cmd, &partial);
        if matches.is_empty() {
            self.message = Some("✗ no .mq matches".into());
            return;
        }

        let common = completion_common_prefix(cmd, &partial, &matches);
        let replacement = if matches.len() == 1 {
            matches[0].clone()
        } else if common.len() > partial.len() {
            self.message = Some(format!("{}: {}", cmd, matches.join("  ")));
            common
        } else {
            self.message = Some(format!("{}: {}", cmd, matches.join("  ")));
            return;
        };

        self.input
            .replace_range(arg_start..self.input.len(), &replacement);
        self.cursor_pos = self.input.chars().count();
        if matches.len() == 1 {
            self.message = None;
        }
    }

    fn complete_edit_input(&mut self) -> bool {
        let trimmed = self.input.trim();
        let mut tokens = trimmed.split_whitespace();
        if tokens.next() != Some("edit") {
            return false;
        }
        let Some(id_token) = tokens.next() else {
            return false;
        };
        if tokens.next().is_some() {
            return false;
        }
        let Ok(id_ref) = id_token.parse::<isize>() else {
            return false;
        };
        let Some(id) = self.resolve_id_ref(id_ref) else {
            self.message = Some(format!("✗ no phrase id {id_ref}"));
            return true;
        };
        let Some(phrase) = self.phrases.iter().find(|phrase| phrase.id == id) else {
            self.message = Some(format!("✗ no phrase id {id}"));
            return true;
        };
        self.input = format!("edit {id_token} {}", phrase.display_src());
        self.cursor_pos = self.input.chars().count();
        self.message = None;
        true
    }

    fn complete_metadata_command_input(&mut self) -> bool {
        let Some((body_start, body)) = command_body_for_completion(&self.input) else {
            return false;
        };
        let Some(completion) = metadata_command_completion(body) else {
            return false;
        };
        if let Some(replacement) = completion.replacement {
            self.input
                .replace_range(body_start..self.input.len(), &replacement);
            self.cursor_pos = self.input.chars().count();
        }
        self.message = completion.message;
        true
    }

    fn complete_phrase_input(&mut self) -> bool {
        let Some(replacement) = phrase_completion(&self.input, &self.phrases) else {
            return false;
        };
        self.input = replacement;
        self.cursor_pos = self.input.chars().count();
        self.message = None;
        true
    }

    pub fn overlay_scroll_up(&mut self) {
        if self.show_help {
            self.help_scroll = self.help_scroll.saturating_sub(1);
        } else if self.show_jins {
            self.jins_scroll = self.jins_scroll.saturating_sub(1);
        }
    }

    pub fn overlay_scroll_down(&mut self) {
        if self.show_help {
            self.help_scroll = self.help_scroll.saturating_add(1);
        } else if self.show_jins {
            self.jins_scroll = self.jins_scroll.saturating_add(1);
        }
    }

    pub fn overlay_scroll_home(&mut self) {
        if self.show_help {
            self.help_scroll = 0;
        }
        if self.show_jins {
            self.jins_scroll = 0;
        }
    }

    pub fn message_scroll_up(&mut self) {
        self.message_scroll = self.message_scroll.saturating_sub(1);
    }

    pub fn message_scroll_down(&mut self) {
        self.message_scroll = self.message_scroll.saturating_add(1);
    }

    pub fn message_scroll_home(&mut self) {
        self.message_scroll = 0;
    }

    // ── Render thread poll ────────────────────────────────────────────────

    pub fn tick(&mut self) {
        if let Some(rx) = &self.llm_rx {
            if let Ok(result) = rx.try_recv() {
                match result {
                    Ok(LlmOutcome::Answer {
                        sent_prompt,
                        answer,
                    }) => {
                        self.remember_llm_exchange(sent_prompt, answer.clone());
                        self.message = Some(answer);
                    }
                    Ok(LlmOutcome::Edit {
                        sent_prompt,
                        commands,
                    }) => {
                        let returned_commands = commands.join("\n");
                        self.apply_llm_edit_commands(commands);
                        let application_result = self
                            .message
                            .clone()
                            .unwrap_or_else(|| "LLM edit produced no visible result".into());
                        self.remember_llm_exchange(
                            sent_prompt,
                            format!(
                                "Tool returned commands:\n{returned_commands}\n\nApplication result:\n{application_result}"
                            ),
                        );
                    }
                    Err(e) => {
                        self.message = Some(format!("✗ {e}"));
                    }
                }
                self.llm_rx = None;
            }
        }
        if let Some(rx) = &self.rec_rx {
            if let Ok(result) = rx.try_recv() {
                match result {
                    Ok(path) => {
                        self.last_recording = Some(path.clone());
                        self.message = Some(format!("saved → {path}"));
                    }
                    Err(e) => {
                        self.message = Some(format!("✗ {e}"));
                    }
                }
                self.rec_rx = None;
            }
        }
        self.poll_nam_download();
        self.poll_tone3000_auth();
        if let Some(result) = self
            .nam_latency_rx
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        {
            self.nam_latency_rx = None;
            self.message = Some(match result {
                Ok(ms) => format!("NAM input round-trip latency: {ms:.1} ms"),
                Err(err) => format!("✗ latency test: {err}"),
            });
        }
    }

    fn poll_tone3000_auth(&mut self) {
        let result = self
            .tone3000_auth_rx
            .as_ref()
            .and_then(|rx| rx.try_recv().ok());
        let Some(result) = result else { return };
        self.tone3000_auth_rx = None;
        match result {
            Ok(auth) => {
                if let Err(err) = save_tone3000_auth(&auth) {
                    self.message = Some(format!(
                        "✗ TONE3000 login succeeded but credentials could not be saved: {err}"
                    ));
                    return;
                }
                self.message = Some("TONE3000 login complete".into());
                if let Some((tone_id, name)) = self.pending_tone3000_download.take() {
                    self.start_tone3000_download(tone_id, name);
                }
            }
            Err(err) => {
                crate::set_nam_error(format!(
                    "TONE3000 login failed: {err}; run `nam login` and complete the browser login"
                ));
                self.message = Some(format!("✗ TONE3000 login failed: {err}"));
            }
        }
    }

    fn poll_nam_download(&mut self) {
        let mut finished = false;
        if let Some(rx) = &self.nam_download_rx {
            while let Ok(result) = rx.try_recv() {
                match result {
                    Ok(NamDownloadEvent::Progress { downloaded, total }) => {
                        if let Some(progress) = &mut self.nam_download_progress {
                            progress.downloaded = downloaded;
                            progress.total = total;
                        }
                    }
                    Ok(NamDownloadEvent::Done {
                        name,
                        load_after,
                        cached,
                    }) => {
                        finished = true;
                        self.nam_download_progress = None;
                        if load_after {
                            match nam_load_audio_cmd(&name) {
                                Ok((audio_cmd, message)) => {
                                    let _ = self.audio_tx.send(audio_cmd);
                                    mark_nam_loaded();
                                    replace_live_nam_command(
                                        &mut self.live_nam_commands,
                                        Some(format!("nam {name}")),
                                    );
                                    self.message = Some(if cached {
                                        format!("NAM capture already cached; {message}")
                                    } else {
                                        message
                                    });
                                }
                                Err(err) => {
                                    crate::set_nam_error(err.clone());
                                    self.message = Some(format!("✗ {err}"));
                                }
                            }
                        } else {
                            crate::clear_nam_error();
                            crate::NAM_STATUS.store(0, std::sync::atomic::Ordering::Relaxed);
                            self.message = Some(if cached {
                                format!("NAM capture already cached: {name}")
                            } else {
                                format!("NAM capture downloaded: {name}")
                            });
                        }
                    }
                    Err(err) => {
                        finished = true;
                        self.nam_download_progress = None;
                        crate::set_nam_error(err.clone());
                        self.message = Some(format!("✗ {err}"));
                    }
                }
            }
        }
        if finished {
            self.nam_download_rx = None;
        }
    }

    fn resync_audio_sequence(&mut self, focus_id: Option<usize>) {
        let target_pos = focus_id
            .and_then(|id| self.phrases.iter().position(|p| p.id == id))
            .unwrap_or(0);
        let (start_bpm, start_sustain, start_vcf, start_fx) = self.sequence_start_settings();
        let _ = self.audio_tx.send(AudioCmd::Clear);
        let _ = self.audio_tx.send(AudioCmd::SetBpm(start_bpm));
        let _ = self.audio_tx.send(AudioCmd::SetSustain(start_sustain));
        let _ = self.audio_tx.send(AudioCmd::SetVcfBank(start_vcf));
        let _ = self.audio_tx.send(AudioCmd::SetFxSettings(start_fx));
        let _ = self.audio_tx.send(AudioCmd::SetVol(self.vol));
        let _ = self.audio_tx.send(AudioCmd::SetPaused(self.paused));
        for p in self.phrases.iter().cloned() {
            let _ = self.audio_tx.send(AudioCmd::AddPhrase(p));
        }
        if !self.phrases.is_empty() {
            let _ = self.audio_tx.send(AudioCmd::SetCurPhrase(
                target_pos.min(self.phrases.len() - 1),
            ));
        }
        self.auditioning_jins = false;
    }

    fn resolve_id_ref(&self, id_ref: isize) -> Option<usize> {
        resolve_id_ref_in_phrases(&self.phrases, id_ref)
    }

    fn insert_sym_control(&mut self, before: isize, src: String, control: ControlSpec) {
        let insert_pos = match self.resolve_id_ref(before) {
            Some(before_id) => self
                .phrases
                .iter()
                .position(|phrase| phrase.id == before_id)
                .unwrap_or(self.phrases.len()),
            None => {
                self.message = Some(format!("✗ no phrase id {before}"));
                return;
            }
        };
        let id = self.next_phrase_id;
        self.next_phrase_id += 1;
        let entry = build_control_entry(id, src, control);
        self.phrases.insert(insert_pos, entry.clone());
        let _ = self.audio_tx.send(AudioCmd::InsertPhrase {
            pos: insert_pos,
            phrase: entry,
        });
        self.message = Some(format!("inserted sym at {insert_pos}"));
    }

    fn replace_sym_control(&mut self, id_ref: isize, src: String, control: ControlSpec) {
        let Some(id) = self.resolve_id_ref(id_ref) else {
            self.message = Some(format!("✗ no phrase id {id_ref}"));
            return;
        };
        let Some(pos) = self.phrases.iter().position(|phrase| phrase.id == id) else {
            self.message = Some(format!("✗ no phrase id {id}"));
            return;
        };
        let entry = build_control_entry(id, src, control);
        self.phrases[pos] = entry.clone();
        let _ = self.audio_tx.send(AudioCmd::ReplacePhrase(entry));
        self.message = Some(format!("edited {id} → sym"));
    }

    fn insert_nam_control(&mut self, before: isize, command: NamCommand) {
        let Some(control) = nam_timeline_control(&command) else {
            self.message = Some("✗ this NAM utility command cannot be scheduled".into());
            return;
        };
        let Some(before_id) = self.resolve_id_ref(before) else {
            self.message = Some(format!("✗ no phrase id {before}"));
            return;
        };
        let pos = self
            .phrases
            .iter()
            .position(|p| p.id == before_id)
            .unwrap_or(self.phrases.len());
        let id = self.next_phrase_id;
        self.next_phrase_id += 1;
        let src = nam_command_src(&command).unwrap();
        let entry = build_control_entry(id, src, control);
        self.phrases.insert(pos, entry.clone());
        let _ = self
            .audio_tx
            .send(AudioCmd::InsertPhrase { pos, phrase: entry });
        self.apply_nam_command(command);
    }

    fn replace_nam_control(&mut self, id_ref: isize, command: NamCommand) {
        let Some(control) = nam_timeline_control(&command) else {
            self.message = Some("✗ this NAM utility command cannot be scheduled".into());
            return;
        };
        let Some(id) = self.resolve_id_ref(id_ref) else {
            self.message = Some(format!("✗ no phrase id {id_ref}"));
            return;
        };
        let Some(pos) = self.phrases.iter().position(|p| p.id == id) else {
            self.message = Some(format!("✗ no phrase id {id}"));
            return;
        };
        let src = nam_command_src(&command).unwrap();
        let entry = build_control_entry(id, src, control);
        self.phrases[pos] = entry.clone();
        let _ = self.audio_tx.send(AudioCmd::ReplacePhrase(entry));
        self.apply_nam_command(command);
    }

    fn sequence_start_settings(&self) -> (f64, f64, VcfBank, FxSettings) {
        let mut bpm = 120.0f64;
        let mut sustain = 1.25f64;
        let mut vcf = VcfBank::default();
        let mut fx = FxSettings::default();
        for phrase in &self.phrases {
            if let Some(ctrl) = phrase.control {
                match ctrl {
                    ControlSpec::Stop => {}
                    ControlSpec::SetBpm(v) => bpm = v,
                    ControlSpec::SetSustain(v) => sustain = v,
                    ControlSpec::SetVcf(v) => {
                        if let Ok(setting) = command::apply_vcf_change(vcf, v) {
                            vcf.apply(setting);
                        }
                    }
                    ControlSpec::SetFx(v) => {
                        if let Ok(setting) = command::apply_fx_change(fx, v) {
                            fx = setting;
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
                continue;
            }
            if phrase.jump.is_none() {
                break;
            }
        }
        (bpm, sustain, vcf, fx)
    }

    fn audition_jins(&mut self, specs: Vec<JinsSpec>) -> Result<(), String> {
        let resolved = resolve_rhythms(specs, &[1])?;
        let src = resolved
            .iter()
            .map(|s| s.src.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let mut phrase = build_phrase(usize::MAX, format!("[preview] {src}"), resolved, 4, 1);
        let n_freqs = phrase.bar.frequencies.len().max(1);
        let mut walk = Vec::with_capacity(n_freqs * 4);
        for degree in 0..n_freqs {
            walk.push(degree);
            walk.push(degree);
        }
        if n_freqs > 1 {
            for degree in (0..(n_freqs - 1)).rev() {
                walk.push(degree);
                walk.push(degree);
            }
        }
        phrase.bar.groups = vec![1; walk.len()];
        phrase.bar.group_degrees = walk;
        phrase.bar.group_degrees.push(0);
        phrase.bar.recompute_events();
        phrase.bar.total_subdivs = phrase.bar.events.len();

        self.paused = false;
        let _ = self.audio_tx.send(AudioCmd::Clear);
        let _ = self.audio_tx.send(AudioCmd::SetBpm(self.bpm));
        let _ = self.audio_tx.send(AudioCmd::SetSustain(self.sustain));
        let _ = self.audio_tx.send(AudioCmd::SetVcfBank(self.vcf));
        let _ = self.audio_tx.send(AudioCmd::SetFxSettings(self.fx));
        let _ = self.audio_tx.send(AudioCmd::SetVol(self.vol));
        let _ = self.audio_tx.send(AudioCmd::SetPaused(false));
        let _ = self.audio_tx.send(AudioCmd::AddPhrase(phrase));
        let _ = self.audio_tx.send(AudioCmd::SetCurPhrase(0));
        self.auditioning_jins = true;
        Ok(())
    }

    // ── Commands ──────────────────────────────────────────────────────────

    pub fn handle_command(&mut self, raw: &str) {
        for part in raw.split(';') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }

            // clockin <device> — receive MIDI clock, sync BPM to external gear
            if let Some(dev) = part.strip_prefix("clockin ") {
                let dev = dev.trim().to_string();
                crate::midi_clock::start_clock_receiver(dev.clone(), self.audio_tx.clone());
                self.message = Some(format!("clock ← {dev}"));
                continue;
            }

            // clockout <device> — send MIDI clock, slave external gear to maqam-live BPM
            if let Some(dev) = part.strip_prefix("clockout ") {
                let dev = dev.trim().to_string();
                let tx = crate::midi_clockout::start_clock_sender(dev.clone(), self.bpm);
                self.clockout_tx = Some(tx);
                self.message = Some(format!("clock → {dev}"));
                continue;
            }

            match command::parse(part) {
                Ok(cmd) => self.execute(cmd),
                Err(msg) => {
                    self.message = Some(format!("✗ {msg}"));
                    return;
                }
            }
        }
    }

    fn execute(&mut self, cmd: Cmd) {
        let keep_audition = matches!(
            &cmd,
            Cmd::CreateJins { .. } | Cmd::AuditionJins { .. } | Cmd::Help | Cmd::ListJins
        );
        if self.auditioning_jins && !keep_audition {
            self.resync_audio_sequence(None);
        }
        match cmd {
            Cmd::Quit => self.should_quit = true,
            Cmd::Help => {
                self.show_help = true;
            }
            Cmd::AskLlm { provider, prompt } => {
                if llm_prompt_is_edit_request(&prompt) {
                    self.ask_llm_for_edit(provider, prompt);
                } else {
                    self.ask_llm(provider, prompt);
                }
            }
            Cmd::Jump { to, times } => {
                if times <= 1 {
                    self.message =
                        Some("✗ jump ×1 is a no-op; use j <id> 2 or omit the jump".into());
                    return;
                }
                let Some(to) = self.resolve_id_ref(to) else {
                    self.message = Some(format!("✗ no phrase id {to}"));
                    return;
                };
                if !self.phrases.iter().any(|p| p.id == to) {
                    self.message = Some(format!("✗ no phrase id {to}"));
                    return;
                }
                let id = self.next_phrase_id;
                self.next_phrase_id += 1;
                let entry = crate::sequencer::build_jump_entry(id, to, times);
                let _ = self.audio_tx.send(AudioCmd::AddPhrase(entry.clone()));
                self.phrases.push(entry);
                self.message = None;
            }

            Cmd::Insert {
                before,
                source,
                specs,
                repeat,
            } => {
                if specs.is_empty() {
                    self.message = Some("✗ empty phrase".into());
                    return;
                }
                let resolved = match resolve_rhythms(specs, &self.last_rhythm) {
                    Ok(r) => r,
                    Err(e) => {
                        self.message = Some(format!("✗ {e}"));
                        return;
                    }
                };
                if let Some(r) = resolved.last() {
                    self.last_rhythm = r.groups.clone();
                }
                let peak = 4usize;
                let id = self.next_phrase_id;
                self.next_phrase_id += 1;
                let phrase = build_phrase(id, source, resolved, peak, repeat.max(1));
                let pos = match self.resolve_id_ref(before) {
                    Some(before_id) => self
                        .phrases
                        .iter()
                        .position(|p| p.id == before_id)
                        .unwrap_or(self.phrases.len()),
                    None => {
                        self.message = Some(format!("✗ no phrase id {before}"));
                        return;
                    }
                };
                self.phrases.insert(pos, phrase.clone());
                let _ = self.audio_tx.send(AudioCmd::InsertPhrase { pos, phrase });
                self.message = Some(format!("inserted at {pos}"));
            }

            Cmd::InsertJump { before, to, times } => {
                if times <= 1 {
                    self.message = Some(
                        "✗ jump ×1 is a no-op; insert a phrase/control line or use j <id> 2".into(),
                    );
                    return;
                }
                let Some(to) = self.resolve_id_ref(to) else {
                    self.message = Some(format!("✗ no phrase id {to}"));
                    return;
                };
                if !self.phrases.iter().any(|p| p.id == to) {
                    self.message = Some(format!("✗ no phrase id {to}"));
                    return;
                }
                let insert_pos = match self.resolve_id_ref(before) {
                    Some(before_id) => self
                        .phrases
                        .iter()
                        .position(|p| p.id == before_id)
                        .unwrap_or(self.phrases.len()),
                    None => {
                        self.message = Some(format!("✗ no phrase id {before}"));
                        return;
                    }
                };
                let id = self.next_phrase_id;
                self.next_phrase_id += 1;
                let entry = crate::sequencer::build_jump_entry(id, to, times);
                self.phrases.insert(insert_pos, entry.clone());
                let _ = self.audio_tx.send(AudioCmd::InsertPhrase {
                    pos: insert_pos,
                    phrase: entry,
                });
                self.message = None;
            }

            Cmd::InsertBpm { before, change } => {
                let bpm = match apply_bpm_change(self.bpm, change) {
                    Ok(v) => v,
                    Err(e) => {
                        self.message = Some(format!("✗ {e}"));
                        return;
                    }
                };
                let insert_pos = match self.resolve_id_ref(before) {
                    Some(before_id) => self
                        .phrases
                        .iter()
                        .position(|p| p.id == before_id)
                        .unwrap_or(self.phrases.len()),
                    None => {
                        self.message = Some(format!("✗ no phrase id {before}"));
                        return;
                    }
                };
                let id = self.next_phrase_id;
                self.next_phrase_id += 1;
                let entry = build_control_entry(id, format!("bpm {bpm}"), ControlSpec::SetBpm(bpm));
                self.phrases.insert(insert_pos, entry.clone());
                let _ = self.audio_tx.send(AudioCmd::InsertPhrase {
                    pos: insert_pos,
                    phrase: entry,
                });
                self.message = Some(format!("inserted bpm at {insert_pos}"));
            }

            Cmd::InsertSustain { before, change } => {
                let secs = match apply_sustain_change(self.sustain, change) {
                    Ok(v) => v,
                    Err(e) => {
                        self.message = Some(format!("✗ {e}"));
                        return;
                    }
                };
                let insert_pos = match self.resolve_id_ref(before) {
                    Some(before_id) => self
                        .phrases
                        .iter()
                        .position(|p| p.id == before_id)
                        .unwrap_or(self.phrases.len()),
                    None => {
                        self.message = Some(format!("✗ no phrase id {before}"));
                        return;
                    }
                };
                let id = self.next_phrase_id;
                self.next_phrase_id += 1;
                let entry =
                    build_control_entry(id, format!("s {secs}"), ControlSpec::SetSustain(secs));
                self.phrases.insert(insert_pos, entry.clone());
                let _ = self.audio_tx.send(AudioCmd::InsertPhrase {
                    pos: insert_pos,
                    phrase: entry,
                });
                self.message = Some(format!("inserted sustain at {insert_pos}"));
            }

            Cmd::InsertVcf { before, change } => {
                let _vcf = match command::apply_vcf_change(self.vcf, change) {
                    Ok(v) => v,
                    Err(e) => {
                        self.message = Some(format!("✗ {e}"));
                        return;
                    }
                };
                let insert_pos = match self.resolve_id_ref(before) {
                    Some(before_id) => self
                        .phrases
                        .iter()
                        .position(|p| p.id == before_id)
                        .unwrap_or(self.phrases.len()),
                    None => {
                        self.message = Some(format!("✗ no phrase id {before}"));
                        return;
                    }
                };
                let id = self.next_phrase_id;
                self.next_phrase_id += 1;
                let entry =
                    build_control_entry(id, vcf_change_src(change), ControlSpec::SetVcf(change));
                self.phrases.insert(insert_pos, entry.clone());
                let _ = self.audio_tx.send(AudioCmd::InsertPhrase {
                    pos: insert_pos,
                    phrase: entry,
                });
                self.message = Some(format!("inserted vcf at {insert_pos}"));
            }

            Cmd::InsertFx { before, change } => {
                let fx = match command::apply_fx_change(self.fx, change) {
                    Ok(v) => v,
                    Err(e) => {
                        self.message = Some(format!("✗ {e}"));
                        return;
                    }
                };
                let insert_pos = match self.resolve_id_ref(before) {
                    Some(before_id) => self
                        .phrases
                        .iter()
                        .position(|p| p.id == before_id)
                        .unwrap_or(self.phrases.len()),
                    None => {
                        self.message = Some(format!("✗ no phrase id {before}"));
                        return;
                    }
                };
                let id = self.next_phrase_id;
                self.next_phrase_id += 1;
                let entry =
                    build_control_entry(id, fx_change_src(change), ControlSpec::SetFx(change));
                self.phrases.insert(insert_pos, entry.clone());
                let _ = self.audio_tx.send(AudioCmd::InsertPhrase {
                    pos: insert_pos,
                    phrase: entry,
                });
                self.message = Some(format!("inserted {}", describe_fx(fx)));
            }

            Cmd::InsertNam { before, command } => self.insert_nam_control(before, command),

            Cmd::InsertSympathetics { before, enabled } => {
                self.insert_sym_control(
                    before,
                    if enabled { "sym on" } else { "sym off" }.into(),
                    ControlSpec::SetSympathetics(enabled),
                );
            }

            Cmd::InsertSympatheticDecay { before, decay } => {
                self.insert_sym_control(
                    before,
                    format!("sym decay {decay}"),
                    ControlSpec::SetSympatheticDecay(decay),
                );
            }

            Cmd::InsertSympatheticGain { before, gain } => {
                self.insert_sym_control(
                    before,
                    format!("sym drive {gain}"),
                    ControlSpec::SetSympatheticGain(gain),
                );
            }

            Cmd::InsertSympathetic { before, change } => {
                self.insert_sym_control(
                    before,
                    sym_change_src(change),
                    ControlSpec::SetSympathetic(change),
                );
            }

            Cmd::TogglePause { start_id } => {
                if let Some(id) = start_id {
                    let Some(id) = self.resolve_id_ref(id) else {
                        self.message = Some(format!("✗ no phrase id {id}"));
                        return;
                    };
                    // z <id>: queue the address for the next phrase exit.
                    match self.phrases.iter().position(|p| p.id == id) {
                        Some(_) => {
                            let _ = self.audio_tx.send(AudioCmd::QueueNextPhrase(id));
                            self.message = Some(format!("next → phrase {id}"));
                        }
                        None => {
                            self.message = Some(format!("✗ no phrase id {id}"));
                        }
                    }
                } else {
                    // z alone: toggle pause; restart from 0 when unpausing
                    self.paused = !self.paused;
                    if !self.paused {
                        let _ = self.audio_tx.send(AudioCmd::SetCurPhrase(0));
                    }
                    let _ = self.audio_tx.send(AudioCmd::SetPaused(self.paused));
                    self.message = Some(if self.paused {
                        "⏸ paused".into()
                    } else {
                        "▶ playing".into()
                    });
                }
            }

            Cmd::SetVol(v) => {
                self.vol = v;
                let _ = self.audio_tx.send(AudioCmd::SetVol(v));
                self.message = Some(match self.save_globals() {
                    Ok(()) => format!("vol → {v:.2}"),
                    Err(err) => format!("✗ {err}"),
                });
            }

            Cmd::Record(reps) => {
                if crate::REC_ACTIVE.load(std::sync::atomic::Ordering::Relaxed) {
                    self.message = Some("✗ already rendering".into());
                    return;
                }
                let phrases = self.phrases.clone();
                let (bpm, sustain, vcf, fx) = self.sequence_start_settings();
                let (tx, rx) = crossbeam_channel::bounded(1);
                self.rec_rx = Some(rx);
                self.message = Some(format!("◉ rendering {}×…", reps));
                std::thread::spawn(move || {
                    let result = record::record_cycle(phrases, bpm, sustain, vcf, fx, reps)
                        .map_err(|e| e.to_string());
                    let _ = tx.send(result);
                });
            }

            Cmd::Rotate => {
                if self.phrases.len() < 2 {
                    self.message = Some("nothing to rotate".into());
                } else {
                    let first = self.phrases.remove(0);
                    self.phrases.push(first);
                    let _ = self.audio_tx.send(AudioCmd::Rotate);
                    self.message = None;
                }
            }

            Cmd::MoveUp(id) => {
                let Some(id) = self.resolve_id_ref(id) else {
                    self.message = Some(format!("✗ no phrase id {id}"));
                    return;
                };
                let Some(pos) = self.phrases.iter().position(|p| p.id == id) else {
                    self.message = Some(format!("✗ no phrase id {id}"));
                    return;
                };
                if pos == 0 {
                    self.message = Some(format!("id {id} is already at top"));
                    return;
                }
                self.phrases.swap(pos - 1, pos);
                let _ = self.audio_tx.send(AudioCmd::MovePhrase { id, down: false });
                self.message = Some(format!("moved {id} up"));
            }

            Cmd::MoveDown(id) => {
                let Some(id) = self.resolve_id_ref(id) else {
                    self.message = Some(format!("✗ no phrase id {id}"));
                    return;
                };
                let Some(pos) = self.phrases.iter().position(|p| p.id == id) else {
                    self.message = Some(format!("✗ no phrase id {id}"));
                    return;
                };
                if pos + 1 >= self.phrases.len() {
                    self.message = Some(format!("id {id} is already at bottom"));
                    return;
                }
                self.phrases.swap(pos, pos + 1);
                let _ = self.audio_tx.send(AudioCmd::MovePhrase { id, down: true });
                self.message = Some(format!("moved {id} down"));
            }

            Cmd::ListJins => {
                self.show_jins = true;
            }

            Cmd::AuditionJins { specs } => {
                let label = specs
                    .iter()
                    .map(|s| s.src.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                match self.audition_jins(specs) {
                    Ok(()) => self.message = Some(format!("auditioning {label}")),
                    Err(e) => self.message = Some(format!("✗ {e}")),
                }
            }

            Cmd::CreateJins { name, ratios } => match crate::tuning::Maqam::create(&name, ratios) {
                Ok(()) => self.message = Some(format!("created jins {name}")),
                Err(e) => self.message = Some(format!("✗ {e}")),
            },

            Cmd::DeleteJins { name } => {
                if crate::tuning::Maqam::delete(&name) {
                    self.message = Some(format!("deleted jins {name}"));
                } else {
                    self.message = Some(format!("✗ no jins '{name}'"));
                }
            }

            Cmd::Save { path } => {
                let path = match path.or_else(|| self.session_path.clone()) {
                    Some(path) => path,
                    None => {
                        self.message = Some("✗ usage: save <path>".into());
                        return;
                    }
                };
                match self.save_session(&path) {
                    Ok(()) => {
                        self.session_path = Some(path.clone());
                        self.message = Some(format!("saved session → {path}"));
                    }
                    Err(e) => self.message = Some(format!("✗ save failed: {e}")),
                }
            }

            Cmd::Load { path } => match self.load_session(&path) {
                Ok(()) => {
                    self.session_path = Some(path.clone());
                    let load_detail = self.message.take();
                    self.message = Some(match load_detail {
                        Some(detail)
                            if self.pending_nam_slot.is_some()
                                || self.nam_download_progress.is_some()
                                || detail.starts_with("TONE3000 authorization")
                                || detail.starts_with("TONE3000 login")
                                || detail.contains("TONE3000_CLIENT_ID") =>
                        {
                            detail
                        }
                        Some(detail) if detail.starts_with("loaded session; ") => {
                            format!(
                                "loaded session ← {path}; {}",
                                detail.trim_start_matches("loaded session; ")
                            )
                        }
                        _ => format!("loaded session ← {path}"),
                    });
                }
                Err(e) => self.message = Some(format!("✗ load failed: {e}")),
            },

            Cmd::Clear => {
                self.phrases.clear();
                self.next_phrase_id = 0;
                let _ = self.audio_tx.send(AudioCmd::Clear);
                self.message = Some("cleared".into());
            }
            Cmd::Stop => {
                let id = self.next_phrase_id;
                self.next_phrase_id += 1;
                let entry = build_control_entry(id, "stop".into(), ControlSpec::Stop);
                self.phrases.push(entry.clone());
                let _ = self.audio_tx.send(AudioCmd::AddPhrase(entry));
                self.message = Some("stop line added".into());
            }
            Cmd::Sympathetics(enabled) => {
                let id = self.next_phrase_id;
                self.next_phrase_id += 1;
                let src = if enabled { "sym on" } else { "sym off" };
                let entry =
                    build_control_entry(id, src.into(), ControlSpec::SetSympathetics(enabled));
                self.phrases.push(entry.clone());
                let _ = self.audio_tx.send(AudioCmd::AddPhrase(entry));
                let _ = self.audio_tx.send(AudioCmd::SetSympathetics(enabled));
                self.message = Some(if enabled {
                    "sympathetic strings on".into()
                } else {
                    "sympathetic strings off".into()
                });
            }
            Cmd::SympatheticDecay(decay) => {
                let id = self.next_phrase_id;
                self.next_phrase_id += 1;
                let entry = build_control_entry(
                    id,
                    format!("sym decay {decay}"),
                    ControlSpec::SetSympatheticDecay(decay),
                );
                self.phrases.push(entry.clone());
                let _ = self.audio_tx.send(AudioCmd::AddPhrase(entry));
                let _ = self.audio_tx.send(AudioCmd::SetSympatheticDecay(decay));
                self.message = Some(format!("sym decay {decay:.5}"));
            }
            Cmd::SympatheticGain(gain) => {
                let id = self.next_phrase_id;
                self.next_phrase_id += 1;
                let entry = build_control_entry(
                    id,
                    format!("sym gain {gain}"),
                    ControlSpec::SetSympatheticGain(gain),
                );
                self.phrases.push(entry.clone());
                let _ = self.audio_tx.send(AudioCmd::AddPhrase(entry));
                let _ = self.audio_tx.send(AudioCmd::SetSympatheticGain(gain));
                self.message = Some(format!("sym gain {gain:.2}"));
            }
            Cmd::Sympathetic(change) => {
                let id = self.next_phrase_id;
                self.next_phrase_id += 1;
                let src = sym_change_src(change);
                let entry =
                    build_control_entry(id, src.clone(), ControlSpec::SetSympathetic(change));
                self.phrases.push(entry.clone());
                let _ = self.audio_tx.send(AudioCmd::AddPhrase(entry));
                let _ = self.audio_tx.send(AudioCmd::SetSympathetic(change));
                self.message = Some(src);
            }
            Cmd::SetBpm(change) => {
                let bpm = match apply_bpm_change(self.bpm, change) {
                    Ok(v) => v,
                    Err(e) => {
                        self.message = Some(format!("✗ {e}"));
                        return;
                    }
                };
                let id = self.next_phrase_id;
                self.next_phrase_id += 1;
                let entry = build_control_entry(id, format!("bpm {bpm}"), ControlSpec::SetBpm(bpm));
                self.phrases.push(entry.clone());
                self.bpm = bpm;
                let _ = self.audio_tx.send(AudioCmd::AddPhrase(entry));
                let _ = self.audio_tx.send(AudioCmd::SetBpm(bpm));
                self.push_clockout_bpm(bpm);
                self.message = Some(format!("BPM line → {bpm:.2}"));
            }
            Cmd::TuneTo(pitch) => {
                crate::tuning::tune_to_standard_pitch(pitch);
                self.tune_to = pitch;
                let src = tune_to_src(pitch);
                self.message = Some(match self.save_globals() {
                    Ok(()) => format!("{src} → standard MIDI {}", pitch.source_token()),
                    Err(err) => format!("✗ {err}"),
                });
            }
            Cmd::SetSustain(change) => {
                let secs = match apply_sustain_change(self.sustain, change) {
                    Ok(v) => v,
                    Err(e) => {
                        self.message = Some(format!("✗ {e}"));
                        return;
                    }
                };
                let id = self.next_phrase_id;
                self.next_phrase_id += 1;
                let entry =
                    build_control_entry(id, format!("s {secs}"), ControlSpec::SetSustain(secs));
                self.phrases.push(entry.clone());
                self.sustain = secs;
                let _ = self.audio_tx.send(AudioCmd::AddPhrase(entry));
                let _ = self.audio_tx.send(AudioCmd::SetSustain(secs));
                self.message = Some(format!("s line → {secs:.2}s"));
            }
            Cmd::SetVcf(change) => {
                let vcf = match command::apply_vcf_change(self.vcf, change) {
                    Ok(v) => v,
                    Err(e) => {
                        self.message = Some(format!("✗ {e}"));
                        return;
                    }
                };
                let id = self.next_phrase_id;
                self.next_phrase_id += 1;
                let entry =
                    build_control_entry(id, vcf_change_src(change), ControlSpec::SetVcf(change));
                self.phrases.push(entry.clone());
                self.vcf.apply(vcf);
                let _ = self.audio_tx.send(AudioCmd::AddPhrase(entry));
                let _ = self.audio_tx.send(AudioCmd::SetVcf(change));
                self.message = Some(format!("VCF line → {}", describe_vcf(vcf)));
            }
            Cmd::SetFx(change) => {
                let fx = match command::apply_fx_change(self.fx, change) {
                    Ok(v) => v,
                    Err(e) => {
                        self.message = Some(format!("✗ {e}"));
                        return;
                    }
                };
                let id = self.next_phrase_id;
                self.next_phrase_id += 1;
                let entry =
                    build_control_entry(id, fx_change_src(change), ControlSpec::SetFx(change));
                self.phrases.push(entry.clone());
                self.fx = fx;
                let _ = self.audio_tx.send(AudioCmd::AddPhrase(entry));
                let _ = self.audio_tx.send(AudioCmd::SetFx(change));
                self.message = Some(format!("FX line → {}", describe_fx(fx)));
            }
            Cmd::SetNam(command) => {
                if let Some(control) = nam_timeline_control(&command) {
                    let id = self.next_phrase_id;
                    self.next_phrase_id += 1;
                    let src = nam_command_src(&command).unwrap();
                    let entry = build_control_entry(id, src, control);
                    self.phrases.push(entry.clone());
                    let _ = self.audio_tx.send(AudioCmd::AddPhrase(entry));
                }
                self.apply_nam_command(command);
            }

            Cmd::EditJump { id, to, times } => {
                if times <= 1 {
                    self.message = Some(
                        "✗ jump ×1 is a no-op; edit the row to a phrase/control line or use j <id> 2"
                            .into(),
                    );
                    return;
                }
                let Some(id) = self.resolve_id_ref(id) else {
                    self.message = Some(format!("✗ no phrase id {id}"));
                    return;
                };
                let Some(to) = self.resolve_id_ref(to) else {
                    self.message = Some(format!("✗ no phrase id {to}"));
                    return;
                };
                if !self.phrases.iter().any(|p| p.id == to) {
                    self.message = Some(format!("✗ no phrase id {to}"));
                    return;
                }
                let pos = match self.phrases.iter().position(|p| p.id == id) {
                    Some(p) => p,
                    None => {
                        self.message = Some(format!("✗ no phrase id {id}"));
                        return;
                    }
                };
                let mut entry = crate::sequencer::build_jump_entry(id, to, times);
                entry.id = id; // preserve the original id
                self.phrases[pos] = entry.clone();
                let _ = self.audio_tx.send(AudioCmd::ReplacePhrase(entry));
                self.message = Some(format!("edited {id} → jump to {to} ×{times}"));
            }

            Cmd::Edit {
                id,
                source,
                specs,
                repeat,
            } => {
                let Some(id) = self.resolve_id_ref(id) else {
                    self.message = Some(format!("✗ no phrase id {id}"));
                    return;
                };
                let pos = match self.phrases.iter().position(|p| p.id == id) {
                    Some(p) => p,
                    None => {
                        self.message = Some(format!("✗ no phrase id {id}"));
                        return;
                    }
                };
                let resolved = match resolve_rhythms(specs, &self.last_rhythm) {
                    Ok(r) => r,
                    Err(e) => {
                        self.message = Some(format!("✗ {e}"));
                        return;
                    }
                };
                if let Some(r) = resolved.last() {
                    self.last_rhythm = r.groups.clone();
                }
                let mut phrase = build_phrase(id, source, resolved, 4, repeat.max(1));
                phrase.id = id;
                self.phrases[pos] = phrase.clone();
                let _ = self.audio_tx.send(AudioCmd::ReplacePhrase(phrase));
                // Editing establishes the score-design position. Move playback
                // to the edited phrase so the TUI immediately marks it current
                // and the audio thread can publish its jump-aware successor.
                crate::CUR_PHRASE.store(pos, std::sync::atomic::Ordering::Relaxed);
                crate::CUR_SUBDIV.store(0, std::sync::atomic::Ordering::Relaxed);
                crate::CUR_PLAYS.store(0, std::sync::atomic::Ordering::Relaxed);
                crate::NEXT_PHRASE.store(usize::MAX, std::sync::atomic::Ordering::Relaxed);
                crate::EXIT_PHRASE.store(usize::MAX, std::sync::atomic::Ordering::Relaxed);
                let _ = self.audio_tx.send(AudioCmd::SetCurPhrase(pos));
                self.message = Some(format!("edited {id}"));
            }

            Cmd::EditBpm { id, change } => {
                let Some(id) = self.resolve_id_ref(id) else {
                    self.message = Some(format!("✗ no phrase id {id}"));
                    return;
                };
                let pos = match self.phrases.iter().position(|p| p.id == id) {
                    Some(p) => p,
                    None => {
                        self.message = Some(format!("✗ no phrase id {id}"));
                        return;
                    }
                };
                let bpm = match apply_bpm_change(self.bpm, change) {
                    Ok(v) => v,
                    Err(e) => {
                        self.message = Some(format!("✗ {e}"));
                        return;
                    }
                };
                let entry = build_control_entry(id, format!("bpm {bpm}"), ControlSpec::SetBpm(bpm));
                self.phrases[pos] = entry.clone();
                let _ = self.audio_tx.send(AudioCmd::ReplacePhrase(entry));
                self.message = Some(format!("edited {id} → bpm {bpm:.2}"));
            }

            Cmd::EditSustain { id, change } => {
                let Some(id) = self.resolve_id_ref(id) else {
                    self.message = Some(format!("✗ no phrase id {id}"));
                    return;
                };
                let pos = match self.phrases.iter().position(|p| p.id == id) {
                    Some(p) => p,
                    None => {
                        self.message = Some(format!("✗ no phrase id {id}"));
                        return;
                    }
                };
                let secs = match apply_sustain_change(self.sustain, change) {
                    Ok(v) => v,
                    Err(e) => {
                        self.message = Some(format!("✗ {e}"));
                        return;
                    }
                };
                let entry =
                    build_control_entry(id, format!("s {secs}"), ControlSpec::SetSustain(secs));
                self.phrases[pos] = entry.clone();
                let _ = self.audio_tx.send(AudioCmd::ReplacePhrase(entry));
                self.message = Some(format!("edited {id} → s {secs:.2}s"));
            }

            Cmd::EditVcf { id, change } => {
                let Some(id) = self.resolve_id_ref(id) else {
                    self.message = Some(format!("✗ no phrase id {id}"));
                    return;
                };
                let pos = match self.phrases.iter().position(|p| p.id == id) {
                    Some(p) => p,
                    None => {
                        self.message = Some(format!("✗ no phrase id {id}"));
                        return;
                    }
                };
                let vcf = match command::apply_vcf_change(self.vcf, change) {
                    Ok(v) => v,
                    Err(e) => {
                        self.message = Some(format!("✗ {e}"));
                        return;
                    }
                };
                let entry =
                    build_control_entry(id, vcf_change_src(change), ControlSpec::SetVcf(change));
                self.phrases[pos] = entry.clone();
                let _ = self.audio_tx.send(AudioCmd::ReplacePhrase(entry));
                self.message = Some(format!("edited {id} → {}", describe_vcf(vcf)));
            }

            Cmd::EditFx { id, change } => {
                let Some(id) = self.resolve_id_ref(id) else {
                    self.message = Some(format!("✗ no phrase id {id}"));
                    return;
                };
                let pos = match self.phrases.iter().position(|p| p.id == id) {
                    Some(p) => p,
                    None => {
                        self.message = Some(format!("✗ no phrase id {id}"));
                        return;
                    }
                };
                let fx = match command::apply_fx_change(self.fx, change) {
                    Ok(v) => v,
                    Err(e) => {
                        self.message = Some(format!("✗ {e}"));
                        return;
                    }
                };
                let entry =
                    build_control_entry(id, fx_change_src(change), ControlSpec::SetFx(change));
                self.phrases[pos] = entry.clone();
                let _ = self.audio_tx.send(AudioCmd::ReplacePhrase(entry));
                self.message = Some(format!("edited {id} → {}", describe_fx(fx)));
            }

            Cmd::EditNam { id, command } => self.replace_nam_control(id, command),

            Cmd::EditSympathetics { id, enabled } => {
                self.replace_sym_control(
                    id,
                    if enabled { "sym on" } else { "sym off" }.into(),
                    ControlSpec::SetSympathetics(enabled),
                );
            }

            Cmd::EditSympatheticDecay { id, decay } => {
                self.replace_sym_control(
                    id,
                    format!("sym decay {decay}"),
                    ControlSpec::SetSympatheticDecay(decay),
                );
            }

            Cmd::EditSympatheticGain { id, gain } => {
                self.replace_sym_control(
                    id,
                    format!("sym gain {gain}"),
                    ControlSpec::SetSympatheticGain(gain),
                );
            }

            Cmd::EditSympathetic { id, change } => {
                self.replace_sym_control(
                    id,
                    sym_change_src(change),
                    ControlSpec::SetSympathetic(change),
                );
            }

            Cmd::DeleteBars(ids) => {
                let mut not_found = Vec::new();
                for id_ref in &ids {
                    let Some(id) = self.resolve_id_ref(*id_ref) else {
                        not_found.push(*id_ref);
                        continue;
                    };
                    if let Some(pos) = self.phrases.iter().position(|p| p.id == id) {
                        let removed = self.phrases.remove(pos);
                        let _ = self.audio_tx.send(AudioCmd::RemovePhrase(removed.id));
                    } else {
                        not_found.push(*id_ref);
                    }
                }
                if !not_found.is_empty() {
                    let s: Vec<String> = not_found.iter().map(|i| i.to_string()).collect();
                    self.message = Some(format!("✗ no id {}", s.join(" ")));
                } else {
                    self.message = None;
                }
            }

            Cmd::AddPhrase {
                source,
                specs,
                repeat,
            } => {
                if specs.is_empty() {
                    self.message = Some("✗ empty phrase".into());
                    return;
                }
                let resolved = match resolve_rhythms(specs, &self.last_rhythm) {
                    Ok(r) => r,
                    Err(e) => {
                        self.message = Some(format!("✗ {e}"));
                        return;
                    }
                };
                if let Some(r) = resolved.last() {
                    self.last_rhythm = r.groups.clone();
                }
                let peak: usize = if self.phrases.is_empty() {
                    4
                } else {
                    let total: usize = self.phrases.iter().map(|p| p.bar.total_subdivs).sum();
                    let count = self.phrases.len().max(1);
                    (total / count / 2).clamp(2, 4)
                };
                let id = self.next_phrase_id;
                self.next_phrase_id += 1;
                let phrase = build_phrase(id, source, resolved, peak, repeat.max(1));
                let _ = self.audio_tx.send(AudioCmd::AddPhrase(phrase.clone()));
                self.phrases.push(phrase);
                self.message = None;
            }
        }
    }

    fn apply_nam_command(&mut self, command: NamCommand) {
        let display_src = nam_command_src(&command);
        match command {
            NamCommand::Login => self.start_tone3000_login(),
            NamCommand::Logout => {
                self.pending_tone3000_download = None;
                match fs::remove_file(tone3000_auth_path()) {
                    Ok(()) => self.message = Some("TONE3000 logged out".into()),
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                        self.message = Some("TONE3000 was not logged in".into())
                    }
                    Err(err) => {
                        self.message =
                            Some(format!("✗ could not remove TONE3000 credentials: {err}"))
                    }
                }
            }
            NamCommand::Off => {
                let _ = self.audio_tx.send(AudioCmd::SetNamEnabled(false));
                crate::clear_nam_error();
                crate::NAM_STATUS.store(5, std::sync::atomic::Ordering::Relaxed);
                replace_live_nam_command(&mut self.live_nam_commands, display_src);
                self.message = Some("NAM input amp off".into());
            }
            NamCommand::List => {
                let cache_dir = nam_cache_dir();
                let cached = list_cached_nam_models(&cache_dir);
                let files = list_current_dir_nam_files();
                self.message = Some(match (cached, files) {
                    (Ok(cached), Ok(files)) if cached.is_empty() && files.is_empty() => {
                        format!(
                            "no NAM captures found; {} is ready. Run `nam import URL as name` to download into it, or `nam import FILENAME.nam as name` to cache a local file",
                            cache_dir.display()
                        )
                    }
                    (Ok(cached), Ok(files)) => {
                        let mut parts = Vec::new();
                        if !cached.is_empty() {
                            parts.push(format!("cached: {}", cached.join("  ")));
                        }
                        if !files.is_empty() {
                            parts.push(format!("files: {}", files.join("  ")));
                        }
                        parts.join(" | ")
                    }
                    (Err(err), _) => format!(
                        "✗ cannot list NAM cache {}: {err}; create ./.nam or set MAQAM_NAM_CACHE_DIR",
                        cache_dir.display()
                    ),
                    (Ok(_), Err(err)) => format!(
                        "✗ cannot list NAM files in current directory: {err}; check directory permissions"
                    ),
                });
            }
            NamCommand::Search { query } => {
                self.message = Some(match find_nam_captures(&query) {
                    Ok(results) => results,
                    Err(err) => format!("✗ {err}"),
                });
            }
            NamCommand::Gain(gain) => {
                let _ = self.audio_tx.send(AudioCmd::SetNamGain(gain));
                replace_live_nam_command(&mut self.live_nam_commands, display_src);
                self.message = Some(format!("NAM input gain {gain:.2}"));
            }
            NamCommand::Input(route) => {
                let _ = self.audio_tx.send(AudioCmd::SetNamInput(route));
                replace_live_nam_command(&mut self.live_nam_commands, display_src);
                self.message = Some(format!("NAM input → {}", nam_input_name(route)));
            }
            NamCommand::Latency(route) => {
                let (tx, rx) = crossbeam_channel::bounded(1);
                self.nam_latency_rx = Some(rx);
                let _ = self.audio_tx.send(AudioCmd::MeasureInputLatency {
                    input: route,
                    result_tx: tx,
                });
                self.message = Some(format!(
                    "measuring capture-to-playback device timestamps for {} input…",
                    nam_input_name(route)
                ));
            }
            NamCommand::Import { path, name } => {
                if is_http_url(&path) {
                    self.start_nam_download(path, name, false);
                    return;
                }
                let source = Path::new(&path);
                if !source.is_file() {
                    self.message = Some(format!(
                        "✗ NAM model file not found: {path}; run `nam import URL as name` to download into ./.nam, or `nam import FILENAME.nam as name` for a real local file"
                    ));
                    return;
                }
                let cache_dir = nam_cache_dir();
                if let Err(err) = fs::create_dir_all(&cache_dir) {
                    self.message = Some(format!(
                        "✗ cannot create NAM cache {}: {err}; create ./.nam or set MAQAM_NAM_CACHE_DIR to a writable directory",
                        cache_dir.display()
                    ));
                    return;
                }
                let cache_name = name
                    .as_deref()
                    .map(sanitize_nam_cache_name)
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| nam_cache_name_from_path(source));
                let dest = cache_dir.join(format!("{cache_name}.nam"));
                if let Err(err) = fs::copy(source, &dest) {
                    self.message = Some(format!(
                        "✗ cannot import NAM model to {}: {err}; check file permissions on ./.nam or set MAQAM_NAM_CACHE_DIR",
                        dest.display()
                    ));
                    return;
                }
                self.message = Some(format!("NAM capture imported: {cache_name}"));
            }
            NamCommand::Pin { url, name } => {
                let name = sanitize_nam_cache_name(&name);
                if name.is_empty() {
                    self.message = Some("✗ NAM pin name is empty".into());
                    return;
                }
                if let Some(path) = self.session_path.as_deref() {
                    if let Err(err) = pin_nam_dependency(path, &name, &url) {
                        self.message = Some(format!("✗ could not update {path}: {err}"));
                        return;
                    }
                }
                self.pending_nam_slot = None;
                replace_live_nam_command(
                    &mut self.live_nam_commands,
                    Some(format!("nam pin {url} as {name}")),
                );
                self.start_nam_download(url, Some(name), true);
            }
            NamCommand::Tone3000 { tone_id, name } => {
                let name = sanitize_nam_cache_name(&name);
                let src = format!("nam tone3000 {tone_id} as {name}");
                if let Some(path) = self.session_path.as_deref() {
                    if let Err(err) = pin_nam_reference(path, &name, &src) {
                        self.message = Some(format!("✗ could not update {path}: {err}"));
                        return;
                    }
                }
                self.pending_nam_slot = None;
                replace_live_nam_command(&mut self.live_nam_commands, Some(src));
                self.start_tone3000_download(tone_id, name);
            }
            NamCommand::Load { path } => {
                if is_http_url(&path) {
                    self.start_nam_download(path, None, true);
                    return;
                }
                let (audio_cmd, message) = match nam_load_audio_cmd(&path) {
                    Ok(value) => value,
                    Err(err) => {
                        crate::set_nam_error(err.clone());
                        self.message = Some(format!("✗ {err}"));
                        return;
                    }
                };
                let _ = self.audio_tx.send(audio_cmd);
                mark_nam_loaded();
                replace_live_nam_command(&mut self.live_nam_commands, display_src);
                self.message = Some(message);
            }
        }
    }

    fn start_nam_download(&mut self, url: String, name: Option<String>, load_after: bool) {
        if self.nam_download_rx.is_some() {
            self.message = Some(
                "✗ NAM download already running; wait for it to finish, then run the command again"
                    .into(),
            );
            return;
        }
        let cache_dir = nam_cache_dir();
        if let Err(err) = fs::create_dir_all(&cache_dir) {
            self.message = Some(format!(
                "✗ cannot create NAM cache {}: {err}; create ./.nam or set MAQAM_NAM_CACHE_DIR to a writable directory",
                cache_dir.display()
            ));
            return;
        }
        let cache_name = name
            .as_deref()
            .map(sanitize_nam_cache_name)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| nam_cache_name_from_url(&url));
        if cache_name.is_empty() {
            self.message = Some(
                "✗ cannot infer a NAM cache name from that URL; run `nam import URL as name`"
                    .into(),
            );
            return;
        }
        let cached_path = cache_dir.join(format!("{cache_name}.nam"));
        if cached_path.is_file() {
            self.finish_cached_nam(&cache_name, load_after);
            return;
        }

        let (tx, rx) = crossbeam_channel::bounded(32);
        crate::clear_nam_error();
        crate::NAM_STATUS.store(3, std::sync::atomic::Ordering::Relaxed);
        self.nam_download_rx = Some(rx);
        self.nam_download_progress = Some(NamDownloadProgress {
            name: cache_name.clone(),
            downloaded: 0,
            total: None,
            load_after,
        });
        self.message = Some(format!("downloading NAM capture → {cache_name}"));

        std::thread::spawn(move || {
            let result =
                download_nam_capture(&url, &cache_dir, &cache_name, load_after, None, tx.clone());
            if let Err(err) = result {
                let _ = tx.send(Err(err));
            }
        });
    }

    fn start_tone3000_download(&mut self, tone_id: u64, name: String) {
        let cache_dir = nam_cache_dir();
        if cache_dir.join(format!("{name}.nam")).is_file() {
            self.finish_cached_nam(&name, true);
            return;
        }
        let token = match tone3000_access_token() {
            Ok(token) => token,
            Err(_) => {
                crate::NAM_STATUS.store(2, std::sync::atomic::Ordering::Relaxed);
                self.pending_tone3000_download = Some((tone_id, name));
                self.start_tone3000_login();
                return;
            }
        };
        crate::clear_nam_error();
        crate::NAM_STATUS.store(3, std::sync::atomic::Ordering::Relaxed);
        let (tx, rx) = crossbeam_channel::unbounded();
        self.nam_download_rx = Some(rx);
        self.nam_download_progress = Some(NamDownloadProgress {
            name: name.clone(),
            downloaded: 0,
            total: None,
            load_after: true,
        });
        self.message = Some(format!("downloading TONE3000 tone {tone_id} → {name}"));
        std::thread::spawn(move || {
            let result = tone3000_model_url(tone_id, &token).and_then(|url| {
                download_nam_capture(&url, &cache_dir, &name, true, Some(&token), tx.clone())
            });
            if let Err(err) = result {
                let _ = tx.send(Err(err));
            }
        });
    }

    fn finish_cached_nam(&mut self, name: &str, load_after: bool) {
        self.nam_download_progress = None;
        self.nam_download_rx = None;
        if load_after {
            match nam_load_audio_cmd(name) {
                Ok((audio_cmd, message)) => {
                    let _ = self.audio_tx.send(audio_cmd);
                    mark_nam_loaded();
                    replace_live_nam_command(
                        &mut self.live_nam_commands,
                        Some(format!("nam {name}")),
                    );
                    self.message = Some(format!("NAM capture already cached; {message}"));
                }
                Err(err) => {
                    crate::set_nam_error(err.clone());
                    self.message = Some(format!("✗ {err}"));
                }
            }
        } else {
            crate::clear_nam_error();
            crate::NAM_STATUS.store(0, std::sync::atomic::Ordering::Relaxed);
            self.message = Some(format!("NAM capture already cached: {name}"));
        }
    }

    fn start_tone3000_login(&mut self) {
        if self.tone3000_auth_rx.is_some() {
            self.message = Some("TONE3000 login is already waiting in the browser".into());
            return;
        }
        let client_id = std::env::var("TONE3000_CLIENT_ID")
            .or_else(|_| std::env::var("TONE3000_PUBLISHABLE_KEY"))
            .or_else(|_| load_tone3000_auth().map(|auth| auth.client_id))
            .unwrap_or_else(|_| TONE3000_PUBLISHABLE_CLIENT_ID.to_string());
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.tone3000_auth_rx = Some(rx);
        self.message =
            Some("TONE3000 login opened in your browser; maqam-live is still running".into());
        std::thread::spawn(move || {
            let _ = tx.send(tone3000_browser_login(&client_id));
        });
    }

    /// Push a BPM update to the clockout thread if it's running.
    fn push_clockout_bpm(&self, bpm: f64) {
        if let Some(tx) = &self.clockout_tx {
            let _ = tx.send(bpm);
        }
    }

    fn ask_llm(&mut self, provider: LlmProvider, prompt: String) {
        if self.llm_rx.is_some() {
            self.message = Some("✗ already asking LLM".into());
            return;
        }
        let Some(request) = LlmRequest::from_env(
            provider,
            prompt.clone(),
            self.llm_history.clone(),
            self.llm_score_context(),
        ) else {
            self.message = Some(match provider {
                LlmProvider::ChatGpt => {
                    "✗ environment variable OPENAI_API_KEY needs to be set to talk to chatgpt"
                        .into()
                }
                LlmProvider::Claude => {
                    "✗ environment variable ANTHROPIC_API_KEY or CLAUDE_API_KEY needs to be set to talk to claude"
                        .into()
                }
            });
            return;
        };
        let provider_name = request.provider_name();
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.llm_rx = Some(rx);
        self.message = Some(format!("asking {provider_name}..."));
        std::thread::spawn(move || {
            let result = request.send().map(|answer| LlmOutcome::Answer {
                sent_prompt: prompt,
                answer,
            });
            let _ = tx.send(result);
        });
    }

    fn ask_llm_for_edit(&mut self, provider: LlmProvider, prompt: String) {
        if self.llm_rx.is_some() {
            self.message =
                Some("✗ already asking LLM; wait for the current answer, then try again".into());
            return;
        }
        let edit_prompt = llm_edit_prompt(&prompt);
        let sent_prompt = edit_prompt.clone();
        let Some(request) = LlmRequest::from_env(
            provider,
            edit_prompt,
            self.llm_history.clone(),
            self.llm_score_context(),
        ) else {
            self.message = Some(match provider {
                LlmProvider::ChatGpt => {
                    "✗ environment variable OPENAI_API_KEY needs to be set to ask chatgpt to edit"
                        .into()
                }
                LlmProvider::Claude => {
                    "✗ environment variable ANTHROPIC_API_KEY or CLAUDE_API_KEY needs to be set to ask claude to edit"
                        .into()
                }
            });
            return;
        };
        let provider_name = request.provider_name();
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.llm_rx = Some(rx);
        self.message = Some(format!("asking {provider_name} for edits..."));
        std::thread::spawn(move || {
            let result = request.send_edit().map(|commands| LlmOutcome::Edit {
                sent_prompt,
                commands,
            });
            let _ = tx.send(result);
        });
    }

    fn remember_llm_exchange(&mut self, user_prompt: String, assistant_answer: String) {
        self.llm_history.push(LlmChatMessage {
            role: LlmRole::User,
            content: user_prompt,
        });
        self.llm_history.push(LlmChatMessage {
            role: LlmRole::Assistant,
            content: assistant_answer,
        });
    }

    fn apply_llm_edit_commands(&mut self, commands: Vec<String>) {
        if commands.is_empty() {
            self.message = Some(
                "✗ the LLM did not return any commands; ask it to return maqam-live commands separated by semicolons or newlines"
                    .into(),
            );
            return;
        }
        let commands = minimize_repeated_llm_phrase_commands(commands, self.next_phrase_id);
        let mut parsed_commands = Vec::new();
        for command_src in &commands {
            let parsed = match command::parse(command_src) {
                Ok(cmd) => cmd,
                Err(err) => {
                    self.message = Some(format!(
                        "✗ LLM returned `{command_src}`, which is not valid: {err}; ask it to return maqam-live commands only"
                    ));
                    return;
                }
            };
            if !llm_edit_command_allowed(&parsed) {
                self.message = Some(llm_rejected_edit_command_message(command_src, &parsed));
                return;
            }
            parsed_commands.push(parsed);
        }

        let snapshot = self.snapshot_score_state();
        let mut applied = 0usize;
        for parsed in parsed_commands {
            self.execute(parsed);
            if self
                .message
                .as_deref()
                .is_some_and(|msg| msg.starts_with('✗'))
            {
                let error = self
                    .message
                    .clone()
                    .unwrap_or_else(|| "✗ LLM edit failed".into());
                self.restore_score_state(snapshot);
                self.message = Some(format!(
                    "{error}; restored the previous phrases, so ask for a simpler edit or try again"
                ));
                return;
            }
            applied += 1;
        }
        self.message = Some(format!("LLM applied {applied} edit command(s)"));
    }

    fn snapshot_score_state(&self) -> ScoreSnapshot {
        ScoreSnapshot {
            phrases: self.phrases.clone(),
            next_phrase_id: self.next_phrase_id,
            last_rhythm: self.last_rhythm.clone(),
            bpm: self.bpm,
            sustain: self.sustain,
            vcf: self.vcf,
            fx: self.fx,
            paused: self.paused,
        }
    }

    fn restore_score_state(&mut self, snapshot: ScoreSnapshot) {
        self.phrases = snapshot.phrases;
        self.next_phrase_id = snapshot.next_phrase_id;
        self.last_rhythm = snapshot.last_rhythm;
        self.bpm = snapshot.bpm;
        self.sustain = snapshot.sustain;
        self.vcf = snapshot.vcf;
        self.fx = snapshot.fx;
        self.paused = snapshot.paused;
        self.resync_audio_sequence(None);
    }

    fn llm_score_context(&self) -> String {
        if self.phrases.is_empty() {
            return "score is empty; time steps are sounding phrase repeat passes, not timeline rows"
                .into();
        }
        let mut time_step = 0usize;
        self.phrases
            .iter()
            .map(|phrase| {
                let src = phrase.display_src();
                if let Some(jump) = &phrase.jump {
                    return format!(
                        "{}: {}  [jump row; restarts at id {} for {} passes; consumes no sounding time step]",
                        phrase.id, src, jump.target_id, jump.times
                    );
                }
                if phrase.control.is_some() {
                    return format!("{}: {}  [control row; consumes no sounding time step]", phrase.id, src);
                }
                let start = time_step;
                let steps = phrase.repeat.max(1);
                let end = start + steps - 1;
                time_step += steps;
                format!(
                    "{}: {}  [time steps {}..{}; {} repeat pass(es); rhythm groups {}; {} subdivisions per pass]",
                    phrase.id,
                    src,
                    start,
                    end,
                    steps,
                    rhythm_groups_display(&phrase.bar.groups),
                    phrase.bar.total_subdivs
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn save_session(&self, path: &str) -> Result<(), String> {
        let out = crate::session_v3::serialize_session_v3(&self.phrases);
        fs::write(path, out).map_err(|e| e.to_string())
    }

    fn globals_path(&self) -> PathBuf {
        #[cfg(test)]
        let default = || std::env::temp_dir().join("maqam-live-default-test.globals.ml");
        #[cfg(not(test))]
        let default = || PathBuf::from(".globals.ml");

        std::env::var("MAQAM_GLOBALS_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| default())
    }

    fn save_globals(&self) -> Result<(), String> {
        let path = self.globals_path();
        let out = format!("vol {}\ntuneto {}\n", self.vol, self.tune_to.source_token());
        fs::write(&path, out).map_err(|e| {
            format!(
                "could not write {}; check directory permissions, then try the command again: {e}",
                path.display()
            )
        })
    }

    fn load_globals(&mut self) -> Result<(), String> {
        let path = self.globals_path();
        let source = match fs::read_to_string(&path) {
            Ok(source) => source,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => {
                return Err(format!(
                    "could not read {}; check file permissions or remove the file, then restart maqam-live: {err}",
                    path.display()
                ));
            }
        };

        for (idx, raw_line) in source.lines().enumerate() {
            let line_no = idx + 1;
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let parsed = command::parse(line)
                .map_err(|e| format!("{} line {line_no}: {e}", path.display()))?;
            match parsed {
                Cmd::SetVol(value) => {
                    self.vol = value;
                }
                Cmd::TuneTo(pitch) => {
                    self.tune_to = pitch;
                    crate::tuning::tune_to_standard_pitch(pitch);
                }
                _ => {
                    return Err(format!(
                        "{} line {line_no}: globals only support vol and tuneto; remove this line or change it to vol <n> or tuneto <pitch>",
                        path.display()
                    ));
                }
            }
        }
        Ok(())
    }

    fn load_session(&mut self, path: &str) -> Result<(), String> {
        let src = fs::read_to_string(path).map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                format!("{path} not found; run `ls` to see available .mq files, or use `load FILENAME.mq` with an existing score file")
            } else {
                format!(
                    "cannot read {path}: {err}; check file permissions or choose another .mq file"
                )
            }
        })?;
        let mut lines = src.lines();
        let Some(header) = lines.next() else {
            return Err("empty file".into());
        };
        let header = header.trim();
        if header == crate::session_v3::HEADER {
            return self.load_session_v3(lines);
        }
        if header == "MAQAM_SESSION_V2" {
            return self.load_session_v2(lines);
        }
        if header == "MAQAM_SESSION_V1" {
            return self.load_session_v1(lines);
        }
        Err("bad header (expected MAQAM_SESSION_V3, MAQAM_SESSION_V2, or MAQAM_SESSION_V1)".into())
    }

    fn load_session_v3<'a, I>(&mut self, lines: I) -> Result<(), String>
    where
        I: Iterator<Item = &'a str>,
    {
        let lines = lines.collect::<Vec<_>>();
        let reserved_ids = lines
            .iter()
            .filter_map(|line| {
                crate::session_v3::split_escaped_fields(line)
                    .get(1)?
                    .parse::<usize>()
                    .ok()
            })
            .collect::<std::collections::HashSet<_>>();
        crate::tuning::Maqam::reset_to_defaults();
        self.pending_nam_slot = None;
        let mut new_bpm = 120.0f64;
        let mut new_sustain = 1.25f64;
        let mut new_vcf = VcfBank::default();
        let mut new_fx = FxSettings::default();
        let new_vol = self.vol;
        let mut loaded: Vec<Phrase> = Vec::new();
        let mut ids = std::collections::HashSet::new();
        let mut max_id = None;
        let mut next_legacy_id = 0usize;
        let mut last_rhythm = vec![3, 3, 2];
        let mut live_nam_audio_cmds = Vec::new();
        let mut live_nam_commands = Vec::new();
        let mut live_nam_warnings = Vec::new();
        let mut pending_nam_pin: Option<(String, String)> = None;
        let mut pending_tone3000: Option<(u64, String)> = None;

        for (line_idx, raw_line) in lines.into_iter().enumerate() {
            let line_no = line_idx + 2;
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with("create ") {
                let parsed = command::parse(line).map_err(|e| format!("line {line_no}: {e}"))?;
                let Cmd::CreateJins { name, ratios } = parsed else {
                    return Err(format!("line {line_no}: expected create line"));
                };
                crate::tuning::Maqam::create(&name, ratios)
                    .map_err(|e| format!("line {line_no}: {e}"))?;
                continue;
            }
            if line.starts_with("vol ") {
                continue;
            }
            if line
                .split_whitespace()
                .next()
                .is_some_and(|word| word.eq_ignore_ascii_case("nam"))
            {
                let parsed = command::parse(line).map_err(|e| format!("line {line_no}: {e}"))?;
                let Cmd::SetNam(command) = parsed else {
                    return Err(format!("line {line_no}: expected nam line"));
                };
                let display_src = nam_command_src(&command);
                let missing_slot = match &command {
                    NamCommand::Load { path } => Some(path.clone()),
                    _ => None,
                };
                let pin = match &command {
                    NamCommand::Pin { url, name } => Some((url.clone(), name.clone())),
                    _ => None,
                };
                let tone3000 = match &command {
                    NamCommand::Tone3000 { tone_id, name } => Some((*tone_id, name.clone())),
                    _ => None,
                };
                match nam_session_audio_cmd(command) {
                    Ok(audio_cmd) => live_nam_audio_cmds.push(audio_cmd),
                    Err(err) => {
                        if let Some(tone3000) = tone3000 {
                            pending_tone3000 = Some(tone3000);
                        } else if let Some(pin) = pin {
                            pending_nam_pin = Some(pin);
                        } else if let Some(slot) = missing_slot {
                            self.pending_nam_slot = Some(slot.clone());
                            live_nam_warnings.push(format!(
                                "This score needs a NAM model for “{slot}”. What amp or tone should it use?"
                            ));
                        } else {
                            live_nam_warnings.push(format!("line {line_no}: {err}"));
                        }
                    }
                }
                replace_live_nam_command(&mut live_nam_commands, display_src);
                if let Some(control) = nam_timeline_control(
                    &command::parse(line)
                        .ok()
                        .and_then(|cmd| {
                            if let Cmd::SetNam(command) = cmd {
                                Some(command)
                            } else {
                                None
                            }
                        })
                        .ok_or_else(|| format!("line {line_no}: expected nam line"))?,
                ) {
                    while ids.contains(&next_legacy_id) || reserved_ids.contains(&next_legacy_id) {
                        next_legacy_id += 1;
                    }
                    let id = next_legacy_id;
                    next_legacy_id += 1;
                    ids.insert(id);
                    max_id = Some(max_id.map_or(id, |current: usize| current.max(id)));
                    loaded.push(build_control_entry(id, line.to_string(), control));
                }
                continue;
            }
            if is_plain_control_line(line) {
                while ids.contains(&next_legacy_id) || reserved_ids.contains(&next_legacy_id) {
                    next_legacy_id += 1;
                }
                let id = next_legacy_id;
                next_legacy_id += 1;
                ids.insert(id);
                max_id = Some(max_id.map_or(id, |current: usize| current.max(id)));

                let parsed = command::parse(line).map_err(|e| format!("line {line_no}: {e}"))?;
                match parsed {
                    Cmd::SetBpm(change) => {
                        new_bpm = apply_bpm_change(new_bpm, change)
                            .map_err(|e| format!("line {line_no}: {e}"))?;
                        loaded.push(build_control_entry(
                            id,
                            format!("bpm {new_bpm}"),
                            ControlSpec::SetBpm(new_bpm),
                        ));
                    }
                    Cmd::SetSustain(change) => {
                        new_sustain = apply_sustain_change(new_sustain, change)
                            .map_err(|e| format!("line {line_no}: {e}"))?;
                        loaded.push(build_control_entry(
                            id,
                            format!("s {new_sustain}"),
                            ControlSpec::SetSustain(new_sustain),
                        ));
                    }
                    Cmd::SetVcf(change) => {
                        let setting = command::apply_vcf_change(new_vcf, change)
                            .map_err(|e| format!("line {line_no}: {e}"))?;
                        new_vcf.apply(setting);
                        loaded.push(build_control_entry(
                            id,
                            vcf_change_src(change),
                            ControlSpec::SetVcf(change),
                        ));
                    }
                    Cmd::SetFx(change) => {
                        new_fx = command::apply_fx_change(new_fx, change)
                            .map_err(|e| format!("line {line_no}: {e}"))?;
                        loaded.push(build_control_entry(
                            id,
                            fx_change_src(change),
                            ControlSpec::SetFx(change),
                        ));
                    }
                    Cmd::Sympathetics(enabled) => {
                        loaded.push(build_control_entry(
                            id,
                            if enabled { "sym on" } else { "sym off" }.into(),
                            ControlSpec::SetSympathetics(enabled),
                        ));
                    }
                    Cmd::SympatheticDecay(decay) => {
                        loaded.push(build_control_entry(
                            id,
                            format!("sym decay {decay}"),
                            ControlSpec::SetSympatheticDecay(decay),
                        ));
                    }
                    Cmd::SympatheticGain(gain) => {
                        loaded.push(build_control_entry(
                            id,
                            format!("sym gain {gain}"),
                            ControlSpec::SetSympatheticGain(gain),
                        ));
                    }
                    Cmd::Sympathetic(change) => {
                        loaded.push(build_control_entry(
                            id,
                            sym_change_src(change),
                            ControlSpec::SetSympathetic(change),
                        ));
                    }
                    _ => return Err(format!("line {line_no}: expected control line")),
                }
                continue;
            }

            let fields = crate::session_v3::split_escaped_fields(line);
            let id = fields
                .get(1)
                .ok_or_else(|| format!("line {line_no}: missing id"))?
                .trim()
                .parse::<usize>()
                .map_err(|_| format!("line {line_no}: bad id"))?;
            if !ids.insert(id) {
                return Err(format!("line {line_no}: duplicate id {id}"));
            }
            max_id = Some(max_id.map_or(id, |current: usize| current.max(id)));

            match fields.first().map(String::as_str) {
                Some("T") if fields.len() == 3 && fields[2].trim() == "stop" => {
                    loaded.push(build_control_entry(id, "stop".into(), ControlSpec::Stop));
                }
                Some("B") if fields.len() == 3 => {
                    new_bpm = fields[2]
                        .trim()
                        .parse::<f64>()
                        .map_err(|_| format!("line {line_no}: bad bpm"))?;
                    if !(20.0..=400.0).contains(&new_bpm) {
                        return Err(format!("line {line_no}: bpm out of range"));
                    }
                    loaded.push(build_control_entry(
                        id,
                        format!("bpm {new_bpm}"),
                        ControlSpec::SetBpm(new_bpm),
                    ));
                }
                Some("S") if fields.len() == 3 => {
                    new_sustain = fields[2]
                        .trim()
                        .parse::<f64>()
                        .map_err(|_| format!("line {line_no}: bad sustain"))?;
                    if !(0.05..=10.0).contains(&new_sustain) {
                        return Err(format!("line {line_no}: sustain out of range"));
                    }
                    loaded.push(build_control_entry(
                        id,
                        format!("s {new_sustain}"),
                        ControlSpec::SetSustain(new_sustain),
                    ));
                }
                Some("V") if fields.len() == 3 => {
                    let parsed =
                        command::parse(&fields[2]).map_err(|e| format!("line {line_no}: {e}"))?;
                    let Cmd::SetVcf(change) = parsed else {
                        return Err(format!("line {line_no}: expected vcf line"));
                    };
                    let setting = command::apply_vcf_change(new_vcf, change)
                        .map_err(|e| format!("line {line_no}: {e}"))?;
                    new_vcf.apply(setting);
                    loaded.push(build_control_entry(
                        id,
                        vcf_change_src(change),
                        ControlSpec::SetVcf(change),
                    ));
                }
                Some("F") if fields.len() == 3 => {
                    let parsed =
                        command::parse(&fields[2]).map_err(|e| format!("line {line_no}: {e}"))?;
                    let Cmd::SetFx(change) = parsed else {
                        return Err(format!("line {line_no}: expected fx line"));
                    };
                    new_fx = command::apply_fx_change(new_fx, change)
                        .map_err(|e| format!("line {line_no}: {e}"))?;
                    loaded.push(build_control_entry(
                        id,
                        fx_change_src(change),
                        ControlSpec::SetFx(change),
                    ));
                }
                Some("N") if fields.len() == 3 => {
                    let parsed =
                        command::parse(&fields[2]).map_err(|e| format!("line {line_no}: {e}"))?;
                    let Cmd::SetNam(command) = parsed else {
                        return Err(format!("line {line_no}: expected nam line"));
                    };
                    let control = nam_timeline_control(&command).ok_or_else(|| {
                        format!("line {line_no}: NAM command cannot be scheduled")
                    })?;
                    let missing_slot = match &command {
                        NamCommand::Load { path } => Some(path.clone()),
                        _ => None,
                    };
                    let pin = match &command {
                        NamCommand::Pin { url, name } => Some((url.clone(), name.clone())),
                        _ => None,
                    };
                    let tone = match &command {
                        NamCommand::Tone3000 { tone_id, name } => Some((*tone_id, name.clone())),
                        _ => None,
                    };
                    match nam_session_audio_cmd(command) {
                        Ok(audio_cmd) => live_nam_audio_cmds.push(audio_cmd),
                        Err(err) => {
                            if let Some(value) = tone {
                                pending_tone3000 = Some(value);
                            } else if let Some(value) = pin {
                                pending_nam_pin = Some(value);
                            } else if let Some(slot) = missing_slot {
                                self.pending_nam_slot = Some(slot);
                            } else {
                                live_nam_warnings.push(format!("line {line_no}: {err}"));
                            }
                        }
                    }
                    loaded.push(build_control_entry(id, fields[2].clone(), control));
                }
                Some("Y") if fields.len() == 3 => {
                    let parsed =
                        command::parse(&fields[2]).map_err(|e| format!("line {line_no}: {e}"))?;
                    let (src, control) = match parsed {
                        Cmd::Sympathetics(enabled) => (
                            if enabled { "sym on" } else { "sym off" }.to_string(),
                            ControlSpec::SetSympathetics(enabled),
                        ),
                        Cmd::SympatheticDecay(decay) => (
                            format!("sym decay {decay}"),
                            ControlSpec::SetSympatheticDecay(decay),
                        ),
                        Cmd::SympatheticGain(gain) => (
                            format!("sym gain {gain}"),
                            ControlSpec::SetSympatheticGain(gain),
                        ),
                        Cmd::Sympathetic(change) => {
                            (sym_change_src(change), ControlSpec::SetSympathetic(change))
                        }
                        _ => return Err(format!("line {line_no}: expected sym line")),
                    };
                    loaded.push(build_control_entry(id, src, control));
                }
                Some("V") if (5..=8).contains(&fields.len()) => {
                    let (target, offset) = if fields.len() >= 6 {
                        let target = VcfTarget::parse(&fields[2])
                            .ok_or_else(|| format!("line {line_no}: bad vcf target"))?;
                        (target, 3)
                    } else {
                        (new_vcf.focus, 2)
                    };
                    let cutoff_hz = fields[offset]
                        .trim()
                        .parse::<f32>()
                        .map_err(|_| format!("line {line_no}: bad vcf cutoff"))?;
                    let resonance = fields[offset + 1]
                        .trim()
                        .parse::<f32>()
                        .map_err(|_| format!("line {line_no}: bad vcf resonance"))?;
                    let drive = fields[offset + 2]
                        .trim()
                        .parse::<f32>()
                        .map_err(|_| format!("line {line_no}: bad vcf drive"))?;
                    let enabled = if fields.len() >= 7 {
                        match fields[6].trim().to_ascii_lowercase().as_str() {
                            "on" | "true" | "1" => true,
                            "off" | "false" | "0" => false,
                            _ => return Err(format!("line {line_no}: bad vcf enabled flag")),
                        }
                    } else {
                        true
                    };
                    let wave = if fields.len() == 8 {
                        VcoWave::parse(&fields[7])
                            .ok_or_else(|| format!("line {line_no}: bad vcf wave"))?
                    } else {
                        new_vcf.get(target).wave
                    };
                    let change = command::VcfChange {
                        enabled: Some(enabled),
                        target: Some(target),
                        cutoff_hz: Some(ValueChange::Set(cutoff_hz as f64)),
                        resonance: Some(ValueChange::Set(resonance as f64)),
                        drive: Some(ValueChange::Set(drive as f64)),
                        wave: Some(wave),
                    };
                    let setting = command::apply_vcf_change(new_vcf, change)
                        .map_err(|e| format!("line {line_no}: {e}"))?;
                    new_vcf.apply(setting);
                    loaded.push(build_control_entry(
                        id,
                        vcf_change_src(change),
                        ControlSpec::SetVcf(change),
                    ));
                }
                Some("J") if fields.len() == 4 => {
                    let target = fields[2]
                        .trim()
                        .parse::<usize>()
                        .map_err(|_| format!("line {line_no}: bad jump target"))?;
                    let times = fields[3]
                        .trim()
                        .parse::<usize>()
                        .map_err(|_| format!("line {line_no}: bad jump times"))?;
                    loaded.push(crate::sequencer::build_jump_entry(id, target, times.max(1)));
                }
                Some("P") if fields.len() == 4 => {
                    let repeat = fields[2]
                        .trim()
                        .parse::<usize>()
                        .map_err(|_| format!("line {line_no}: bad repeat"))?
                        .max(1);
                    let src = &fields[3];
                    let parsed = command::parse(src).map_err(|e| format!("line {line_no}: {e}"))?;
                    let Cmd::AddPhrase { specs, .. } = parsed else {
                        return Err(format!("line {line_no}: expected phrase command"));
                    };
                    let resolved = resolve_rhythms(specs, &last_rhythm)
                        .map_err(|e| format!("line {line_no}: {e}"))?;
                    if let Some(rhythm) = resolved.last() {
                        last_rhythm = rhythm.groups.clone();
                    }
                    loaded.push(build_phrase(id, src.clone(), resolved, 4, repeat));
                }
                _ => return Err(format!("line {line_no}: malformed V3 record")),
            }
        }

        self.phrases = loaded.clone();
        self.next_phrase_id = max_id.map_or(0, |id| id.saturating_add(1));
        self.last_rhythm = last_rhythm;
        self.bpm = new_bpm;
        self.sustain = new_sustain;
        self.vcf = new_vcf;
        self.fx = new_fx;
        self.vol = new_vol;
        self.live_nam_commands = live_nam_commands;
        self.paused = false;
        let (start_bpm, start_sustain, start_vcf, start_fx) = self.sequence_start_settings();

        let _ = self.audio_tx.send(AudioCmd::Clear);
        let _ = self.audio_tx.send(AudioCmd::SetBpm(start_bpm));
        let _ = self.audio_tx.send(AudioCmd::SetSustain(start_sustain));
        let _ = self.audio_tx.send(AudioCmd::SetVcfBank(start_vcf));
        let _ = self.audio_tx.send(AudioCmd::SetFxSettings(start_fx));
        let _ = self.audio_tx.send(AudioCmd::SetVol(self.vol));
        let _ = self.audio_tx.send(AudioCmd::SetPaused(false));
        for cmd in live_nam_audio_cmds {
            let _ = self.audio_tx.send(cmd);
        }
        for phrase in loaded {
            let _ = self.audio_tx.send(AudioCmd::AddPhrase(phrase));
        }
        let _ = self.audio_tx.send(AudioCmd::SetCurPhrase(0));
        if let Some((url, name)) = pending_nam_pin {
            self.start_nam_download(url, Some(name), true);
        }
        if let Some((tone_id, name)) = pending_tone3000 {
            self.start_tone3000_download(tone_id, name);
        }
        if let Some(warning) = live_nam_warnings.first() {
            self.message = Some(warning.clone());
        }
        Ok(())
    }

    fn load_session_v1<'a, I>(&mut self, lines: I) -> Result<(), String>
    where
        I: Iterator<Item = &'a str>,
    {
        crate::tuning::Maqam::reset_to_defaults();
        let mut new_bpm = self.bpm;
        let mut new_sustain = self.sustain;
        let mut new_vcf = self.vcf;
        let mut new_fx = self.fx;
        let new_vol = self.vol;
        let mut loaded: Vec<Phrase> = Vec::new();
        let mut max_id = 0usize;
        let mut last_rhythm = vec![3, 3, 2];

        for (line_idx, raw_line) in lines.enumerate() {
            let line_no = line_idx + 2;
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            if line.starts_with("create ") {
                let parsed = command::parse(line).map_err(|e| format!("line {line_no}: {e}"))?;
                let Cmd::CreateJins { name, ratios } = parsed else {
                    return Err(format!("line {line_no}: expected create line"));
                };
                crate::tuning::Maqam::create(&name, ratios)
                    .map_err(|e| format!("line {line_no}: {e}"))?;
                continue;
            }

            if line.starts_with("bpm ") {
                let parsed = command::parse(line).map_err(|e| format!("line {line_no}: {e}"))?;
                let Cmd::SetBpm(change) = parsed else {
                    return Err(format!("line {line_no}: expected bpm line"));
                };
                new_bpm = apply_bpm_change(new_bpm, change)
                    .map_err(|e| format!("line {line_no}: {e}"))?;
                let entry = build_control_entry(
                    max_id,
                    format!("bpm {new_bpm}"),
                    ControlSpec::SetBpm(new_bpm),
                );
                loaded.push(entry);
                max_id += 1;
                continue;
            }
            if line.starts_with("s ") || line.starts_with("sus ") {
                let parsed = command::parse(line).map_err(|e| format!("line {line_no}: {e}"))?;
                let Cmd::SetSustain(change) = parsed else {
                    return Err(format!("line {line_no}: expected sustain line"));
                };
                new_sustain = apply_sustain_change(new_sustain, change)
                    .map_err(|e| format!("line {line_no}: {e}"))?;
                let entry = build_control_entry(
                    max_id,
                    format!("s {new_sustain}"),
                    ControlSpec::SetSustain(new_sustain),
                );
                loaded.push(entry);
                max_id += 1;
                continue;
            }
            if is_plain_vcf_control_line(line) {
                let parsed = command::parse(line).map_err(|e| format!("line {line_no}: {e}"))?;
                let Cmd::SetVcf(change) = parsed else {
                    return Err(format!("line {line_no}: expected vcf line"));
                };
                let setting = command::apply_vcf_change(new_vcf, change)
                    .map_err(|e| format!("line {line_no}: {e}"))?;
                new_vcf.apply(setting);
                let entry = build_control_entry(
                    max_id,
                    vcf_change_src(change),
                    ControlSpec::SetVcf(change),
                );
                loaded.push(entry);
                max_id += 1;
                continue;
            }
            if is_plain_fx_control_line(line) {
                let parsed = command::parse(line).map_err(|e| format!("line {line_no}: {e}"))?;
                let Cmd::SetFx(change) = parsed else {
                    return Err(format!("line {line_no}: expected fx line"));
                };
                new_fx = command::apply_fx_change(new_fx, change)
                    .map_err(|e| format!("line {line_no}: {e}"))?;
                let entry =
                    build_control_entry(max_id, fx_change_src(change), ControlSpec::SetFx(change));
                loaded.push(entry);
                max_id += 1;
                continue;
            }
            if line.starts_with("vol ") {
                continue;
            }

            if let Some(payload) = line.strip_prefix("J|") {
                let mut parts = payload.splitn(3, '|');
                let id = parts
                    .next()
                    .ok_or(format!("line {line_no}: missing jump id"))?
                    .parse::<usize>()
                    .map_err(|_| format!("line {line_no}: bad jump id"))?;
                let target = parts
                    .next()
                    .ok_or(format!("line {line_no}: missing jump target"))?
                    .parse::<usize>()
                    .map_err(|_| format!("line {line_no}: bad jump target"))?;
                let times = parts
                    .next()
                    .ok_or(format!("line {line_no}: missing jump times"))?
                    .parse::<usize>()
                    .map_err(|_| format!("line {line_no}: bad jump times"))?;
                max_id = max_id.max(id);
                loaded.push(crate::sequencer::build_jump_entry(id, target, times.max(1)));
                continue;
            }

            if let Some(payload) = line.strip_prefix("P|") {
                let mut parts = payload.splitn(3, '|');
                let id = parts
                    .next()
                    .ok_or(format!("line {line_no}: missing phrase id"))?
                    .parse::<usize>()
                    .map_err(|_| format!("line {line_no}: bad phrase id"))?;
                let repeat = parts
                    .next()
                    .ok_or(format!("line {line_no}: missing repeat"))?
                    .parse::<usize>()
                    .map_err(|_| format!("line {line_no}: bad repeat"))?;
                let src = parts
                    .next()
                    .ok_or(format!("line {line_no}: missing phrase source"))?;
                let cmd_src = if repeat > 1 {
                    format!("{src} r{repeat}")
                } else {
                    src.to_string()
                };
                let parsed =
                    command::parse(&cmd_src).map_err(|e| format!("line {line_no}: {e}"))?;
                let (specs, rep) = match parsed {
                    Cmd::AddPhrase { specs, repeat, .. } => (specs, repeat),
                    _ => return Err(format!("line {line_no}: expected phrase command")),
                };
                let resolved = resolve_rhythms(specs, &last_rhythm)
                    .map_err(|e| format!("line {line_no}: {e}"))?;
                if let Some(r) = resolved.last() {
                    last_rhythm = r.groups.clone();
                }
                let phrase = build_phrase(id, src.to_string(), resolved, 4, rep.max(1));
                max_id = max_id.max(id);
                loaded.push(phrase);
                continue;
            }

            return Err(format!("line {line_no}: unrecognized line"));
        }

        self.phrases = loaded.clone();
        self.next_phrase_id = max_id.saturating_add(1);
        self.last_rhythm = last_rhythm;
        self.bpm = new_bpm;
        self.sustain = new_sustain;
        self.vcf = new_vcf;
        self.fx = new_fx;
        self.vol = new_vol;
        self.paused = false;
        let (start_bpm, start_sustain, start_vcf, start_fx) = self.sequence_start_settings();

        let _ = self.audio_tx.send(AudioCmd::Clear);
        let _ = self.audio_tx.send(AudioCmd::SetBpm(start_bpm));
        let _ = self.audio_tx.send(AudioCmd::SetSustain(start_sustain));
        let _ = self.audio_tx.send(AudioCmd::SetVcfBank(start_vcf));
        let _ = self.audio_tx.send(AudioCmd::SetFxSettings(start_fx));
        let _ = self.audio_tx.send(AudioCmd::SetVol(self.vol));
        let _ = self.audio_tx.send(AudioCmd::SetPaused(false));
        for p in loaded {
            let _ = self.audio_tx.send(AudioCmd::AddPhrase(p));
        }
        let _ = self.audio_tx.send(AudioCmd::SetCurPhrase(0));
        Ok(())
    }

    fn load_session_v2<'a, I>(&mut self, lines: I) -> Result<(), String>
    where
        I: Iterator<Item = &'a str>,
    {
        crate::tuning::Maqam::reset_to_defaults();
        let mut new_bpm = 120.0f64;
        let mut new_sustain = 1.25f64;
        let mut new_vcf = VcfBank::default();
        let mut new_fx = FxSettings::default();
        let new_vol = self.vol;
        let mut loaded: Vec<Phrase> = Vec::new();
        let mut next_id = 0usize;
        let mut last_rhythm = vec![3, 3, 2];

        for (line_idx, raw_line) in lines.enumerate() {
            let line_no = line_idx + 2;
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let cmd = command::parse(line).map_err(|e| format!("line {line_no}: {e}"))?;
            match cmd {
                Cmd::SetBpm(change) => {
                    new_bpm = apply_bpm_change(new_bpm, change)
                        .map_err(|e| format!("line {line_no}: {e}"))?;
                    let entry = build_control_entry(
                        next_id,
                        format!("bpm {new_bpm}"),
                        ControlSpec::SetBpm(new_bpm),
                    );
                    next_id += 1;
                    loaded.push(entry);
                }
                Cmd::SetSustain(change) => {
                    new_sustain = apply_sustain_change(new_sustain, change)
                        .map_err(|e| format!("line {line_no}: {e}"))?;
                    let entry = build_control_entry(
                        next_id,
                        format!("s {new_sustain}"),
                        ControlSpec::SetSustain(new_sustain),
                    );
                    next_id += 1;
                    loaded.push(entry);
                }
                Cmd::SetVcf(change) => {
                    let setting = command::apply_vcf_change(new_vcf, change)
                        .map_err(|e| format!("line {line_no}: {e}"))?;
                    new_vcf.apply(setting);
                    let entry = build_control_entry(
                        next_id,
                        vcf_change_src(change),
                        ControlSpec::SetVcf(change),
                    );
                    next_id += 1;
                    loaded.push(entry);
                }
                Cmd::SetFx(change) => {
                    new_fx = command::apply_fx_change(new_fx, change)
                        .map_err(|e| format!("line {line_no}: {e}"))?;
                    let entry = build_control_entry(
                        next_id,
                        fx_change_src(change),
                        ControlSpec::SetFx(change),
                    );
                    next_id += 1;
                    loaded.push(entry);
                }
                Cmd::SetVol(_) => {}
                Cmd::AddPhrase {
                    source,
                    specs,
                    repeat,
                } => {
                    let resolved = resolve_rhythms(specs, &last_rhythm)
                        .map_err(|e| format!("line {line_no}: {e}"))?;
                    if let Some(r) = resolved.last() {
                        last_rhythm = r.groups.clone();
                    }
                    let phrase = build_phrase(next_id, source, resolved, 4, repeat.max(1));
                    next_id += 1;
                    loaded.push(phrase);
                }
                Cmd::Jump { to, times } => {
                    if to < 0 {
                        return Err(format!("line {line_no}: negative ids are only supported in interactive commands"));
                    }
                    let target = to as usize;
                    let entry = crate::sequencer::build_jump_entry(next_id, target, times.max(1));
                    next_id += 1;
                    loaded.push(entry);
                }
                Cmd::Clear => {
                    loaded.clear();
                    next_id = 0;
                    last_rhythm = vec![3, 3, 2];
                }
                Cmd::CreateJins { name, ratios } => {
                    crate::tuning::Maqam::create(&name, ratios)
                        .map_err(|e| format!("line {line_no}: {e}"))?;
                }
                Cmd::DeleteJins { name } => {
                    let _ = crate::tuning::Maqam::delete(&name);
                }
                _ => {
                    return Err(format!("line {line_no}: unsupported command in session"));
                }
            }
        }

        self.phrases = loaded.clone();
        self.next_phrase_id = next_id;
        self.last_rhythm = last_rhythm;
        self.bpm = new_bpm;
        self.sustain = new_sustain;
        self.vcf = new_vcf;
        self.fx = new_fx;
        self.vol = new_vol;
        self.paused = false;
        let (start_bpm, start_sustain, start_vcf, start_fx) = self.sequence_start_settings();

        let _ = self.audio_tx.send(AudioCmd::Clear);
        let _ = self.audio_tx.send(AudioCmd::SetBpm(start_bpm));
        let _ = self.audio_tx.send(AudioCmd::SetSustain(start_sustain));
        let _ = self.audio_tx.send(AudioCmd::SetVcfBank(start_vcf));
        let _ = self.audio_tx.send(AudioCmd::SetFxSettings(start_fx));
        let _ = self.audio_tx.send(AudioCmd::SetVol(self.vol));
        let _ = self.audio_tx.send(AudioCmd::SetPaused(false));
        for p in loaded {
            let _ = self.audio_tx.send(AudioCmd::AddPhrase(p));
        }
        let _ = self.audio_tx.send(AudioCmd::SetCurPhrase(0));
        Ok(())
    }
}

fn rhythm_groups_display(groups: &[u8]) -> String {
    groups
        .iter()
        .map(|group| group.to_string())
        .collect::<Vec<_>>()
        .join("")
}

enum LlmOutcome {
    Answer {
        sent_prompt: String,
        answer: String,
    },
    Edit {
        sent_prompt: String,
        commands: Vec<String>,
    },
}

#[derive(Clone, Debug)]
struct LlmChatMessage {
    role: LlmRole,
    content: String,
}

#[derive(Clone, Copy, Debug)]
enum LlmRole {
    User,
    Assistant,
}

struct ScoreSnapshot {
    phrases: Vec<Phrase>,
    next_phrase_id: usize,
    last_rhythm: Vec<u8>,
    bpm: f64,
    sustain: f64,
    vcf: VcfBank,
    fx: FxSettings,
    paused: bool,
}

enum LlmRequest {
    ChatGpt {
        key: String,
        model: String,
        prompt: String,
        history: Vec<LlmChatMessage>,
        score_context: String,
    },
    Claude {
        key: String,
        model: String,
        prompt: String,
        history: Vec<LlmChatMessage>,
        score_context: String,
    },
}

impl LlmRequest {
    fn from_env(
        provider: LlmProvider,
        prompt: String,
        history: Vec<LlmChatMessage>,
        score_context: String,
    ) -> Option<Self> {
        match provider {
            LlmProvider::ChatGpt => Some(Self::ChatGpt {
                key: std::env::var("OPENAI_API_KEY").ok()?,
                model: std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into()),
                prompt,
                history,
                score_context,
            }),
            LlmProvider::Claude => Some(Self::Claude {
                key: std::env::var("ANTHROPIC_API_KEY")
                    .or_else(|_| std::env::var("CLAUDE_API_KEY"))
                    .ok()?,
                model: std::env::var("ANTHROPIC_MODEL")
                    .unwrap_or_else(|_| "claude-3-5-haiku-latest".into()),
                prompt,
                history,
                score_context,
            }),
        }
    }

    fn provider_name(&self) -> &'static str {
        match self {
            Self::ChatGpt { .. } => "chatgpt",
            Self::Claude { .. } => "claude",
        }
    }

    fn send(self) -> Result<String, String> {
        match self {
            Self::ChatGpt {
                key,
                model,
                prompt,
                history,
                score_context,
            } => ask_chatgpt(&key, &model, &prompt, &history, &score_context),
            Self::Claude {
                key,
                model,
                prompt,
                history,
                score_context,
            } => ask_claude(&key, &model, &prompt, &history, &score_context),
        }
    }

    fn send_edit(self) -> Result<Vec<String>, String> {
        match self {
            Self::ChatGpt {
                key,
                model,
                prompt,
                history,
                score_context,
            } => {
                let prompt = prompt_with_automatic_nam_research(&prompt)?;
                ask_chatgpt_edit(&key, &model, &prompt, &history, &score_context)
            }
            Self::Claude {
                key,
                model,
                prompt,
                history,
                score_context,
            } => {
                let prompt = prompt_with_automatic_nam_research(&prompt)?;
                ask_claude_edit(&key, &model, &prompt, &history, &score_context)
            }
        }
    }
}

fn ask_chatgpt(
    key: &str,
    model: &str,
    prompt: &str,
    history: &[LlmChatMessage],
    score_context: &str,
) -> Result<String, String> {
    let prompt_for_model = prompt_with_automatic_nam_research(prompt)?;
    let mut messages = openai_messages(history, &prompt_for_model, score_context);
    let response: serde_json::Value = ureq::post("https://api.openai.com/v1/chat/completions")
        .set("Authorization", &format!("Bearer {key}"))
        .set("Content-Type", "application/json")
        .timeout(std::time::Duration::from_secs(30))
        .send_json(serde_json::json!({
            "model": model,
            "messages": messages,
            "temperature": 0.2,
            "tools": [openai_find_nam_captures_tool()],
            "tool_choice": "auto"
        }))
        .map_err(describe_http_error)?
        .into_json()
        .map_err(|e| format!("chatgpt response parse failed: {e}"))?;
    if let Some(calls) = response
        .pointer("/choices/0/message/tool_calls")
        .and_then(|value| value.as_array())
        .filter(|calls| !calls.is_empty())
    {
        let assistant_message =
            response
                .pointer("/choices/0/message")
                .cloned()
                .ok_or_else(|| {
                    "chatgpt tool response had no assistant message; ask again".to_string()
                })?;
        messages.push(assistant_message);
        for call in calls {
            let id = call
                .get("id")
                .and_then(|value| value.as_str())
                .ok_or_else(|| "chatgpt tool call had no id; ask again".to_string())?;
            let result = execute_openai_llm_tool(call)?;
            messages.push(serde_json::json!({
                "role": "tool",
                "tool_call_id": id,
                "content": result
            }));
        }
        let response: serde_json::Value = ureq::post("https://api.openai.com/v1/chat/completions")
            .set("Authorization", &format!("Bearer {key}"))
            .set("Content-Type", "application/json")
            .timeout(std::time::Duration::from_secs(30))
            .send_json(serde_json::json!({
                "model": model,
                "messages": messages,
                "temperature": 0.2
            }))
            .map_err(describe_http_error)?
            .into_json()
            .map_err(|e| format!("chatgpt response parse failed after NAM search: {e}"))?;
        return response
            .pointer("/choices/0/message/content")
            .and_then(|value| value.as_str())
            .map(clean_llm_answer)
            .filter(|answer| !answer.is_empty())
            .ok_or_else(|| {
                "chatgpt returned no message after NAM search; try again, or paste a direct .nam URL and run `nam import URL as name`".into()
            });
    }
    response
        .pointer("/choices/0/message/content")
        .and_then(|value| value.as_str())
        .map(clean_llm_answer)
        .filter(|answer| !answer.is_empty())
        .ok_or_else(|| {
            "chatgpt returned no message content; try asking again, or set OPENAI_MODEL to a chat-capable model".into()
        })
}

fn ask_chatgpt_edit(
    key: &str,
    model: &str,
    prompt: &str,
    history: &[LlmChatMessage],
    score_context: &str,
) -> Result<Vec<String>, String> {
    let messages = openai_messages(history, prompt, score_context);
    let response: serde_json::Value = ureq::post("https://api.openai.com/v1/chat/completions")
        .set("Authorization", &format!("Bearer {key}"))
        .set("Content-Type", "application/json")
        .timeout(std::time::Duration::from_secs(30))
        .send_json(serde_json::json!({
            "model": model,
            "messages": messages,
            "temperature": 0.2,
            "tools": [openai_apply_commands_tool()],
            "tool_choice": {
                "type": "function",
                "function": { "name": "apply_maqam_commands" }
            }
        }))
        .map_err(describe_http_error)?
        .into_json()
        .map_err(|e| format!("chatgpt tool-call response parse failed: {e}"))?;

    let calls = response
        .pointer("/choices/0/message/tool_calls")
        .and_then(|value| value.as_array())
        .ok_or_else(|| {
            "chatgpt did not call apply_maqam_commands; ask again, or set OPENAI_MODEL to a model that supports tool calling".to_string()
        })?;
    let arguments = calls
        .iter()
        .find(|call| {
            call.pointer("/function/name").and_then(|value| value.as_str())
                == Some("apply_maqam_commands")
        })
        .and_then(|call| call.pointer("/function/arguments"))
        .and_then(|value| value.as_str())
        .ok_or_else(|| {
            "chatgpt called the edit tool without command arguments; ask again with the edit request".to_string()
        })?;
    let input = serde_json::from_str::<serde_json::Value>(arguments)
        .map_err(|e| format!("chatgpt edit tool arguments were not valid JSON: {e}; ask again"))?;
    extract_tool_commands(&input)
}

fn ask_claude(
    key: &str,
    model: &str,
    prompt: &str,
    history: &[LlmChatMessage],
    score_context: &str,
) -> Result<String, String> {
    let prompt_for_model = prompt_with_automatic_nam_research(prompt)?;
    let mut messages = anthropic_messages(history, &prompt_for_model);
    let response: serde_json::Value = ureq::post("https://api.anthropic.com/v1/messages")
        .set("x-api-key", key)
        .set("anthropic-version", "2023-06-01")
        .set("Content-Type", "application/json")
        .timeout(std::time::Duration::from_secs(30))
        .send_json(serde_json::json!({
            "model": model,
            "max_tokens": 500,
            "system": llm_system_prompt(score_context),
            "messages": messages,
            "tools": [anthropic_find_nam_captures_tool()]
        }))
        .map_err(describe_http_error)?
        .into_json()
        .map_err(|e| format!("claude response parse failed: {e}"))?;
    let content = response
        .get("content")
        .and_then(|value| value.as_array())
        .ok_or_else(|| {
            "claude returned no content; try asking again, or set ANTHROPIC_MODEL to a messages-capable model".to_string()
        })?;
    let tool_uses = content
        .iter()
        .filter(|item| {
            item.get("type").and_then(|value| value.as_str()) == Some("tool_use")
                && item.get("name").and_then(|value| value.as_str()) == Some("find_nam_captures")
        })
        .collect::<Vec<_>>();
    if !tool_uses.is_empty() {
        messages.push(serde_json::json!({
            "role": "assistant",
            "content": content
        }));
        let mut tool_results = Vec::new();
        for tool_use in tool_uses {
            let id = tool_use
                .get("id")
                .and_then(|value| value.as_str())
                .ok_or_else(|| "claude NAM search tool call had no id; ask again".to_string())?;
            let result = execute_anthropic_llm_tool(tool_use)?;
            tool_results.push(serde_json::json!({
                "type": "tool_result",
                "tool_use_id": id,
                "content": result
            }));
        }
        messages.push(serde_json::json!({
            "role": "user",
            "content": tool_results
        }));
        let response: serde_json::Value = ureq::post("https://api.anthropic.com/v1/messages")
            .set("x-api-key", key)
            .set("anthropic-version", "2023-06-01")
            .set("Content-Type", "application/json")
            .timeout(std::time::Duration::from_secs(30))
            .send_json(serde_json::json!({
                "model": model,
                "max_tokens": 500,
                "system": llm_system_prompt(score_context),
                "messages": messages
            }))
            .map_err(describe_http_error)?
            .into_json()
            .map_err(|e| format!("claude response parse failed after NAM search: {e}"))?;
        return response
            .get("content")
            .and_then(|value| value.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("text").and_then(|text| text.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .map(|answer| clean_llm_answer(&answer))
            .filter(|answer| !answer.is_empty())
            .ok_or_else(|| {
                "claude returned no text after NAM search; try again, or paste a direct .nam URL and run `nam import URL as name`".into()
            });
    }
    response
        .get("content")
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("text").and_then(|text| text.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .map(|answer| clean_llm_answer(&answer))
        .filter(|answer| !answer.is_empty())
        .ok_or_else(|| {
            "claude returned no text content; try asking again, or set ANTHROPIC_MODEL to a messages-capable model".into()
        })
}

fn ask_claude_edit(
    key: &str,
    model: &str,
    prompt: &str,
    history: &[LlmChatMessage],
    score_context: &str,
) -> Result<Vec<String>, String> {
    let messages = anthropic_messages(history, prompt);
    let response: serde_json::Value = ureq::post("https://api.anthropic.com/v1/messages")
        .set("x-api-key", key)
        .set("anthropic-version", "2023-06-01")
        .set("Content-Type", "application/json")
        .timeout(std::time::Duration::from_secs(30))
        .send_json(serde_json::json!({
            "model": model,
            "max_tokens": 500,
            "system": llm_system_prompt(score_context),
            "messages": messages,
            "tools": [anthropic_apply_commands_tool()],
            "tool_choice": {
                "type": "tool",
                "name": "apply_maqam_commands"
            }
        }))
        .map_err(describe_http_error)?
        .into_json()
        .map_err(|e| format!("claude tool-call response parse failed: {e}"))?;

    let content = response
        .get("content")
        .and_then(|value| value.as_array())
        .ok_or_else(|| {
            "claude returned no content; ask again, or set ANTHROPIC_MODEL to a model that supports tool calling".to_string()
        })?;
    let input = content
        .iter()
        .find(|item| {
            item.get("type").and_then(|value| value.as_str()) == Some("tool_use")
                && item.get("name").and_then(|value| value.as_str())
                    == Some("apply_maqam_commands")
        })
        .and_then(|item| item.get("input"))
        .ok_or_else(|| {
            "claude did not call apply_maqam_commands; ask again, or set ANTHROPIC_MODEL to a model that supports tool calling".to_string()
        })?;
    extract_tool_commands(input)
}

fn describe_http_error(error: ureq::Error) -> String {
    match error {
        ureq::Error::Status(code, response) => {
            let body = response.into_string().unwrap_or_default();
            format!(
                "LLM HTTP {code}: {}; check the API key, model name, and account access, then try again",
                compact_error_body(&body)
            )
        }
        ureq::Error::Transport(error) => {
            format!("LLM request failed: {error}; check your network connection and try again")
        }
    }
}

fn compact_error_body(body: &str) -> String {
    let message = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .or_else(|| value.pointer("/error/error/message"))
                .and_then(|message| message.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| body.trim().to_string());
    clean_llm_answer(&message)
}

fn clean_llm_answer(answer: &str) -> String {
    answer
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn openai_messages(
    history: &[LlmChatMessage],
    prompt: &str,
    score_context: &str,
) -> Vec<serde_json::Value> {
    let mut messages = vec![serde_json::json!({
        "role": "system",
        "content": llm_system_prompt(score_context)
    })];
    for message in history {
        messages.push(serde_json::json!({
            "role": message.role.as_api_role(),
            "content": message.content
        }));
    }
    messages.push(serde_json::json!({
        "role": "user",
        "content": prompt
    }));
    messages
}

fn anthropic_messages(history: &[LlmChatMessage], prompt: &str) -> Vec<serde_json::Value> {
    let mut messages = Vec::new();
    for message in history {
        messages.push(serde_json::json!({
            "role": message.role.as_api_role(),
            "content": message.content
        }));
    }
    messages.push(serde_json::json!({
        "role": "user",
        "content": prompt
    }));
    messages
}

impl LlmRole {
    fn as_api_role(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

fn openai_apply_commands_tool() -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": "apply_maqam_commands",
            "description": "Apply only valid maqam-live score-edit commands from the command language in the system prompt. Use bpm 180, never set tempo 180. Do not include save, load, record, playback, clock, help, audition, or clear commands.",
            "parameters": apply_commands_tool_input_schema()
        }
    })
}

fn anthropic_apply_commands_tool() -> serde_json::Value {
    serde_json::json!({
        "name": "apply_maqam_commands",
        "description": "Apply only valid maqam-live score-edit commands from the command language in the system prompt. Use bpm 180, never set tempo 180. Do not include save, load, record, playback, clock, help, audition, or clear commands.",
        "input_schema": apply_commands_tool_input_schema()
    })
}

fn openai_find_nam_captures_tool() -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": "find_nam_captures",
            "description": "Search the web for real Neural Amp Modeler (.nam) capture pages or direct .nam download links. Use this when the user asks for an amp model, NAM capture, or a style such as Metallica. Return only real links from the tool result; never invent local paths or URLs.",
            "parameters": find_nam_captures_tool_input_schema()
        }
    })
}

fn anthropic_find_nam_captures_tool() -> serde_json::Value {
    serde_json::json!({
        "name": "find_nam_captures",
        "description": "Search the web for real Neural Amp Modeler (.nam) capture pages or direct .nam download links. Use this when the user asks for an amp model, NAM capture, or a style such as Metallica. Return only real links from the tool result; never invent local paths or URLs.",
        "input_schema": find_nam_captures_tool_input_schema()
    })
}

fn find_nam_captures_tool_input_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "query": {
                "type": "string",
                "description": "Amp, artist, tone, or capture search terms, for example `Metallica Mesa Mark IIC+ NAM`."
            }
        },
        "required": ["query"]
    })
}

fn apply_commands_tool_input_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "commands": {
                "type": "array",
                "description": "maqam-live commands to apply. Commands may also contain semicolon-separated maqam-live commands. If returning multiple commands, separate them with array items, semicolons, or newlines; never concatenate commands like `sym on sym decay 0.999 drive 2`.",
                "items": { "type": "string" },
                "minItems": 1
            }
        },
        "required": ["commands"]
    })
}

fn extract_tool_commands(input: &serde_json::Value) -> Result<Vec<String>, String> {
    let commands = input
        .get("commands")
        .and_then(|value| value.as_array())
        .ok_or_else(|| {
            "the edit tool needs a commands array; ask again with the edit request".to_string()
        })?
        .iter()
        .filter_map(|value| value.as_str())
        .flat_map(split_tool_command)
        .collect::<Vec<_>>();
    if commands.is_empty() {
        return Err(
            "the edit tool returned no commands; ask it to return maqam-live commands".into(),
        );
    }
    Ok(commands)
}

fn execute_openai_llm_tool(call: &serde_json::Value) -> Result<String, String> {
    let name = call
        .pointer("/function/name")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "chatgpt tool call had no function name; ask again".to_string())?;
    let arguments = call
        .pointer("/function/arguments")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "chatgpt tool call had no arguments; ask again".to_string())?;
    let input = serde_json::from_str::<serde_json::Value>(arguments)
        .map_err(|err| format!("chatgpt tool arguments were not valid JSON: {err}; ask again"))?;
    execute_llm_tool(name, &input)
}

fn execute_anthropic_llm_tool(tool_use: &serde_json::Value) -> Result<String, String> {
    let name = tool_use
        .get("name")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "claude tool call had no name; ask again".to_string())?;
    let input = tool_use
        .get("input")
        .ok_or_else(|| "claude tool call had no input; ask again".to_string())?;
    execute_llm_tool(name, input)
}

fn execute_llm_tool(name: &str, input: &serde_json::Value) -> Result<String, String> {
    match name {
        "find_nam_captures" => {
            let query = input
                .get("query")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    "find_nam_captures needs a non-empty query; ask for a specific amp, artist, or tone"
                        .to_string()
                })?;
            find_nam_captures(query)
        }
        other => Err(format!(
            "unknown LLM tool `{other}`; ask again using maqam-live tools only"
        )),
    }
}

fn prompt_with_automatic_nam_research(prompt: &str) -> Result<String, String> {
    if !llm_prompt_needs_nam_research(prompt) {
        return Ok(prompt.to_string());
    }
    let results = find_nam_captures(prompt)?;
    Ok(format!(
        "{prompt}\n\nmaqam-live already performed NAM capture research for this request:\n{results}\n\nUse these researched links directly. Do not tell the user to go research amp models. If there is a direct .nam URL, give the exact `nam import URL as name` command. If there are only result pages, give the best result links and explain that maqam-live could not extract a direct .nam URL from those pages."
    ))
}

fn llm_prompt_needs_nam_research(prompt: &str) -> bool {
    let lower = prompt.to_ascii_lowercase();
    let mentions_nam_or_amp = lower.contains(".nam")
        || lower.contains(" nam ")
        || lower.starts_with("nam ")
        || lower.contains("neural amp")
        || lower.contains("amp model")
        || lower.contains("amp capture");
    let asks_for_discovery = [
        "find",
        "search",
        "browse",
        "download",
        "add",
        "use",
        "common",
        "get me",
        "look up",
        "lookup",
        "where can i get",
        "where do i get",
        "metallica",
        "mesa",
        "marshall",
        "5150",
        "soldano",
        "rectifier",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    mentions_nam_or_amp && asks_for_discovery
}

fn find_nam_captures(query: &str) -> Result<String, String> {
    let search_query = format!("{query} Neural Amp Modeler NAM capture download");
    let url = format!(
        "https://duckduckgo.com/html/?q={}",
        url_query_encode(&search_query)
    );
    let body = ureq::get(&url)
        .set("User-Agent", "maqam-live/1.1")
        .timeout(std::time::Duration::from_secs(20))
        .call()
        .map_err(|err| describe_nam_search_error(err, query))?
        .into_string()
        .map_err(|err| {
            format!("NAM search response could not be read: {err}; paste a direct .nam URL and run `nam import URL as name`")
        })?;
    let mut results = extract_search_links(&body, 8);
    let mut direct_links = results
        .iter()
        .filter(|result| result.url.to_ascii_lowercase().contains(".nam"))
        .cloned()
        .collect::<Vec<_>>();
    for result in results.iter().take(4) {
        if direct_links.len() >= 8 || result.url.to_ascii_lowercase().contains(".nam") {
            continue;
        }
        if let Ok(page_links) = fetch_direct_nam_links_from_page(&result.url, 4) {
            for link in page_links {
                if !direct_links.iter().any(|existing| existing.url == link.url) {
                    direct_links.push(link);
                }
            }
        }
    }
    if !direct_links.is_empty() {
        results.splice(0..0, direct_links);
    }
    if results.is_empty() {
        return Ok(format!(
            "No NAM capture links were found for `{query}`. Try a more specific query such as `Mesa Mark IIC+ NAM`, `5150 NAM`, or paste a direct .nam URL and run `nam import URL as name`."
        ));
    }
    let mut lines = vec![
        format!("Real search results for `{query}`:"),
        "Use a direct .nam URL with `nam import URL as name`; if a result is a page, open it and copy the .nam download URL.".to_string(),
    ];
    for (idx, result) in results.iter().enumerate() {
        lines.push(format!("{}. {} — {}", idx + 1, result.title, result.url));
    }
    Ok(lines.join("\n"))
}

fn fetch_direct_nam_links_from_page(url: &str, limit: usize) -> Result<Vec<SearchResult>, String> {
    let body = ureq::get(url)
        .set("User-Agent", "maqam-live/1.1")
        .timeout(std::time::Duration::from_secs(10))
        .call()
        .map_err(|err| describe_nam_search_error(err, url))?
        .into_string()
        .map_err(|err| format!("NAM result page could not be read: {err}"))?;
    Ok(extract_direct_nam_links_from_html(&body, url, limit))
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SearchResult {
    title: String,
    url: String,
}

fn extract_search_links(html: &str, limit: usize) -> Vec<SearchResult> {
    let mut results = Vec::new();
    let mut rest = html;
    while let Some(href_start) = rest.find("href=\"") {
        rest = &rest[href_start + "href=\"".len()..];
        let Some(href_end) = rest.find('"') else {
            break;
        };
        let raw_href = &rest[..href_end];
        rest = &rest[href_end + 1..];
        let url = normalize_search_href(raw_href);
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            continue;
        }
        if results
            .iter()
            .any(|result: &SearchResult| result.url == url)
        {
            continue;
        }
        let title = rest
            .find('>')
            .and_then(|start| rest[start + 1..].find("</a>").map(|end| (start, end)))
            .map(|(start, end)| strip_html_tags(&rest[start + 1..start + 1 + end]))
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| url.clone());
        if !looks_like_nam_result(&url, &title) {
            continue;
        }
        results.push(SearchResult { title, url });
        if results.len() >= limit {
            break;
        }
    }
    results
}

fn extract_direct_nam_links_from_html(
    html: &str,
    base_url: &str,
    limit: usize,
) -> Vec<SearchResult> {
    let mut results = Vec::new();
    let mut rest = html;
    while let Some(href_start) = rest.find("href=\"") {
        rest = &rest[href_start + "href=\"".len()..];
        let Some(href_end) = rest.find('"') else {
            break;
        };
        let raw_href = &rest[..href_end];
        rest = &rest[href_end + 1..];
        let Some(url) = resolve_link_url(&html_entity_decode(raw_href), base_url) else {
            continue;
        };
        if !url.to_ascii_lowercase().contains(".nam") {
            continue;
        }
        if results
            .iter()
            .any(|result: &SearchResult| result.url == url)
        {
            continue;
        }
        let title = url
            .rsplit('/')
            .next()
            .map(percent_decode)
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| "NAM download".to_string());
        results.push(SearchResult { title, url });
        if results.len() >= limit {
            break;
        }
    }
    results
}

fn resolve_link_url(href: &str, base_url: &str) -> Option<String> {
    if href.starts_with("https://") || href.starts_with("http://") {
        return Some(href.to_string());
    }
    if href.starts_with("//") {
        return Some(format!("https:{href}"));
    }
    if href.starts_with('/') {
        let (scheme, rest) = base_url.split_once("://")?;
        let host = rest.split('/').next()?;
        return Some(format!("{scheme}://{host}{href}"));
    }
    None
}

fn normalize_search_href(raw_href: &str) -> String {
    let href = html_entity_decode(raw_href);
    if let Some(query) = href
        .strip_prefix("//duckduckgo.com/l/?")
        .or_else(|| href.strip_prefix("https://duckduckgo.com/l/?"))
        .or_else(|| href.strip_prefix("http://duckduckgo.com/l/?"))
        .or_else(|| href.strip_prefix("/l/?"))
    {
        for part in query.split('&') {
            if let Some(value) = part.strip_prefix("uddg=") {
                return percent_decode(value);
            }
        }
    }
    href
}

fn looks_like_nam_result(url: &str, title: &str) -> bool {
    let combined = format!("{url} {title}").to_ascii_lowercase();
    combined.contains(".nam")
        || combined.contains("neural amp modeler")
        || combined.contains("tonehunt")
        || combined.contains("nam capture")
}

fn strip_html_tags(input: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    clean_llm_answer(&html_entity_decode(&out))
}

fn url_query_encode(input: &str) -> String {
    let mut out = String::new();
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else if byte == b' ' {
            out.push('+');
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(value) = u8::from_str_radix(&input[i + 1..i + 3], 16) {
                out.push(value);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn html_entity_decode(input: &str) -> String {
    input
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

fn describe_nam_search_error(error: ureq::Error, query: &str) -> String {
    match error {
        ureq::Error::Status(code, response) => {
            let body = response.into_string().unwrap_or_default();
            format!(
                "NAM search HTTP {code} for `{query}`: {}; try a different query, or paste a direct .nam URL and run `nam import URL as name`",
                compact_error_body(&body)
            )
        }
        ureq::Error::Transport(error) => {
            format!(
                "NAM search failed for `{query}`: {error}; check your network connection, or paste a direct .nam URL and run `nam import URL as name`"
            )
        }
    }
}

fn split_tool_command(command: &str) -> Vec<String> {
    command
        .split(';')
        .map(str::trim)
        .filter(|command| !command.is_empty())
        .flat_map(split_repeated_command_nouns)
        .collect()
}

fn split_repeated_command_nouns(command: &str) -> Vec<String> {
    let tokens = command.split_whitespace().collect::<Vec<_>>();
    if tokens.len() < 3 || !is_sym_command_noun(tokens[0]) {
        return vec![command.to_string()];
    }

    let mut chunks = Vec::new();
    let mut start = 0usize;
    for i in 1..tokens.len() {
        if is_sym_command_noun(tokens[i]) {
            chunks.push(tokens[start..i].join(" "));
            start = i;
        }
    }
    chunks.push(tokens[start..].join(" "));
    chunks
}

fn is_sym_command_noun(token: &str) -> bool {
    matches!(
        token.to_ascii_lowercase().as_str(),
        "sym" | "sympathetics" | "tanbura" | "tambura"
    )
}

fn llm_prompt_is_edit_request(prompt: &str) -> bool {
    let lower = prompt.trim_start().to_ascii_lowercase();
    lower.starts_with("let's ")
        || lower.starts_with("lets ")
        || lower.starts_with("i need ")
        || lower.starts_with("we need ")
        || lower.starts_with("i want ")
        || lower.starts_with("we want ")
        || lower.starts_with("please ")
        || lower.starts_with("can you ")
        || lower.starts_with("make ")
        || lower.starts_with("add ")
        || lower.starts_with("do ")
        || lower.starts_with("put ")
        || lower.starts_with("use ")
        || lower.starts_with("enable ")
        || lower.starts_with("turn on ")
        || lower.starts_with("set ")
        || lower.starts_with("create ")
        || lower.starts_with("change ")
        || lower.starts_with("replace ")
        || lower.starts_with("insert ")
        || lower.starts_with("delete ")
        || llm_prompt_is_how_to_action(&lower)
}

fn llm_prompt_is_how_to_action(lower_prompt: &str) -> bool {
    if lower_prompt.starts_with("what ")
        || lower_prompt.starts_with("why ")
        || lower_prompt.contains("valid value")
        || lower_prompt.contains("valid values")
        || lower_prompt.contains("explain")
    {
        return false;
    }
    let actionish = [
        "how do i get ",
        "how do i add ",
        "how do i use ",
        "how do i enable ",
        "how do i turn on ",
        "how can i get ",
        "how can i add ",
        "how can i use ",
        "how can i enable ",
        "how can i turn on ",
    ];
    actionish
        .iter()
        .any(|prefix| lower_prompt.starts_with(prefix))
}

fn llm_rejected_edit_command_message(command_src: &str, cmd: &Cmd) -> String {
    match cmd {
        Cmd::SetNam(_) => format!(
            "✗ LLM returned `{command_src}`, but NAM is live input state, not a score edit; import a real .nam file with `nam import FILENAME.nam as name`, then run `nam name` yourself"
        ),
        _ => format!(
            "✗ LLM returned `{command_src}`, but LLM edits cannot run save/load/playback/system commands; ask it for score-edit commands only"
        ),
    }
}

fn llm_edit_prompt(user_prompt: &str) -> String {
    format!(
        "User wants this edit:\n{user_prompt}\n\nThis is an action request: make the change with the apply_maqam_commands tool instead of explaining how the user could do it. Use the current score from the system prompt as context. Return only new maqam-live commands to apply the edit, separated by newlines or semicolons. Never concatenate two commands without a separator; use `sym on; sym decay 0.999 drive 2`, not `sym on sym decay 0.999 drive 2`. Do not include existing score lines, ids, markdown, bullets, explanations, or comments. Never return save, load, m/record, q/quit, z/playback, pause, start, clock, help, ls, audition, or clear commands. NAM edits must be portable: use only `nam tone3000 ID as name` or `nam pin DIRECT_URL as name`, never a bare local alias."
    )
}

fn llm_edit_command_allowed(cmd: &Cmd) -> bool {
    if matches!(
        cmd,
        Cmd::SetNam(NamCommand::Pin { .. } | NamCommand::Tone3000 { .. })
    ) {
        return true;
    }
    matches!(
        cmd,
        Cmd::AddPhrase { .. }
            | Cmd::Jump { .. }
            | Cmd::Insert { .. }
            | Cmd::InsertBpm { .. }
            | Cmd::InsertSustain { .. }
            | Cmd::InsertVcf { .. }
            | Cmd::InsertFx { .. }
            | Cmd::InsertSympathetics { .. }
            | Cmd::InsertSympatheticDecay { .. }
            | Cmd::InsertSympatheticGain { .. }
            | Cmd::InsertSympathetic { .. }
            | Cmd::MoveUp(_)
            | Cmd::MoveDown(_)
            | Cmd::Edit { .. }
            | Cmd::EditJump { .. }
            | Cmd::EditBpm { .. }
            | Cmd::EditSustain { .. }
            | Cmd::EditVcf { .. }
            | Cmd::EditFx { .. }
            | Cmd::EditSympathetics { .. }
            | Cmd::EditSympatheticDecay { .. }
            | Cmd::EditSympatheticGain { .. }
            | Cmd::EditSympathetic { .. }
            | Cmd::InsertJump { .. }
            | Cmd::DeleteBars(_)
            | Cmd::Rotate
            | Cmd::Stop
            | Cmd::Sympathetics(_)
            | Cmd::SympatheticDecay(_)
            | Cmd::SympatheticGain(_)
            | Cmd::Sympathetic(_)
            | Cmd::SetBpm(_)
            | Cmd::SetSustain(_)
            | Cmd::SetVcf(_)
            | Cmd::SetFx(_)
            | Cmd::CreateJins { .. }
            | Cmd::DeleteJins { .. }
    )
}

fn minimize_repeated_llm_phrase_commands(
    commands: Vec<String>,
    next_phrase_id: usize,
) -> Vec<String> {
    if commands.len() < 4 {
        return commands;
    }
    let parsed = commands
        .iter()
        .map(|command_src| command::parse(command_src))
        .collect::<Result<Vec<_>, _>>();
    let Ok(parsed) = parsed else {
        return commands;
    };

    let Some(last_non_phrase) = parsed
        .iter()
        .rposition(|cmd| !matches!(cmd, Cmd::AddPhrase { .. }))
    else {
        return minimize_repeated_phrase_run(&commands, next_phrase_id).unwrap_or(commands);
    };
    let run_start = last_non_phrase + 1;
    if run_start >= parsed.len() || parsed.len() - run_start < 4 {
        return commands;
    }
    let first_phrase_id = next_phrase_id
        + parsed[..run_start]
            .iter()
            .filter(|cmd| llm_edit_command_consumes_timeline_id(cmd))
            .count();
    let run = &commands[run_start..];
    let Some(minimized_run) = minimize_repeated_phrase_run(run, first_phrase_id) else {
        return commands;
    };

    let mut out = commands[..run_start].to_vec();
    out.extend(minimized_run);
    out
}

fn minimize_repeated_phrase_run(
    commands: &[String],
    first_phrase_id: usize,
) -> Option<Vec<String>> {
    let phrase_sources = commands
        .iter()
        .map(|command_src| match command::parse(command_src) {
            Ok(Cmd::AddPhrase { source, repeat, .. }) => Some((source, repeat)),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;

    let total = phrase_sources.len();
    for block_len in 1..=total / 2 {
        if total % block_len != 0 {
            continue;
        }
        let repeats = total / block_len;
        if repeats < 2 {
            continue;
        }
        let block = &phrase_sources[..block_len];
        if phrase_sources
            .chunks(block_len)
            .all(|candidate| candidate == block)
        {
            if block_len == 1 {
                let (source, repeat) = &block[0];
                return Some(vec![phrase_source_with_repeat(source, repeat * repeats)]);
            }
            let mut out = block
                .iter()
                .map(|(source, _repeat)| source.clone())
                .collect::<Vec<_>>();
            out.push(format!("j {} {}", first_phrase_id, repeats));
            return Some(out);
        }
    }

    None
}

fn phrase_source_with_repeat(source: &str, repeat: usize) -> String {
    let trimmed = source.trim_end();
    let Some((base, last)) = trimmed.rsplit_once(char::is_whitespace) else {
        return format!("{trimmed} r{}", repeat.max(1));
    };
    let lower = last.to_ascii_lowercase();
    if lower
        .strip_prefix('r')
        .is_some_and(|digits| !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()))
    {
        return format!("{} r{}", base.trim_end(), repeat.max(1));
    }
    format!("{trimmed} r{}", repeat.max(1))
}

fn nam_cache_dir() -> PathBuf {
    if let Ok(path) = std::env::var("MAQAM_NAM_CACHE_DIR") {
        return PathBuf::from(path);
    }
    PathBuf::from(".nam")
}

fn pin_nam_dependency(session_path: &str, name: &str, url: &str) -> Result<(), String> {
    pin_nam_reference(session_path, name, &format!("nam pin {url} as {name}"))
}

fn pin_nam_reference(session_path: &str, name: &str, pinned: &str) -> Result<(), String> {
    let source = fs::read_to_string(session_path).map_err(|err| err.to_string())?;
    let mut replaced = false;
    let mut lines = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim();
        let matches_alias = trimmed == format!("nam {name}")
            || ((trimmed.starts_with("nam pin ") || trimmed.starts_with("nam tone3000 "))
                && trimmed.ends_with(&format!(" as {name}")));
        if matches_alias {
            if !replaced {
                lines.push(pinned.to_string());
                replaced = true;
            }
        } else {
            lines.push(line.to_string());
        }
    }
    if !replaced {
        let insert_at = usize::from(lines.first().is_some_and(|line| line == "MAQAM_SESSION_V3"));
        lines.insert(insert_at, pinned.to_string());
    }
    let mut output = lines.join("\n");
    output.push('\n');
    fs::write(session_path, output).map_err(|err| err.to_string())
}

fn is_http_url(value: &str) -> bool {
    value.starts_with("https://") || value.starts_with("http://")
}

fn resolve_nam_model_path(value: &str) -> Result<PathBuf, String> {
    let direct = PathBuf::from(value);
    if direct.is_file() {
        return cache_nam_model_file(&direct);
    }
    let cache_dir = nam_cache_dir();
    let cached = if value.ends_with(".nam") {
        cache_dir.join(value)
    } else {
        cache_dir.join(format!("{value}.nam"))
    };
    if cached.is_file() {
        return Ok(cached);
    }
    Err(format!(
        "NAM model `{value}` not found as a file or cached capture in ./.nam; run `nam search <amp or tone>` to find one, then `nam import URL as {}` to download into ./.nam, or `nam import FILENAME.nam as {}` for a downloaded file",
        sanitize_nam_cache_name(value),
        sanitize_nam_cache_name(value)
    ))
}

fn cache_nam_model_file(source: &Path) -> Result<PathBuf, String> {
    let cache_dir = nam_cache_dir();
    fs::create_dir_all(&cache_dir).map_err(|err| {
        format!(
            "cannot create NAM cache {}: {err}; create ./.nam or set MAQAM_NAM_CACHE_DIR to a writable directory",
            cache_dir.display()
        )
    })?;
    let cache_name = nam_cache_name_from_path(source);
    let dest = cache_dir.join(format!("{cache_name}.nam"));
    if source == dest {
        return Ok(dest);
    }
    fs::copy(source, &dest).map_err(|err| {
        format!(
            "cannot cache NAM model in {}: {err}; check file permissions on ./.nam or set MAQAM_NAM_CACHE_DIR",
            dest.display()
        )
    })?;
    Ok(dest)
}

fn download_nam_capture(
    url: &str,
    cache_dir: &Path,
    cache_name: &str,
    load_after: bool,
    bearer_token: Option<&str>,
    tx: crossbeam_channel::Sender<Result<NamDownloadEvent, String>>,
) -> Result<(), String> {
    fs::create_dir_all(cache_dir).map_err(|err| {
        format!(
            "cannot create NAM cache {}: {err}; create ./.nam or set MAQAM_NAM_CACHE_DIR to a writable directory",
            cache_dir.display()
        )
    })?;
    let dest = cache_dir.join(format!("{cache_name}.nam"));
    let partial = cache_dir.join(format!("{cache_name}.nam.part"));
    if dest.is_file() {
        let downloaded = dest.metadata().map(|meta| meta.len()).unwrap_or(0);
        let _ = tx.send(Ok(NamDownloadEvent::Progress {
            downloaded,
            total: Some(downloaded),
        }));
        let _ = tx.send(Ok(NamDownloadEvent::Done {
            name: cache_name.to_string(),
            load_after,
            cached: true,
        }));
        return Ok(());
    }

    let resume_from = partial.metadata().map(|meta| meta.len()).unwrap_or(0);
    let mut request = ureq::get(url).timeout(std::time::Duration::from_secs(60 * 60));
    if let Some(token) = bearer_token {
        request = request.set("Authorization", &format!("Bearer {token}"));
    }
    if resume_from > 0 {
        request = request.set("Range", &format!("bytes={resume_from}-"));
    }
    let response = match request.call() {
        Ok(response) => response,
        Err(ureq::Error::Status(416, _)) if resume_from > 0 => {
            let _ = fs::remove_file(&partial);
            return Err(format!(
                "NAM download resume failed because {} is stale for this URL; the partial file was removed, so run the same `nam import` command again to restart from zero",
                partial.display()
            ));
        }
        Err(err) => {
            return Err(format!(
                "NAM download failed from {url}: {}; check the URL, network connection, or download the .nam file manually and run `nam import FILENAME.nam as {cache_name}`",
                describe_http_error(err)
            ));
        }
    };
    let resumed = resume_from > 0 && response.status() == 206;
    let total = if resumed {
        response
            .header("Content-Range")
            .and_then(parse_content_range_total)
            .or_else(|| {
                response
                    .header("Content-Length")
                    .and_then(|value| value.parse::<u64>().ok())
                    .map(|len| len + resume_from)
            })
    } else {
        response
            .header("Content-Length")
            .and_then(|value| value.parse::<u64>().ok())
    };
    let mut reader = response.into_reader();
    let mut file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(resumed)
        .truncate(!resumed)
        .open(&partial)
        .map_err(|err| {
            format!(
                "cannot write NAM download {}: {err}; check file permissions on ./.nam or set MAQAM_NAM_CACHE_DIR",
                partial.display()
            )
        })?;
    let mut downloaded = if resumed { resume_from } else { 0 };
    let _ = tx.send(Ok(NamDownloadEvent::Progress { downloaded, total }));
    let mut buf = [0_u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).map_err(|err| {
            format!(
                "NAM download read failed from {url}: {err}; check your network connection and run the same `nam import` command again to resume from {}",
                partial.display()
            )
        })?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|err| {
            format!(
                "cannot write NAM download {}: {err}; free disk space or check file permissions, then run the same `nam import` command again to resume",
                partial.display()
            )
        })?;
        downloaded += n as u64;
        let _ = tx.send(Ok(NamDownloadEvent::Progress { downloaded, total }));
    }
    file.flush().map_err(|err| {
        format!(
            "cannot finish NAM download {}: {err}; free disk space or check file permissions, then run the same `nam import` command again to resume",
            partial.display()
        )
    })?;
    fs::rename(&partial, &dest).map_err(|err| {
        format!(
            "cannot move NAM download into {}: {err}; check file permissions on ./.nam, then run the same `nam import` command again",
            dest.display()
        )
    })?;
    let _ = tx.send(Ok(NamDownloadEvent::Done {
        name: cache_name.to_string(),
        load_after,
        cached: false,
    }));
    Ok(())
}

fn parse_content_range_total(value: &str) -> Option<u64> {
    let total = value.rsplit('/').next()?.trim();
    if total == "*" {
        None
    } else {
        total.parse().ok()
    }
}

fn tone3000_model_url(tone_id: u64, token: &str) -> Result<String, String> {
    let url = format!(
        "https://www.tone3000.com/api/v1/models?tone_id={tone_id}&page=1&page_size=1&architecture=2"
    );
    let response: serde_json::Value = ureq::get(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/json")
        .timeout(std::time::Duration::from_secs(30))
        .call()
        .map_err(describe_http_error)?
        .into_json()
        .map_err(|err| format!("TONE3000 response could not be read: {err}"))?;
    response
        .pointer("/data/0/model_url")
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .ok_or_else(|| format!("TONE3000 tone {tone_id} has no downloadable A2 NAM model"))
}

fn tone3000_auth_path() -> PathBuf {
    std::env::var_os("MAQAM_TONE3000_AUTH_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".tone3000-auth.json"))
}

fn save_tone3000_auth(auth: &Tone3000Auth) -> Result<(), String> {
    let value = serde_json::json!({
        "access_token": auth.access_token,
        "refresh_token": auth.refresh_token,
        "expires_at": auth.expires_at,
        "client_id": auth.client_id,
    });
    let path = tone3000_auth_path();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        file.write_all(value.to_string().as_bytes())
            .map_err(|e| e.to_string())?;
    }
    #[cfg(not(unix))]
    fs::write(&path, value.to_string()).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(())
}

fn load_tone3000_auth() -> Result<Tone3000Auth, String> {
    let path = tone3000_auth_path();
    let value: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?,
    )
    .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Tone3000Auth {
        access_token: value["access_token"]
            .as_str()
            .ok_or("stored access token is missing")?
            .into(),
        refresh_token: value["refresh_token"].as_str().map(str::to_string),
        expires_at: value["expires_at"].as_u64().unwrap_or(0),
        client_id: value["client_id"]
            .as_str()
            .ok_or("stored client ID is missing")?
            .into(),
    })
}

fn unix_time() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn tone3000_access_token() -> Result<String, String> {
    if let Ok(token) = std::env::var("TONE3000_ACCESS_TOKEN") {
        return Ok(token);
    }
    let mut auth = load_tone3000_auth()?;
    if auth.expires_at > unix_time() + 60 {
        return Ok(auth.access_token);
    }
    let refresh = auth
        .refresh_token
        .clone()
        .ok_or("TONE3000 login expired; run `nam login`")?;
    let response: serde_json::Value = ureq::post("https://www.tone3000.com/api/v1/oauth/token")
        .send_form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", &refresh),
            ("client_id", &auth.client_id),
        ])
        .map_err(describe_http_error)?
        .into_json()
        .map_err(|e| format!("TONE3000 refresh response could not be read: {e}"))?;
    auth.access_token = response["access_token"]
        .as_str()
        .ok_or("TONE3000 refresh returned no access token")?
        .into();
    auth.refresh_token = response["refresh_token"]
        .as_str()
        .map(str::to_string)
        .or(auth.refresh_token);
    auth.expires_at = unix_time() + response["expires_in"].as_u64().unwrap_or(3600);
    save_tone3000_auth(&auth)?;
    Ok(auth.access_token)
}

fn random_base64url(bytes: usize) -> Result<String, String> {
    let mut raw = vec![0u8; bytes];
    getrandom::getrandom(&mut raw).map_err(|e| format!("could not generate OAuth secret: {e}"))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw))
}

fn tone3000_browser_login(client_id: &str) -> Result<Tone3000Auth, String> {
    let listener = TcpListener::bind(("localhost", 0))
        .map_err(|e| format!("could not start local OAuth callback: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let redirect_uri = format!("http://localhost:{port}/callback");
    let verifier = random_base64url(64)?;
    let state = random_base64url(32)?;
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(verifier.as_bytes()));
    let mut auth_url = url::Url::parse("https://www.tone3000.com/api/v1/oauth/authorize")
        .map_err(|e| e.to_string())?;
    auth_url
        .query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &state);
    open_system_browser(auth_url.as_str())?;
    listener.set_nonblocking(false).map_err(|e| e.to_string())?;
    let (mut stream, _) = listener
        .accept()
        .map_err(|e| format!("OAuth callback failed: {e}"))?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .ok();
    let mut request = [0u8; 8192];
    let length = stream
        .read(&mut request)
        .map_err(|e| format!("OAuth callback could not be read: {e}"))?;
    let first_line = String::from_utf8_lossy(&request[..length])
        .lines()
        .next()
        .unwrap_or("")
        .to_string();
    let target = first_line
        .split_whitespace()
        .nth(1)
        .ok_or("invalid OAuth callback")?;
    let callback =
        url::Url::parse(&format!("http://localhost{target}")).map_err(|e| e.to_string())?;
    let params = callback
        .query_pairs()
        .collect::<std::collections::HashMap<_, _>>();
    let returned_state = params.get("state").ok_or("OAuth callback omitted state")?;
    if returned_state.as_ref() != state {
        return Err("OAuth state did not match".into());
    }
    let code = params.get("code").ok_or_else(|| {
        params
            .get("error")
            .map(|e| e.to_string())
            .unwrap_or_else(|| "OAuth callback omitted code".into())
    })?;
    let body = b"TONE3000 login complete. You can return to maqam-live.";
    let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.write_all(body);
    let token: serde_json::Value = ureq::post("https://www.tone3000.com/api/v1/oauth/token")
        .send_form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_ref()),
            ("redirect_uri", &redirect_uri),
            ("client_id", client_id),
            ("code_verifier", &verifier),
        ])
        .map_err(describe_http_error)?
        .into_json()
        .map_err(|e| format!("TONE3000 token response could not be read: {e}"))?;
    Ok(Tone3000Auth {
        access_token: token["access_token"]
            .as_str()
            .ok_or("TONE3000 returned no access token")?
            .into(),
        refresh_token: token["refresh_token"].as_str().map(str::to_string),
        expires_at: unix_time() + token["expires_in"].as_u64().unwrap_or(3600),
        client_id: client_id.into(),
    })
}

fn open_system_browser(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let status = std::process::Command::new("open").arg(url).status();
    #[cfg(target_os = "linux")]
    let status = std::process::Command::new("xdg-open").arg(url).status();
    #[cfg(target_os = "windows")]
    let status = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .status();
    status
        .map_err(|e| format!("could not open browser: {e}"))?
        .success()
        .then_some(())
        .ok_or_else(|| format!("could not open browser; visit {url}"))
}

fn nam_session_audio_cmd(command: NamCommand) -> Result<AudioCmd, String> {
    match command {
        NamCommand::Load { path } => nam_load_audio_cmd(&path).map(|(cmd, _message)| cmd),
        NamCommand::Off => Ok(AudioCmd::SetNamModel(None)),
        NamCommand::Gain(gain) => Ok(AudioCmd::SetNamGain(gain)),
        NamCommand::Input(route) => Ok(AudioCmd::SetNamInput(route)),
        NamCommand::Pin { name, .. } => {
            nam_load_audio_cmd(&name).map(|(cmd, _message)| cmd)
        }
        NamCommand::Tone3000 { name, .. } => {
            nam_load_audio_cmd(&name).map(|(cmd, _message)| cmd)
        }
        NamCommand::Login | NamCommand::Logout | NamCommand::Latency(_) | NamCommand::Import { .. } | NamCommand::Search { .. } | NamCommand::List => Err(
            "NAM lines in .mq files can load, bypass, or set gain only; run import/search/list interactively"
                .into(),
        ),
    }
}

fn nam_command_src(command: &NamCommand) -> Option<String> {
    match command {
        NamCommand::Load { path } => Some(format!("nam {path}")),
        NamCommand::Off => Some("nam off".to_string()),
        NamCommand::Gain(gain) => Some(format!("nam gain {gain}")),
        NamCommand::Input(route) => Some(format!("nam input {}", nam_input_name(*route))),
        NamCommand::Pin { url, name } => Some(format!("nam pin {url} as {name}")),
        NamCommand::Tone3000 { tone_id, name } => Some(format!("nam tone3000 {tone_id} as {name}")),
        NamCommand::Login
        | NamCommand::Logout
        | NamCommand::Latency(_)
        | NamCommand::Import { .. }
        | NamCommand::Search { .. }
        | NamCommand::List => None,
    }
}

fn nam_input_name(route: NamInput) -> &'static str {
    match route {
        NamInput::Left => "left",
        NamInput::Right => "right",
        NamInput::Stereo => "stereo",
    }
}

fn nam_timeline_control(command: &NamCommand) -> Option<ControlSpec> {
    match command {
        NamCommand::Off => Some(ControlSpec::SetNamEnabled(false)),
        NamCommand::Gain(gain) => Some(ControlSpec::SetNamGain(*gain)),
        NamCommand::Input(route) => Some(ControlSpec::SetNamInput(*route)),
        NamCommand::Load { .. } | NamCommand::Pin { .. } | NamCommand::Tone3000 { .. } => {
            Some(ControlSpec::SetNamEnabled(true))
        }
        NamCommand::Login
        | NamCommand::Logout
        | NamCommand::Latency(_)
        | NamCommand::Import { .. }
        | NamCommand::Search { .. }
        | NamCommand::List => None,
    }
}

fn replace_live_nam_command(commands: &mut Vec<String>, src: Option<String>) {
    let Some(src) = src else {
        return;
    };
    let is_gain = src.starts_with("nam gain ");
    let is_state = src == "nam off" || (!is_gain && src.starts_with("nam "));
    if let Some(existing) = commands.iter_mut().find(|command| {
        if is_gain {
            command.starts_with("nam gain ")
        } else if is_state {
            *command == "nam off"
                || (!command.starts_with("nam gain ") && command.starts_with("nam "))
        } else {
            false
        }
    }) {
        *existing = src;
        return;
    }
    commands.push(src);
}

fn mark_nam_loaded() {
    crate::clear_nam_error();
    crate::NAM_MODEL_ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);
    crate::NAM_STATUS.store(1, std::sync::atomic::Ordering::Relaxed);
}

pub fn preferred_nam_sample_rate_for_startup_commands(commands: &[String]) -> Option<u32> {
    commands
        .iter()
        .find_map(|command_src| preferred_nam_sample_rate_for_startup_command(command_src))
}

pub fn preferred_nam_sample_rate_for_cached_models() -> Option<u32> {
    let mut rates = std::collections::BTreeSet::new();
    collect_nam_sample_rates_from_dir(&nam_cache_dir(), &mut rates);
    if rates.len() == 1 {
        rates.into_iter().next()
    } else {
        None
    }
}

fn collect_nam_sample_rates_from_dir(dir: &Path, rates: &mut std::collections::BTreeSet<u32>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("nam") {
            continue;
        }
        if let Ok(model) = nam_rs::NamModel::from_file(&path) {
            rates.insert(model.expected_sample_rate().round() as u32);
        }
    }
}

fn preferred_nam_sample_rate_for_startup_command(command_src: &str) -> Option<u32> {
    if let Some(path) = command_src
        .trim()
        .strip_suffix(".mq")
        .map(|stem| format!("{stem}.mq"))
        .filter(|path| std::path::Path::new(path).is_file())
    {
        return preferred_nam_sample_rate_for_session_file(&path);
    }

    match command::parse(command_src).ok()? {
        Cmd::Load { path } => preferred_nam_sample_rate_for_session_file(&path),
        Cmd::SetNam(command) => preferred_nam_sample_rate_for_nam_command(&command),
        _ => None,
    }
}

fn preferred_nam_sample_rate_for_session_file(path: &str) -> Option<u32> {
    let source = fs::read_to_string(path).ok()?;
    let mut lines = source.lines();
    let header = lines.next()?.trim();
    if header == crate::session_v3::HEADER {
        for line in lines {
            let fields = crate::session_v3::split_escaped_fields(line);
            if fields.first().map(String::as_str) == Some("N") {
                if let Some(rate) = fields
                    .get(2)
                    .and_then(|src| preferred_nam_sample_rate_for_command_src(src))
                {
                    return Some(rate);
                }
            }
        }
    } else {
        for line in source.lines() {
            if let Some(rate) = preferred_nam_sample_rate_for_command_src(line) {
                return Some(rate);
            }
        }
    }
    None
}

fn preferred_nam_sample_rate_for_command_src(src: &str) -> Option<u32> {
    match command::parse(src).ok()? {
        Cmd::SetNam(command) => preferred_nam_sample_rate_for_nam_command(&command),
        _ => None,
    }
}

fn preferred_nam_sample_rate_for_nam_command(command: &NamCommand) -> Option<u32> {
    let value = match command {
        NamCommand::Load { path } => path.as_str(),
        NamCommand::Pin { name, .. } | NamCommand::Tone3000 { name, .. } => name.as_str(),
        NamCommand::Import { path, name } => {
            if let Some(name) = name {
                name.as_str()
            } else {
                path.as_str()
            }
        }
        _ => return None,
    };
    let path = resolve_nam_model_path(value).ok()?;
    nam_rs::NamModel::from_file(&path)
        .ok()
        .map(|model| model.expected_sample_rate().round() as u32)
}

fn nam_load_audio_cmd(value: &str) -> Result<(AudioCmd, String), String> {
    let model_path = resolve_nam_model_path(value)?;
    let model = nam_rs::NamModel::from_file(&model_path).map_err(|err| {
        format!(
            "NAM could not load {}: {err}; use a supported A1/A2 .nam file",
            model_path.display()
        )
    })?;
    let expected_sr = model.expected_sample_rate();
    let output_sr = crate::AUDIO_OUTPUT_SAMPLE_RATE_HZ.load(std::sync::atomic::Ordering::Relaxed);
    if output_sr != 0 && expected_sr.round() as u32 != output_sr {
        return Err(format!(
            "NAM sample-rate mismatch for {}: model expects {:.0} Hz but audio output is {output_sr} Hz; restart maqam-live with `MAQAM_SAMPLE_RATE={:.0} maqam-live` or use a .nam captured at {output_sr} Hz",
            model_path.display(),
            expected_sr,
            expected_sr
        ));
    }
    let mut runtime = nam_rs::Model::from_nam(&model).map_err(|err| {
        format!(
            "NAM could not load {}: {err}; use a supported A1/A2 .nam file",
            model_path.display()
        )
    })?;
    let slim_note = configure_nam_slim_size(&mut runtime);
    Ok((
        AudioCmd::SetNamModel(Some(runtime)),
        if output_sr == 0 {
            format!(
                "NAM input amp loaded ← {} ({:.0} Hz model; audio output unavailable{slim_note})",
                model_path.display(),
                expected_sr
            )
        } else {
            format!(
                "NAM input amp loaded ← {} ({:.0} Hz model; audio is running at {output_sr} Hz{slim_note})",
                model_path.display(),
                expected_sr
            )
        },
    ))
}

fn configure_nam_slim_size(runtime: &mut nam_rs::Model) -> String {
    let Some(slimmable) = runtime.as_slimmable_mut() else {
        return String::new();
    };
    let value = std::env::var("MAQAM_NAM_SLIM")
        .ok()
        .and_then(|value| value.parse::<f32>().ok())
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    slimmable.set_slim_size(value);
    format!(
        "; slim {:.2} -> submodel {}",
        value,
        slimmable.active_index()
    )
}

fn list_cached_nam_models(cache_dir: &Path) -> Result<Vec<String>, String> {
    if !cache_dir.exists() {
        fs::create_dir_all(cache_dir).map_err(|err| err.to_string())?;
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    for entry in fs::read_dir(cache_dir).map_err(|err| err.to_string())? {
        let entry = entry.map_err(|err| err.to_string())?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("nam") {
            continue;
        }
        if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
            names.push(stem.to_string());
        }
    }
    names.sort();
    Ok(names)
}

fn list_current_dir_nam_files() -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    for entry in fs::read_dir(".").map_err(|err| err.to_string())? {
        let entry = entry.map_err(|err| err.to_string())?;
        let path = entry.path();
        if !path.is_file() || path.extension().and_then(|ext| ext.to_str()) != Some("nam") {
            continue;
        }
        if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
            names.push(name.to_string());
        }
    }
    names.sort();
    Ok(names)
}

fn nam_cache_name_from_path(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(sanitize_nam_cache_name)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "model".into())
}

fn nam_cache_name_from_url(url: &str) -> String {
    let without_fragment = url.split('#').next().unwrap_or(url);
    let without_query = without_fragment
        .split('?')
        .next()
        .unwrap_or(without_fragment)
        .trim_end_matches('/');
    let filename = without_query.rsplit('/').next().unwrap_or(without_query);
    let stem = filename.strip_suffix(".nam").unwrap_or(filename);
    sanitize_nam_cache_name(stem)
}

fn sanitize_nam_cache_name(name: &str) -> String {
    let mut out = String::new();
    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            out.push(ch);
        } else if ch.is_whitespace() {
            out.push('-');
        }
    }
    out.trim_matches('.').trim_matches('-').to_string()
}

fn llm_edit_command_consumes_timeline_id(cmd: &Cmd) -> bool {
    matches!(
        cmd,
        Cmd::AddPhrase { .. }
            | Cmd::Jump { .. }
            | Cmd::Insert { .. }
            | Cmd::InsertBpm { .. }
            | Cmd::InsertSustain { .. }
            | Cmd::InsertVcf { .. }
            | Cmd::InsertFx { .. }
            | Cmd::InsertSympathetics { .. }
            | Cmd::InsertSympatheticDecay { .. }
            | Cmd::InsertSympatheticGain { .. }
            | Cmd::InsertSympathetic { .. }
            | Cmd::InsertJump { .. }
            | Cmd::Stop
            | Cmd::Sympathetics(_)
            | Cmd::SympatheticDecay(_)
            | Cmd::SympatheticGain(_)
            | Cmd::Sympathetic(_)
            | Cmd::SetBpm(_)
            | Cmd::SetSustain(_)
            | Cmd::SetVcf(_)
            | Cmd::SetFx(_)
    )
}

fn llm_system_prompt(score_context: &str) -> String {
    format!(
        "You help with maqam-live, a terminal live-coding sequencer.\n\n{}\nCurrent score context:\n{}\n\nLLM behavior:\n- The request includes the current score plus prior user prompts and prior assistant answers/tool-command results; use that continuity.\n- For questions, answer concisely and prefer exact commands.\n- When the user asks to find, browse, or download a NAM capture or amp model, use the find_nam_captures tool; never claim you cannot browse, and never invent URLs or fake local paths.\n- For direct .nam URLs found by the tool, suggest `nam import URL as name` or `nam URL`; for result pages, tell the user to open the page and copy the .nam download URL.\n- For edit requests, use the apply_maqam_commands tool with valid commands only.\n- Never return save, load, m/record, q/quit, z/playback, pause, start, clock, help, ls, audition, clear, vol, tuneto, or nam commands.",
        command::language_reference(),
        score_context
    )
}

fn completion_target(input: &str) -> Option<(&str, usize, String)> {
    let trimmed = input.trim_start();
    let leading_ws = input.len().saturating_sub(trimmed.len());
    let (cmd, rest) = trimmed
        .split_once(char::is_whitespace)
        .unwrap_or((trimmed, ""));
    if cmd != "save" && cmd != "load" {
        return None;
    }
    let rest_start = leading_ws + cmd.len();
    let arg = rest.trim_start();
    let arg_start = input.len().saturating_sub(arg.len());
    Some((cmd, arg_start.max(rest_start), arg.to_string()))
}

fn phrase_completion(input: &str, phrases: &[Phrase]) -> Option<String> {
    let trimmed = input.trim_start();
    let leading_ws = input.len().saturating_sub(trimmed.len());
    let words = words_with_spans(trimmed);
    if words.len() != 1 {
        return None;
    }
    let root_token = words[0].2;
    let typed_root = crate::tuning::Pitch::parse(root_token)?;
    let current = phrases
        .iter()
        .rev()
        .find(|phrase| phrase.jump.is_none() && phrase.control.is_none())?;
    let maqam = phrase_completion_maqam(current, typed_root)?;
    let rhythm = phrase_rhythm_token(current)?;
    let mut completion = format!(
        "{}{} {} {}",
        " ".repeat(leading_ws),
        root_token,
        maqam,
        rhythm
    );
    if current.repeat > 1 {
        completion.push_str(&format!(" r{}", current.repeat));
    }
    Some(completion)
}

fn phrase_rhythm_token(phrase: &Phrase) -> Option<&str> {
    phrase
        .src
        .split_whitespace()
        .rev()
        .find(|token| token.chars().all(|ch| ch.is_ascii_digit()))
}

fn phrase_completion_maqam(
    current: &Phrase,
    typed_root: crate::tuning::Pitch,
) -> Option<&'static str> {
    let current_name = current.bar.maqam.name();
    let shift = pitch_class_delta(current.bar.root, typed_root);
    match (current_name, shift) {
        ("Bayati", 10) => Some("rast"),
        ("Minor" | "Aeolian", 3) => Some("major"),
        _ => None,
    }
}

fn pitch_class_delta(from: crate::tuning::Pitch, to: crate::tuning::Pitch) -> i8 {
    (pitch_class(to) - pitch_class(from)).rem_euclid(12)
}

fn pitch_class(pitch: crate::tuning::Pitch) -> i8 {
    let natural = match pitch.letter.to_ascii_lowercase() {
        'c' => 0,
        'd' => 2,
        'e' => 4,
        'f' => 5,
        'g' => 7,
        'a' => 9,
        'b' => 11,
        _ => 0,
    };
    (natural + pitch.accidental).rem_euclid(12)
}

struct MetadataCompletion {
    replacement: Option<String>,
    message: Option<String>,
}

fn command_body_for_completion(input: &str) -> Option<(usize, &str)> {
    let trimmed = input.trim_start();
    let leading_ws = input.len().saturating_sub(trimmed.len());
    let words = words_with_spans(trimmed);
    let first = words.first()?;
    let first_text = first.2;
    let first_alpha: String = first_text
        .chars()
        .take_while(|ch| ch.is_ascii_alphabetic())
        .collect();
    let first_digits: String = first_text
        .chars()
        .skip_while(|ch| ch.is_ascii_alphabetic())
        .collect();
    let first_lower = first_alpha.to_ascii_lowercase();

    if first_lower == "edit" {
        let id = words.get(1)?;
        let body_start = words
            .get(2)
            .map(|word| word.0)
            .unwrap_or_else(|| trimmed.len());
        if id.2.parse::<isize>().is_err() {
            return None;
        }
        return Some((leading_ws + body_start, &trimmed[body_start..]));
    }

    if first_lower == "i" {
        if !first_digits.is_empty() {
            let body_start = words
                .get(1)
                .map(|word| word.0)
                .unwrap_or_else(|| trimmed.len());
            return Some((leading_ws + body_start, &trimmed[body_start..]));
        }
        let id = words.get(1)?;
        let body_start = words
            .get(2)
            .map(|word| word.0)
            .unwrap_or_else(|| trimmed.len());
        if id.2.parse::<isize>().is_err() {
            return None;
        }
        return Some((leading_ws + body_start, &trimmed[body_start..]));
    }

    Some((leading_ws, trimmed))
}

fn words_with_spans(input: &str) -> Vec<(usize, usize, &str)> {
    let mut out = Vec::new();
    let mut start = None;
    for (idx, ch) in input.char_indices() {
        if ch.is_whitespace() {
            if let Some(word_start) = start.take() {
                out.push((word_start, idx, &input[word_start..idx]));
            }
        } else if start.is_none() {
            start = Some(idx);
        }
    }
    if let Some(word_start) = start {
        out.push((word_start, input.len(), &input[word_start..]));
    }
    out
}

fn metadata_command_completion(body: &str) -> Option<MetadataCompletion> {
    let body_leading_ws = body.len().saturating_sub(body.trim_start().len());
    let body_trimmed = body.trim_start();
    if body_trimmed.is_empty() {
        return None;
    }
    let mut tokens: Vec<&str> = body_trimmed.split_whitespace().collect();
    let head = tokens.first().copied()?;
    let meta = command::command_metadata(head)?;
    let trailing_space = body_trimmed.chars().last().is_some_and(char::is_whitespace);

    if tokens.len() == 1 || trailing_space {
        tokens.push("");
    }

    let current = tokens.last().copied().unwrap_or("");
    let before_current = &tokens[..tokens.len().saturating_sub(1)];
    if let Some(mut completion) = metadata_value_completion(meta, before_current, current) {
        if let Some(replacement) = completion.replacement {
            completion.replacement =
                Some(format!("{}{}", " ".repeat(body_leading_ws), replacement));
        }
        return Some(completion);
    }
    let replacement = metadata_command_replacement(meta, before_current, current)?;
    let replacement = format!("{}{}", " ".repeat(body_leading_ws), replacement);
    Some(MetadataCompletion {
        replacement: Some(replacement),
        message: None,
    })
}

fn metadata_command_replacement(
    meta: &'static command::CommandMetadata,
    before_current: &[&str],
    current: &str,
) -> Option<String> {
    let head = before_current.first().copied()?;
    let mut body = vec![meta.name.to_string()];
    let mut idx = 1usize;
    if let Some(target) = before_current.get(idx).and_then(|token| {
        command::command_token_name(meta.targets, canonical_completion_key(token))
    }) {
        body.push(target.to_string());
        idx += 1;
    }

    let current_key = canonical_completion_key(current);
    if idx == 1 && before_current.len() == 1 {
        if let Some(target) = exact_completion_target(meta, current_key) {
            body.push(target.to_string());
            body.push(meta.first_parameter.to_string());
            return Some(format!("{} ", body.join(" ")));
        }
        if current_key.is_empty() {
            body.push(meta.first_parameter.to_string());
            return Some(format!("{} ", body.join(" ")));
        }
        if let Some(token) = first_matching_completion_token(meta, current_key) {
            body.push(token.to_string());
            if exact_completion_target(meta, token).is_some() {
                body.push(meta.first_parameter.to_string());
            }
            return Some(format!("{} ", body.join(" ")));
        }
        return None;
    }

    let mut used = Vec::new();
    let mut scan = idx;
    while scan < before_current.len() {
        let token = before_current[scan];
        let key = canonical_completion_key(token);
        if let Some(param) = command::command_parameter(meta, key) {
            body.push(param.name.to_string());
            used.push(param.name);
            if token.contains('=') {
                scan += 1;
            } else {
                if let Some(value) = before_current.get(scan + 1) {
                    if command::command_parameter(meta, canonical_completion_key(value)).is_none()
                        && command::command_token_name(
                            meta.targets,
                            canonical_completion_key(value),
                        )
                        .is_none()
                    {
                        body.push((*value).to_string());
                        scan += 1;
                    }
                }
                scan += 1;
            }
        } else {
            body.push(token.to_string());
            scan += 1;
        }
    }

    if let Some(param) = command::command_parameter(meta, current_key) {
        body.push(param.name.to_string());
        return Some(format!("{} ", body.join(" ")));
    }
    if let Some(param) = first_matching_completion_parameter(meta, current_key, &used) {
        body.push(param.name.to_string());
        return Some(format!("{} ", body.join(" ")));
    }
    if current_key.is_empty() {
        if let Some(param) = meta
            .parameters
            .iter()
            .find(|param| !used.contains(&param.name) && param_expects_value(param))
        {
            body.push(param.name.to_string());
            return Some(format!("{} ", body.join(" ")));
        }
    }

    if head != meta.name {
        Some(body.join(" "))
    } else {
        None
    }
}

fn metadata_value_completion(
    meta: &'static command::CommandMetadata,
    before_current: &[&str],
    current: &str,
) -> Option<MetadataCompletion> {
    if before_current.len() == 2
        && command::command_token_name(meta.targets, canonical_completion_key(before_current[1]))
            .is_some()
    {
        return None;
    }
    let param = before_current
        .last()
        .and_then(|token| command::command_parameter(meta, canonical_completion_key(token)))?;
    if !param_expects_value(param) {
        return None;
    }
    if param.values.is_empty() {
        return Some(MetadataCompletion {
            replacement: None,
            message: Some(format!(
                "{} {} {}",
                meta.name,
                param.name,
                parameter_value_hint(param)
            )),
        });
    }
    let value = param
        .values
        .iter()
        .find(|value| value.starts_with(current))
        .copied()?;
    let mut body: Vec<String> = before_current
        .iter()
        .map(|token| (*token).to_string())
        .collect();
    body[0] = meta.name.to_string();
    body.push(value.to_string());
    Some(MetadataCompletion {
        replacement: Some(format!("{} ", body.join(" "))),
        message: None,
    })
}

fn param_expects_value(param: &command::CommandParameterMetadata) -> bool {
    !param.values.is_empty()
        || param.lower.is_some()
        || param.upper.is_some()
        || !param.units.is_empty()
}

fn parameter_value_hint(param: &command::CommandParameterMetadata) -> String {
    if !param.values.is_empty() {
        return format!("<{}>", param.values.join("|"));
    }
    let range = match (param.lower, param.upper) {
        (Some(lower), Some(upper)) => format!("{}..{}", compact_float(lower), compact_float(upper)),
        (Some(lower), None) => format!(">= {}", compact_float(lower)),
        (None, Some(upper)) => format!("<= {}", compact_float(upper)),
        (None, None) => "value".to_string(),
    };
    let units = if param.units.is_empty() {
        String::new()
    } else {
        format!(" {}", param.units)
    };
    format!("<{range}{units}|+n|-n|+nt>")
}

fn compact_float(value: f64) -> String {
    let mut out = format!("{value:.5}");
    while out.contains('.') && out.ends_with('0') {
        out.pop();
    }
    if out.ends_with('.') {
        out.pop();
    }
    out
}

fn canonical_completion_key(token: &str) -> &str {
    token.split_once('=').map(|(key, _)| key).unwrap_or(token)
}

fn exact_completion_target(
    meta: &'static command::CommandMetadata,
    token: &str,
) -> Option<&'static str> {
    if token.is_empty() {
        return None;
    }
    command::command_token_name(meta.targets, token)
}

fn first_matching_completion_token(
    meta: &'static command::CommandMetadata,
    partial: &str,
) -> Option<&'static str> {
    meta.targets
        .iter()
        .find(|target| completion_token_matches(target.name, target.aliases, partial))
        .map(|target| target.name)
        .or_else(|| first_matching_completion_parameter(meta, partial, &[]).map(|p| p.name))
}

fn first_matching_completion_parameter(
    meta: &'static command::CommandMetadata,
    partial: &str,
    used: &[&str],
) -> Option<&'static command::CommandParameterMetadata> {
    meta.parameters.iter().find(|param| {
        !used.contains(&param.name)
            && param_expects_value(param)
            && completion_token_matches(param.name, param.aliases, partial)
    })
}

fn completion_token_matches(name: &str, aliases: &[&str], partial: &str) -> bool {
    !partial.is_empty()
        && (name.starts_with(partial) || aliases.iter().any(|alias| alias.starts_with(partial)))
}

fn mq_matches(cmd: &str, partial: &str) -> Vec<String> {
    let partial_path = Path::new(partial);
    let (dir, prefix) = match partial_path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => (
            PathBuf::from(parent),
            partial_path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(""),
        ),
        _ => (PathBuf::from("."), partial),
    };

    let recursive = cmd == "load" && !partial.contains('/');
    let mut matches = if recursive {
        recursive_mq_matches(Path::new("."), prefix, Path::new("."))
    } else {
        direct_mq_matches(&dir, prefix)
    };
    matches.sort();
    matches.dedup();
    matches
}

fn direct_mq_matches(dir: &Path, prefix: &str) -> Vec<String> {
    fs::read_dir(dir)
        .ok()
        .into_iter()
        .flat_map(|it| it.filter_map(Result::ok))
        .filter_map(|entry| {
            let path = entry.path();
            if entry.file_type().ok()?.is_dir() {
                return None;
            }
            if path.extension().and_then(|s| s.to_str()) != Some("mq") {
                return None;
            }
            let name = path.file_name()?.to_str()?;
            if !name.starts_with(prefix) {
                return None;
            }
            if dir == Path::new(".") {
                Some(name.to_string())
            } else {
                Some(dir.join(name).to_string_lossy().replace('\\', "/"))
            }
        })
        .collect()
}

fn recursive_mq_matches(dir: &Path, prefix: &str, base: &Path) -> Vec<String> {
    let mut out: Vec<String> = fs::read_dir(dir)
        .ok()
        .into_iter()
        .flat_map(|it| it.filter_map(Result::ok))
        .filter_map(|entry| {
            let path = entry.path();
            if entry.file_type().ok()?.is_dir() {
                return None;
            }
            if path.extension().and_then(|s| s.to_str()) != Some("mq") {
                return None;
            }
            let name = path.file_name()?.to_str()?;
            if !name.starts_with(prefix) {
                return None;
            }
            Some(
                path.strip_prefix(base)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/"),
            )
        })
        .collect();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.filter_map(Result::ok) {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if name.starts_with('.') || name == "target" {
                continue;
            }
            out.extend(recursive_mq_matches(&entry.path(), prefix, base));
        }
    }
    out
}

fn longest_common_prefix(items: &[String]) -> String {
    let Some(first) = items.first() else {
        return String::new();
    };
    let mut prefix = first.clone();
    for item in &items[1..] {
        let mut keep = 0usize;
        for (a, b) in prefix.chars().zip(item.chars()) {
            if a != b {
                break;
            }
            keep += a.len_utf8();
        }
        prefix.truncate(keep);
        if prefix.is_empty() {
            break;
        }
    }
    prefix
}

fn completion_common_prefix(cmd: &str, partial: &str, matches: &[String]) -> String {
    if cmd == "load" && !partial.contains('/') {
        let basenames: Vec<String> = matches
            .iter()
            .filter_map(|path| {
                Path::new(path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_string)
            })
            .collect();
        return longest_common_prefix(&basenames);
    }
    longest_common_prefix(matches)
}

fn resolve_rhythms(specs: Vec<JinsSpec>, default: &[u8]) -> Result<Vec<BarSpec>, String> {
    let n = specs.len();
    let mut groups: Vec<Option<Vec<u8>>> = specs.iter().map(|s| s.groups.clone()).collect();
    let mut carry: Option<Vec<u8>> = None;
    for i in (0..n).rev() {
        if groups[i].is_some() {
            carry = groups[i].clone();
        } else {
            groups[i] = carry.clone();
        }
    }
    let fallback = default.to_vec();
    Ok(specs
        .into_iter()
        .zip(groups)
        .map(|(spec, grp)| BarSpec {
            src: spec.src,
            root: spec.root,
            maqam: spec.maqam,
            groups: grp.unwrap_or_else(|| fallback.clone()),
        })
        .collect())
}

fn apply_bpm_change(current: f64, change: ValueChange) -> Result<f64, String> {
    let next = change.apply(current)?;
    if !(20.0..=400.0).contains(&next) {
        return Err(format!("bpm {next} out of range"));
    }
    Ok(next)
}

fn apply_sustain_change(current: f64, change: ValueChange) -> Result<f64, String> {
    let next = change.apply(current)?;
    if !(0.05..=10.0).contains(&next) {
        return Err(format!("sustain {next}s out of range"));
    }
    Ok(next)
}

fn vcf_change_src(change: command::VcfChange) -> String {
    let mut parts = vec!["vcf".to_string()];
    if let Some(target) = change.target {
        parts.push(target.as_str().to_string());
    }
    if change.enabled == Some(false) {
        if change.target == Some(VcfTarget::All) || change.target.is_none() {
            return "vcf off".to_string();
        }
        return format!("vcf {} off", change.target.unwrap().as_str());
    }
    if let Some(cutoff) = change.cutoff_hz {
        parts.push("cut".to_string());
        parts.push(value_change_src(cutoff));
    }
    if let Some(resonance) = change.resonance {
        parts.push("res".to_string());
        parts.push(value_change_src(resonance));
    }
    if let Some(drive) = change.drive {
        parts.push("drive".to_string());
        parts.push(value_change_src(drive));
    }
    if let Some(wave) = change
        .wave
        .filter(|_| !matches!(change.target, Some(VcfTarget::All | VcfTarget::Mic)))
    {
        parts.push("wave".to_string());
        parts.push(wave.as_str().to_string());
    }
    parts.join(" ")
}

fn sym_change_src(change: command::SympatheticChange) -> String {
    let mut parts = vec!["sym".to_string()];
    if let Some(target) = change.target {
        parts.push(target.as_str().to_string());
    }
    if let Some(enabled) = change.enabled {
        parts.push(if enabled { "on" } else { "off" }.to_string());
    }
    if let Some(decay) = change.decay {
        parts.push("decay".to_string());
        parts.push(format!("{decay}"));
    }
    if let Some(gain) = change.gain {
        parts.push("drive".to_string());
        parts.push(format!("{gain}"));
    }
    if let Some(ratio) = change.interval_ratio {
        if ratio < 1.0 {
            parts.push("down".to_string());
            parts.push(sym_interval_name(1.0 / ratio));
        } else {
            parts.push("up".to_string());
            parts.push(sym_interval_name(ratio));
        }
    }
    if let Some(harmony) = change.harmony {
        parts.push("harmony".to_string());
        for component in harmony.iter() {
            parts.push(sym_harmony_interval_name(component.ratio));
            parts.push(format!("{:.2}", component.weight));
        }
    }
    if let Some(amount) = change.amount {
        parts.push("amount".to_string());
        parts.push(format!("{amount}"));
    }
    if let Some(mic) = change.mic {
        parts.push("mic".to_string());
        parts.push(format!("{mic}"));
    }
    if let Some(kanun) = change.kanun {
        parts.push("kanun".to_string());
        parts.push(format!("{kanun}"));
    }
    if let Some(bass) = change.bass {
        parts.push("bass".to_string());
        parts.push(format!("{bass}"));
    }
    if let Some(drums) = change.drums {
        parts.push("drums".to_string());
        parts.push(format!("{drums}"));
    }
    parts.join(" ")
}

fn sym_interval_name(ratio: f64) -> String {
    let known = [
        (1.0, "unison"),
        (16.0 / 15.0, "minor-second"),
        (9.0 / 8.0, "second"),
        (6.0 / 5.0, "third"),
        (5.0 / 4.0, "major-third"),
        (4.0 / 3.0, "fourth"),
        (45.0 / 32.0, "tritone"),
        (3.0 / 2.0, "fifth"),
        (8.0 / 5.0, "sixth"),
        (5.0 / 3.0, "major-sixth"),
        (9.0 / 5.0, "minor-seventh"),
        (15.0 / 8.0, "seventh"),
        (2.0, "octave"),
    ];
    known
        .iter()
        .find_map(|(known_ratio, name)| {
            ((ratio - known_ratio).abs() < 0.000_001).then_some((*name).to_string())
        })
        .unwrap_or_else(|| format!("{ratio:.5}"))
}

fn sym_harmony_interval_name(ratio: f64) -> String {
    if (ratio - 1.0).abs() < 0.000_001 {
        "root".to_string()
    } else {
        sym_interval_name(ratio)
    }
}

fn value_change_src(change: ValueChange) -> String {
    match change {
        ValueChange::Set(n) => format!("{n}"),
        ValueChange::Add(n) if n < 0.0 => format!("{n}"),
        ValueChange::Add(n) => format!("+{n}"),
        ValueChange::Mul(n) => format!("*{n}"),
        ValueChange::Div(n) => format!("/{n}"),
        ValueChange::Tick(n) if n < 0.0 => format!("{n}t"),
        ValueChange::Tick(n) => format!("+{n}t"),
    }
}

fn tune_to_src(pitch: crate::tuning::Pitch) -> String {
    format!("tuneto {}", pitch.source_token())
}

fn fx_change_src(change: command::FxChange) -> String {
    if change.reverb_enabled == Some(false) && change.delay_enabled == Some(false) {
        return "fx off".to_string();
    }
    let mut parts = Vec::new();
    if change.reverb_enabled.is_some()
        || change.reverb_mix.is_some()
        || change.reverb_decay.is_some()
    {
        parts.push("reverb".to_string());
        if change.reverb_enabled == Some(false) {
            parts.push("off".to_string());
            return parts.join(" ");
        }
        if let Some(mix) = change.reverb_mix {
            parts.push("mix".to_string());
            parts.push(value_change_src(mix));
        }
        if let Some(decay) = change.reverb_decay {
            parts.push("decay".to_string());
            parts.push(value_change_src(decay));
        }
    } else {
        parts.push("delay".to_string());
        if change.delay_enabled == Some(false) {
            parts.push("off".to_string());
            return parts.join(" ");
        }
        if let Some(time) = change.delay_time_secs {
            parts.push("time".to_string());
            parts.push(value_change_src(time));
        }
        if let Some(feedback) = change.delay_feedback {
            parts.push("feedback".to_string());
            parts.push(value_change_src(feedback));
        }
        if let Some(mix) = change.delay_mix {
            parts.push("mix".to_string());
            parts.push(value_change_src(mix));
        }
    }
    parts.join(" ")
}

fn describe_fx(fx: FxSettings) -> String {
    let rev = if fx.reverb_enabled {
        format!("rev {:.2}/{:.2}", fx.reverb_mix, fx.reverb_decay)
    } else {
        "rev off".to_string()
    };
    let delay = if fx.delay_enabled {
        format!(
            "delay {:.2}s/{:.2}/{:.2}",
            fx.delay_time_secs, fx.delay_feedback, fx.delay_mix
        )
    } else {
        "delay off".to_string()
    };
    format!("{rev} {delay}")
}

fn describe_vcf(v: VcfSettings) -> String {
    if !v.enabled {
        if v.target == VcfTarget::All {
            return "vcf off".to_string();
        }
        return format!("vcf {} off", v.target.as_str());
    }
    format!(
        "vcf {} cut {:.1} Hz  res {:.2}  drive {:.2}  {}",
        v.target.as_str(),
        v.cutoff_hz,
        v.resonance,
        v.drive,
        v.wave.as_str()
    )
}

fn is_plain_control_line(line: &str) -> bool {
    line.starts_with("bpm ")
        || line.starts_with("s ")
        || line.starts_with("sus ")
        || is_plain_vcf_control_line(line)
        || is_plain_fx_control_line(line)
        || line
            .split_whitespace()
            .next()
            .is_some_and(|word| word.eq_ignore_ascii_case("sym"))
}

fn is_plain_vcf_control_line(line: &str) -> bool {
    let first = line.split_whitespace().next().unwrap_or("");
    matches!(
        first.to_ascii_lowercase().as_str(),
        "vcf" | "filter" | "filt" | "cut" | "cutoff" | "res" | "q" | "drive" | "drv"
    )
}

fn is_plain_fx_control_line(line: &str) -> bool {
    let first = line.split_whitespace().next().unwrap_or("");
    matches!(
        first.to_ascii_lowercase().as_str(),
        "fx" | "reverb" | "rev" | "delay" | "pingpong"
    )
}

fn resolve_id_ref_in_phrases(phrases: &[Phrase], id_ref: isize) -> Option<usize> {
    if id_ref == crate::command::START_REF {
        return phrases.first().map(|phrase| phrase.id);
    }
    if id_ref >= 0 {
        return Some(id_ref as usize);
    }
    let back = id_ref.unsigned_abs();
    if back == 0 || back > phrases.len() {
        return None;
    }
    phrases.get(phrases.len() - back).map(|p| p.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::bounded;
    use std::sync::{Mutex, OnceLock};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn parses_direct_nam_search_command() {
        let parsed = command::parse("nam search clean dynamic A2 amp cab").unwrap();
        assert!(matches!(
            parsed,
            Cmd::SetNam(NamCommand::Search { query })
                if query == "clean dynamic A2 amp cab"
        ));
        let pinned = command::parse("nam pin https://models.example/clean.nam as clean").unwrap();
        assert!(matches!(
            pinned,
            Cmd::SetNam(NamCommand::Pin { url, name })
                if url == "https://models.example/clean.nam" && name == "clean"
        ));
        let tone = command::parse("nam tone3000 45896 as nama2").unwrap();
        assert!(matches!(
            tone,
            Cmd::SetNam(NamCommand::Tone3000 { tone_id: 45896, name }) if name == "nama2"
        ));
    }

    #[test]
    fn parses_tone3000_login_commands() {
        assert!(matches!(
            command::parse("nam login").unwrap(),
            Cmd::SetNam(NamCommand::Login)
        ));
        assert!(matches!(
            command::parse("nam logout").unwrap(),
            Cmd::SetNam(NamCommand::Logout)
        ));
        assert!(matches!(
            command::parse("nam input right").unwrap(),
            Cmd::SetNam(NamCommand::Input(NamInput::Right))
        ));
        assert!(matches!(
            command::parse("nam latency left").unwrap(),
            Cmd::SetNam(NamCommand::Latency(NamInput::Left))
        ));
        assert!(matches!(
            command::parse("i 6 nam gain 4").unwrap(),
            Cmd::InsertNam {
                before: 6,
                command: NamCommand::Gain(4.0)
            }
        ));
        assert!(matches!(
            command::parse("edit 2 nam input stereo").unwrap(),
            Cmd::EditNam {
                id: 2,
                command: NamCommand::Input(NamInput::Stereo)
            }
        ));
    }

    #[test]
    fn pinning_nam_rewrites_ambiguous_session_dependency() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("maqam-pin-{suffix}.mq"));
        fs::write(&path, "MAQAM_SESSION_V3\nnam slot\nP|0|1|d bayati 4\n").unwrap();

        pin_nam_dependency(
            path.to_str().unwrap(),
            "slot",
            "https://models.example/clean.nam",
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "MAQAM_SESSION_V3\nnam pin https://models.example/clean.nam as slot\nP|0|1|d bayati 4\n"
        );
        let _ = fs::remove_file(path);
    }

    fn session_test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    #[test]
    fn phrase_display_source_preserves_the_users_text() {
        let (tx, _rx) = bounded(8);
        let mut app = App::new(tx);

        app.handle_command("d nah 332,   a hij 44 r3");

        assert_eq!(app.phrases[0].src, "d nah 332,   a hij 44 r3");
        assert_eq!(app.phrases[0].repeat, 3);
    }

    #[test]
    fn western_mode_names_parse_as_builtin_jins() {
        let (tx, _rx) = bounded(8);
        let mut app = App::new(tx);

        app.handle_command("d major 332");
        app.handle_command("e dorian 332");
        app.handle_command("f locrian 332");
        app.handle_command("g diminished 332");

        assert_eq!(app.phrases[0].bar.maqam_names, vec!["Major"]);
        assert_eq!(
            app.phrases[0].bar.ratio_strs[0],
            "1/1 9/8 5/4 4/3 3/2 5/3 15/8"
        );
        assert_eq!(app.phrases[1].bar.maqam_names, vec!["Dorian"]);
        assert_eq!(app.phrases[2].bar.maqam_names, vec!["Locrian"]);
        assert_eq!(
            app.phrases[2].bar.ratio_strs[0],
            "1/1 16/15 6/5 4/3 64/45 8/5 9/5"
        );
        assert_eq!(app.phrases[3].bar.maqam_names, vec!["Diminished"]);
        assert_eq!(
            app.phrases[3].bar.ratio_strs[0],
            "1/1 9/8 6/5 4/3 64/45 8/5 5/3 15/8"
        );
    }

    #[test]
    fn inserts_sym_drive_as_a_timeline_control() {
        let (tx, _rx) = bounded(32);
        let mut app = App::new(tx);
        for _ in 0..5 {
            app.handle_command("d bayati 4");
        }

        app.handle_command("i 4 sym drive 64");

        assert_eq!(app.phrases[4].id, 5);
        assert_eq!(app.phrases[4].src, "sym drive 64");
        assert!(matches!(
            app.phrases[4].control,
            Some(ControlSpec::SetSympatheticGain(64.0))
        ));
        assert_eq!(app.phrases[5].id, 4);

        app.handle_command("edit 5 sym gain 96");
        assert_eq!(app.phrases[4].id, 5);
        assert_eq!(app.phrases[4].src, "sym gain 96");
        assert!(matches!(
            app.phrases[4].control,
            Some(ControlSpec::SetSympatheticGain(96.0))
        ));

        app.handle_command("edit 5 sym decay 0.999 drive 2 kanun 0.5 bass 0.5");
        assert_eq!(
            app.phrases[4].src,
            "sym decay 0.999 drive 2 kanun 0.5 bass 0.5"
        );
        let Some(ControlSpec::SetSympathetic(change)) = app.phrases[4].control else {
            panic!("expected combined sym control");
        };
        assert_eq!(change.target, None);
        assert_eq!(change.enabled, None);
        assert_eq!(change.decay, Some(0.999));
        assert_eq!(change.gain, Some(2.0));
        assert_eq!(change.interval_ratio, None);
        assert_eq!(change.amount, None);
        assert_eq!(change.mic, None);
        assert_eq!(change.kanun, Some(0.5));
        assert_eq!(change.bass, Some(0.5));
        assert_eq!(change.drums, None);

        app.handle_command("edit 5 sym mic decay 0.9999 drive 8 amount 1.5");
        assert_eq!(
            app.phrases[4].src,
            "sym mic decay 0.9999 drive 8 amount 1.5"
        );
        let Some(ControlSpec::SetSympathetic(change)) = app.phrases[4].control else {
            panic!("expected targeted sym control");
        };
        assert_eq!(change.target, Some(command::SympatheticTarget::Mic));
        assert_eq!(change.enabled, None);
        assert_eq!(change.decay, Some(0.9999));
        assert_eq!(change.gain, Some(8.0));
        assert_eq!(change.interval_ratio, None);
        assert_eq!(change.amount, Some(1.5));
        assert_eq!(change.kanun, None);

        app.handle_command("edit 5 sym up fifth");
        assert_eq!(app.phrases[4].src, "sym up fifth");
        let Some(ControlSpec::SetSympathetic(change)) = app.phrases[4].control else {
            panic!("expected interval sym control");
        };
        assert_eq!(change.interval_ratio, Some(3.0 / 2.0));

        app.handle_command("edit 5 sym harmony root 0.50 third 0.25 fifth 0.25");
        assert_eq!(
            app.phrases[4].src,
            "sym harmony root 0.50 third 0.25 fifth 0.25"
        );
        let Some(ControlSpec::SetSympathetic(change)) = app.phrases[4].control else {
            panic!("expected harmony sym control");
        };
        let harmony = change.harmony.expect("expected harmony");
        assert_eq!(harmony.len, 3);
        let components: Vec<_> = harmony.iter().collect();
        assert!((components[0].ratio - 1.0).abs() < f64::EPSILON);
        assert!((components[0].weight - 0.5).abs() < f32::EPSILON);
        assert!((components[1].ratio - 6.0 / 5.0).abs() < f64::EPSILON);
        assert!((components[1].weight - 0.25).abs() < f32::EPSILON);
        assert!((components[2].ratio - 3.0 / 2.0).abs() < f64::EPSILON);
        assert!((components[2].weight - 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn sympathetic_aliases_and_repeated_noun_parse() {
        assert!(matches!(
            command::parse("sympathetics on").unwrap(),
            Cmd::Sympathetics(true)
        ));
        assert!(matches!(
            command::parse("tanbura off").unwrap(),
            Cmd::Sympathetics(false)
        ));

        let parsed = command::parse("sym on sym decay 0.999 drive 2").unwrap();
        assert!(matches!(
            parsed,
            Cmd::Sympathetic(command::SympatheticChange {
                enabled: Some(true),
                decay: Some(decay),
                gain: Some(gain),
                ..
            }) if (decay - 0.999).abs() < f32::EPSILON && (gain - 2.0).abs() < f32::EPSILON
        ));

        assert!(matches!(
            command::parse("sym up third").unwrap(),
            Cmd::Sympathetic(command::SympatheticChange {
                interval_ratio: Some(ratio),
                ..
            }) if (ratio - 6.0 / 5.0).abs() < f64::EPSILON
        ));
        assert!(matches!(
            command::parse("sym up major-third").unwrap(),
            Cmd::Sympathetic(command::SympatheticChange {
                interval_ratio: Some(ratio),
                ..
            }) if (ratio - 5.0 / 4.0).abs() < f64::EPSILON
        ));
        assert!(matches!(
            command::parse("sym down fifth").unwrap(),
            Cmd::Sympathetic(command::SympatheticChange {
                interval_ratio: Some(ratio),
                ..
            }) if (ratio - 2.0 / 3.0).abs() < f64::EPSILON
        ));
        assert!(matches!(
            command::parse("sym interval 3/2").unwrap(),
            Cmd::Sympathetic(command::SympatheticChange {
                interval_ratio: Some(ratio),
                ..
            }) if (ratio - 3.0 / 2.0).abs() < f64::EPSILON
        ));

        let parsed = command::parse("sym harmony root third fourth octave").unwrap();
        let Cmd::Sympathetic(change) = parsed else {
            panic!("expected sym harmony command");
        };
        let harmony = change.harmony.expect("expected harmony");
        assert_eq!(harmony.len, 4);
        let components: Vec<_> = harmony.iter().collect();
        assert!((components[0].ratio - 1.0).abs() < f64::EPSILON);
        assert!((components[0].weight - 1.0).abs() < f32::EPSILON);
        assert!((components[1].ratio - 6.0 / 5.0).abs() < f64::EPSILON);
        assert!((components[2].ratio - 4.0 / 3.0).abs() < f64::EPSILON);
        assert!((components[3].ratio - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn nam_input_commands_parse_as_live_state() {
        assert!(matches!(
            command::parse("nam off").unwrap(),
            Cmd::SetNam(NamCommand::Off)
        ));
        assert!(matches!(
            command::parse("nam ls").unwrap(),
            Cmd::SetNam(NamCommand::List)
        ));
        assert!(matches!(
            command::parse("nam gain 1.5").unwrap(),
            Cmd::SetNam(NamCommand::Gain(gain)) if (gain - 1.5).abs() < f32::EPSILON
        ));
        let parsed = command::parse("nam import metallica.nam as metallica").unwrap();
        let Cmd::SetNam(NamCommand::Import { path, name }) = parsed else {
            panic!("expected NAM import command");
        };
        assert_eq!(path, "metallica.nam");
        assert_eq!(name.as_deref(), Some("metallica"));
        let parsed = command::parse("nam metallica").unwrap();
        let Cmd::SetNam(NamCommand::Load { path }) = parsed else {
            panic!("expected NAM load command");
        };
        assert_eq!(path, "metallica");
        assert!(command::parse("nam gain 12").is_err());
    }

    #[test]
    fn nam_cache_names_are_shell_friendly() {
        assert_eq!(sanitize_nam_cache_name("Metallica A2"), "Metallica-A2");
        assert_eq!(sanitize_nam_cache_name("../bad name!!"), "bad-name");
        assert_eq!(
            nam_cache_name_from_path(Path::new("mesa dual rect.nam")),
            "mesa-dual-rect"
        );
        assert_eq!(
            nam_cache_name_from_url("https://example.test/models/NAM%20A2.nam?download=1"),
            "NAM20A2"
        );
        assert_eq!(
            nam_cache_name_from_url("https://example.test/models/"),
            "models"
        );
    }

    #[test]
    fn nam_download_content_range_total_is_parsed() {
        assert_eq!(
            parse_content_range_total("bytes 100-199/123456"),
            Some(123456)
        );
        assert_eq!(parse_content_range_total("bytes 100-199/*"), None);
        assert_eq!(parse_content_range_total("not a range"), None);
    }

    #[test]
    fn cached_nam_download_reports_cached_done() {
        let _guard = session_test_lock();
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("maqam-cached-nam-{suffix}"));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("cached.nam"), b"already here").unwrap();
        let (tx, rx) = crossbeam_channel::unbounded();

        download_nam_capture(
            "https://example.test/cached.nam",
            &dir,
            "cached",
            true,
            None,
            tx,
        )
        .unwrap();

        assert!(matches!(
            rx.recv().unwrap().unwrap(),
            NamDownloadEvent::Progress {
                downloaded: 12,
                total: Some(12)
            }
        ));
        assert!(matches!(
            rx.recv().unwrap().unwrap(),
            NamDownloadEvent::Done {
                name,
                load_after: true,
                cached: true
            } if name == "cached"
        ));
        let _ = fs::remove_dir_all(dir);
    }

    fn test_nam_json(sample_rate: u32) -> String {
        format!(
            r#"{{
                "version": "0.5.4",
                "architecture": "LSTM",
                "config": {{ "input_size": 1, "hidden_size": 1, "num_layers": 1 }},
                "weights": [1.0,0.0, 0.0,0.0, 2.0,0.0, 0.0,0.0, 0.0,0.0,0.0,0.0, 0.0, 0.0, 3.0, 0.5],
                "sample_rate": {sample_rate}
            }}"#
        )
    }

    #[test]
    fn startup_commands_prefer_cached_nam_sample_rate() {
        let _guard = session_test_lock();
        let old_cache = std::env::var("MAQAM_NAM_CACHE_DIR").ok();
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("maqam-startup-nam-cache-{suffix}"));
        let session = std::env::temp_dir().join(format!("maqam-startup-nam-{suffix}.mq"));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("cached.nam"), test_nam_json(48_000)).unwrap();
        fs::write(
            &session,
            "MAQAM_SESSION_V3\nN|0|nam tone3000 123 as cached\nP|1|1|d bayati 44\n",
        )
        .unwrap();
        std::env::set_var("MAQAM_NAM_CACHE_DIR", &dir);

        let rate = preferred_nam_sample_rate_for_startup_commands(&[format!(
            "load {}",
            session.display()
        )]);

        assert_eq!(rate, Some(48_000));
        let _ = fs::remove_file(session);
        let _ = fs::remove_dir_all(dir);
        match old_cache {
            Some(value) => std::env::set_var("MAQAM_NAM_CACHE_DIR", value),
            None => std::env::remove_var("MAQAM_NAM_CACHE_DIR"),
        }
    }

    #[test]
    fn cached_nam_models_prefer_single_sample_rate() {
        let _guard = session_test_lock();
        let old_cache = std::env::var("MAQAM_NAM_CACHE_DIR").ok();
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("maqam-cache-rate-{suffix}"));
        fs::create_dir_all(&dir).unwrap();
        std::env::set_var("MAQAM_NAM_CACHE_DIR", &dir);

        fs::write(dir.join("one.nam"), test_nam_json(48_000)).unwrap();
        fs::write(dir.join("two.nam"), test_nam_json(48_000)).unwrap();
        assert_eq!(preferred_nam_sample_rate_for_cached_models(), Some(48_000));

        fs::write(dir.join("other.nam"), test_nam_json(44_100)).unwrap();
        assert_eq!(preferred_nam_sample_rate_for_cached_models(), None);

        let _ = fs::remove_dir_all(dir);
        match old_cache {
            Some(value) => std::env::set_var("MAQAM_NAM_CACHE_DIR", value),
            None => std::env::remove_var("MAQAM_NAM_CACHE_DIR"),
        }
    }

    #[test]
    fn successful_nam_load_clears_error_status() {
        crate::NAM_MODEL_ACTIVE.store(false, std::sync::atomic::Ordering::Relaxed);
        crate::set_nam_error("sample-rate mismatch; restart with MAQAM_SAMPLE_RATE=48000");

        mark_nam_loaded();

        assert!(crate::NAM_MODEL_ACTIVE.load(std::sync::atomic::Ordering::Relaxed));
        assert_eq!(
            crate::NAM_STATUS.load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        assert!(crate::nam_error().lock().unwrap().is_none());
    }

    #[test]
    fn listing_nam_cache_creates_missing_cache_dir() {
        let _guard = session_test_lock();
        let old_cache = std::env::var("MAQAM_NAM_CACHE_DIR").ok();
        let dir = std::env::temp_dir().join(format!(
            "maqam-nam-cache-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::env::set_var("MAQAM_NAM_CACHE_DIR", &dir);

        let names = list_cached_nam_models(&nam_cache_dir()).unwrap();

        assert!(names.is_empty());
        assert!(dir.is_dir());
        let _ = fs::remove_dir(&dir);
        match old_cache {
            Some(value) => std::env::set_var("MAQAM_NAM_CACHE_DIR", value),
            None => std::env::remove_var("MAQAM_NAM_CACHE_DIR"),
        }
    }

    #[test]
    fn missing_nam_live_state_does_not_block_session_load() {
        let _guard = session_test_lock();
        let old_cache_dir = std::env::var("MAQAM_NAM_CACHE_DIR").ok();
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let cache_dir = std::env::temp_dir().join(format!("maqam-empty-nam-cache-{suffix}"));
        std::env::set_var("MAQAM_NAM_CACHE_DIR", &cache_dir);

        let (tx, _rx) = bounded(16);
        let mut app = App::new(tx);
        app.load_session_v3(["nam practice", "P|0|1|d bayati 4444"].into_iter())
            .unwrap();
        assert_eq!(app.live_nam_commands, vec!["nam practice"]);
        assert_eq!(app.phrases.len(), 2);
        assert_eq!(app.phrases[0].src, "nam practice");
        assert!(matches!(
            app.phrases[0].control,
            Some(ControlSpec::SetNamEnabled(true))
        ));
        assert_eq!(app.phrases[1].src, "d bayati 4444");
        assert_eq!(app.pending_nam_slot.as_deref(), Some("practice"));
        assert_eq!(
            app.message.as_deref(),
            Some("This score needs a NAM model for “practice”. What amp or tone should it use?")
        );

        let _ = fs::remove_dir_all(&cache_dir);
        if let Some(path) = old_cache_dir {
            std::env::set_var("MAQAM_NAM_CACHE_DIR", path);
        } else {
            std::env::remove_var("MAQAM_NAM_CACHE_DIR");
        }
    }

    #[test]
    fn loads_and_saves_v3_without_rewriting_input() {
        let _guard = session_test_lock();
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let input_path = std::env::temp_dir().join(format!("maqam-v3-input-{suffix}.mq"));
        let output_path = std::env::temp_dir().join(format!("maqam-v3-output-{suffix}.mq"));
        let source = concat!(
            "MAQAM_SESSION_V3\n",
            "create testv3 1/1 9/8 5/4\n",
            "vol 0.75\n",
            "B|4|180\n",
            "S|7|1.2\n",
            "Y|8|sym on\n",
            "Y|9|sym gain 64\n",
            "Y|10|sym decay 0.99\n",
            "P|11|2|g testv3 332\n",
            "J|15|11|3\n",
        );
        fs::write(&input_path, source).unwrap();

        let (tx, _rx) = bounded(32);
        let mut app = App::new(tx);
        app.vol = 0.42;
        app.load_session(input_path.to_str().unwrap()).unwrap();
        assert_eq!(app.vol, 0.42);

        assert_eq!(fs::read_to_string(&input_path).unwrap(), source);
        assert_eq!(
            app.phrases
                .iter()
                .map(|phrase| phrase.id)
                .collect::<Vec<_>>(),
            vec![4, 7, 8, 9, 10, 11, 15]
        );
        assert!(matches!(
            app.phrases[2].control,
            Some(ControlSpec::SetSympathetics(true))
        ));
        assert!(matches!(
            app.phrases[3].control,
            Some(ControlSpec::SetSympatheticGain(64.0))
        ));
        assert!(matches!(
            app.phrases[4].control,
            Some(ControlSpec::SetSympatheticDecay(0.99))
        ));
        assert_eq!(app.phrases[5].repeat, 2);
        assert_eq!(app.phrases[6].jump.as_ref().unwrap().target_id, 11);
        assert_eq!(app.next_phrase_id, 16);

        app.save_session(output_path.to_str().unwrap()).unwrap();
        let saved = fs::read_to_string(&output_path).unwrap();
        assert!(saved.starts_with("MAQAM_SESSION_V3\n"));
        assert!(!saved.contains("\nvol "));
        assert!(!saved.contains("tuneto"));
        assert!(saved.contains("B|4|180\n"));
        assert!(saved.contains("Y|8|sym on\n"));
        assert!(saved.contains("Y|9|sym gain 64\n"));
        assert!(saved.contains("Y|10|sym decay 0.99\n"));
        assert!(saved.contains("P|11|2|g testv3 332\n"));

        let _ = fs::remove_file(input_path);
        let _ = fs::remove_file(output_path);
    }

    #[test]
    fn missing_load_error_names_file_and_fix() {
        let _guard = session_test_lock();
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("maqam-missing-{suffix}.mq"));
        let (tx, _rx) = bounded(16);
        let mut app = App::new(tx);

        let err = app.load_session(path.to_str().unwrap()).unwrap_err();

        assert!(err.contains(path.to_str().unwrap()));
        assert!(err.contains("run `ls`"));
        assert!(err.contains("load FILENAME.mq"));
    }

    #[test]
    fn tuneto_is_live_state_and_not_saved_in_sessions() {
        let _guard = session_test_lock();
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let output_path = std::env::temp_dir().join(format!("maqam-tuneto-output-{suffix}.mq"));
        let (tx, _rx) = bounded(16);
        let mut app = App::new(tx);

        app.handle_command("tuneto a");
        assert_eq!(app.phrases.len(), 0);
        app.handle_command("a major 44");
        assert!((app.phrases[0].bar.root_hz - 440.0).abs() < 0.0001);

        app.save_session(output_path.to_str().unwrap()).unwrap();
        let saved = fs::read_to_string(&output_path).unwrap();
        assert!(!saved.contains("tuneto"));
        assert!(saved.contains("P|0|1|a major 44\n"));

        let _ = fs::remove_file(output_path);
    }

    #[test]
    fn globals_are_written_immediately_when_live_settings_change() {
        let _guard = session_test_lock();
        let old_path = std::env::var("MAQAM_GLOBALS_PATH").ok();
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let globals_path = std::env::temp_dir().join(format!("maqam-globals-{suffix}.ml"));
        std::env::set_var("MAQAM_GLOBALS_PATH", &globals_path);

        let (tx, _rx) = bounded(16);
        let mut app = App::new(tx);
        app.handle_command("vol 0.7");
        assert_eq!(
            fs::read_to_string(&globals_path).unwrap(),
            "vol 0.7\ntuneto d\n"
        );

        app.handle_command("tuneto a");
        assert_eq!(
            fs::read_to_string(&globals_path).unwrap(),
            "vol 0.7\ntuneto a\n"
        );
        assert!((crate::tuning::pitch_to_hz('a', 0, 4) - 440.0).abs() < 0.0001);

        let _ = fs::remove_file(&globals_path);
        if let Some(path) = old_path {
            std::env::set_var("MAQAM_GLOBALS_PATH", path);
        } else {
            std::env::remove_var("MAQAM_GLOBALS_PATH");
        }
    }

    #[test]
    fn globals_load_on_startup() {
        let _guard = session_test_lock();
        let old_path = std::env::var("MAQAM_GLOBALS_PATH").ok();
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let globals_path = std::env::temp_dir().join(format!("maqam-globals-load-{suffix}.ml"));
        fs::write(&globals_path, "vol 0.25\ntuneto a\n").unwrap();
        std::env::set_var("MAQAM_GLOBALS_PATH", &globals_path);

        let (tx, rx) = bounded(16);
        let app = App::new(tx);
        assert_eq!(app.vol, 0.25);
        assert_eq!(app.tune_to.source_token(), "a");
        assert!(matches!(rx.try_recv(), Ok(AudioCmd::SetVol(v)) if (v - 0.25).abs() < 0.0001));
        assert!((crate::tuning::pitch_to_hz('a', 0, 4) - 440.0).abs() < 0.0001);

        let _ = fs::remove_file(&globals_path);
        if let Some(path) = old_path {
            std::env::set_var("MAQAM_GLOBALS_PATH", path);
        } else {
            std::env::remove_var("MAQAM_GLOBALS_PATH");
        }
    }

    #[test]
    fn loads_legacy_control_lines_under_v3_header() {
        let _guard = session_test_lock();
        let (tx, _rx) = bounded(16);
        let mut app = App::new(tx);
        app.load_session_v3(["bpm 180", "s 1.2", "P|2|1|g hijaz 4444"].into_iter())
            .unwrap();

        assert_eq!(
            app.phrases
                .iter()
                .map(|phrase| phrase.id)
                .collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert_eq!(app.bpm, 180.0);
        assert_eq!(app.sustain, 1.2);
    }

    #[test]
    fn loads_vcf_control_lines_under_v3_header() {
        let _guard = session_test_lock();
        let (tx, _rx) = bounded(16);
        let mut app = App::new(tx);
        app.load_session_v3(
            [
                "vcf bass cut=900 res=0.65 drive=3.5",
                "cut +100",
                "V|5|kanun|1200|0.4|2.25",
                "P|6|1|g hijaz 4444",
            ]
            .into_iter(),
        )
        .unwrap();

        assert_eq!(
            app.phrases
                .iter()
                .map(|phrase| phrase.id)
                .collect::<Vec<_>>(),
            vec![0, 1, 5, 6]
        );
        assert_eq!(app.vcf.kanun.cutoff_hz, 1200.0);
        assert_eq!(app.vcf.kanun.resonance, 0.4);
        assert_eq!(app.vcf.kanun.drive, 2.25);
        assert_eq!(app.vcf.kanun.target, VcfTarget::Kanun);
    }

    #[test]
    fn vcf_off_is_a_transparent_control_command() {
        let _guard = session_test_lock();
        let (tx, _rx) = bounded(16);
        let mut app = App::new(tx);

        app.handle_command("vcf");
        assert!(app.vcf.all.enabled);
        assert_eq!(app.vcf.all.target, VcfTarget::All);
        assert_eq!(app.phrases.last().unwrap().src, "vcf");

        app.handle_command("vcf off");
        assert!(!app.vcf.all.enabled);
        assert!(!app.vcf.mic.enabled);
        assert!(!app.vcf.bass.enabled);
        assert!(!app.vcf.kanun.enabled);
        assert!(!app.vcf.kick.enabled);
        assert!(!app.vcf.tanbura.enabled);
        assert_eq!(app.vcf.focus, VcfTarget::All);

        app.handle_command("vcf bass off");
        assert!(!app.vcf.all.enabled);
        assert!(!app.vcf.mic.enabled);
        assert!(!app.vcf.bass.enabled);
        assert!(!app.vcf.kanun.enabled);
        assert!(!app.vcf.kick.enabled);
        assert!(!app.vcf.tanbura.enabled);
        assert_eq!(app.vcf.focus, VcfTarget::Bass);

        app.handle_command("vcf bass 900 0.65 3.5");
        assert!(app.vcf.bass.enabled);
        assert_eq!(app.vcf.bass.target, VcfTarget::Bass);
    }

    #[test]
    fn vcf_wave_is_named() {
        let _guard = session_test_lock();
        let (tx, _rx) = bounded(64);
        let mut app = App::new(tx);

        app.handle_command("vcf bass 900 0.65 3.5 wave=saw");
        assert!(app.vcf.bass.enabled);
        assert_eq!(app.vcf.bass.target, VcfTarget::Bass);
        assert_eq!(app.vcf.bass.cutoff_hz, 900.0);
        assert_eq!(app.vcf.bass.resonance, 0.65);
        assert_eq!(app.vcf.bass.drive, 3.5);
        assert_eq!(app.vcf.bass.wave, VcoWave::Saw);

        app.handle_command("vcf kanun cut=2400 res=0.35 drive=2.0 wave=tri");
        assert!(app.vcf.bass.enabled);
        assert!(app.vcf.kanun.enabled);
        assert!(!app.vcf.all.enabled);
        assert_eq!(app.vcf.kanun.target, VcfTarget::Kanun);
        assert_eq!(app.vcf.kanun.wave, VcoWave::Tri);

        app.handle_command("vcf drums cut=700 res=0.25 drive=2.5 wave=squ");
        assert!(app.vcf.bass.enabled);
        assert!(app.vcf.kanun.enabled);
        assert!(app.vcf.kick.enabled);
        assert_eq!(app.vcf.kick.target, VcfTarget::Kick);
        assert_eq!(app.vcf.kick.wave, VcoWave::Squ);
        assert_eq!(
            app.phrases.last().unwrap().src,
            "vcf drums cut 700 res 0.25 drive 2.5 wave squ"
        );

        app.handle_command("vcf mic cut=1800 res=0.2 drive=1.2 wave=sin");
        assert!(app.vcf.mic.enabled);
        assert_eq!(app.vcf.mic.target, VcfTarget::Mic);
        assert_eq!(app.vcf.mic.cutoff_hz, 1800.0);
        assert_eq!(app.vcf.mic.wave, VcoWave::Mic);

        app.handle_command("vcf mic cut 1200 res 0.6 drive 2 wave mic");
        assert!(app.vcf.mic.enabled);
        assert_eq!(app.vcf.mic.cutoff_hz, 1200.0);
        assert_eq!(app.vcf.mic.resonance, 0.6);
        assert_eq!(app.vcf.mic.drive, 2.0);
        assert_eq!(app.vcf.mic.wave, VcoWave::Mic);
        assert_eq!(
            app.phrases.last().unwrap().src,
            "vcf mic cut 1200 res 0.6 drive 2"
        );

        app.handle_command("vcf sym cut=1800 res=0.7 drive=1.5");
        assert!(app.vcf.tanbura.enabled);
        assert_eq!(app.vcf.tanbura.target, VcfTarget::Tanbura);
        assert_eq!(app.vcf.tanbura.cutoff_hz, 1800.0);
        assert_eq!(
            app.phrases.last().unwrap().src,
            "vcf sym cut 1800 res 0.7 drive 1.5"
        );

        app.handle_command("vcf bass off");
        assert!(!app.vcf.bass.enabled);
        assert!(app.vcf.kanun.enabled);
        assert!(app.vcf.kick.enabled);
        assert_eq!(app.vcf.focus, VcfTarget::Bass);

        app.handle_command("vcf all 1200 0.35 1.5 wave=saw");
        assert!(app.vcf.all.enabled);
        assert_eq!(app.vcf.all.wave, VcoWave::Sin);
        assert!(!app.vcf.mic.enabled);
        assert!(!app.vcf.bass.enabled);
        assert!(!app.vcf.kanun.enabled);
        assert!(!app.vcf.kick.enabled);
        assert!(!app.vcf.tanbura.enabled);
        assert_eq!(
            app.phrases.last().unwrap().src,
            "vcf all cut 1200 res 0.35 drive 1.5"
        );

        app.handle_command("vcf all off");
        assert!(!app.vcf.all.enabled);
        assert!(!app.vcf.mic.enabled);
        assert!(!app.vcf.bass.enabled);
        assert!(!app.vcf.kanun.enabled);
        assert!(!app.vcf.kick.enabled);
        assert!(!app.vcf.tanbura.enabled);
        assert_eq!(app.vcf.focus, VcfTarget::All);
    }

    #[test]
    fn vcf_relative_and_tick_changes_are_preserved() {
        let _guard = session_test_lock();
        let (tx, _rx) = bounded(16);
        let mut app = App::new(tx);

        app.handle_command("vcf bass 900 0.65 3.5 wave=saw");
        app.handle_command("vcf bass cut -100");
        assert_eq!(app.vcf.bass.cutoff_hz, 800.0);
        assert_eq!(app.phrases.last().unwrap().src, "vcf bass cut -100");

        app.handle_command("vcf bass cut=+2t");
        assert_eq!(app.vcf.bass.cutoff_step_per_tick, 2.0);
        assert_eq!(app.phrases.last().unwrap().src, "vcf bass cut +2t");

        app.handle_command("vcf bass cut=+0");
        assert_eq!(app.vcf.bass.cutoff_step_per_tick, 0.0);
        assert_eq!(app.vcf.bass.cutoff_hz, 800.0);
    }

    #[test]
    fn fx_commands_use_vcf_style_parameter_rules() {
        let _guard = session_test_lock();
        let (tx, _rx) = bounded(16);
        let mut app = App::new(tx);

        app.handle_command("reverb mix=0.25 decay=0.7");
        assert!(app.fx.reverb_enabled);
        assert_eq!(app.fx.reverb_mix, 0.25);
        assert_eq!(app.fx.reverb_decay, 0.7);

        app.handle_command("pingpong time=0.33 feedback=0.45 mix=0.2");
        assert!(app.fx.delay_enabled);
        assert_eq!(app.fx.delay_time_secs, 0.33);
        assert_eq!(app.fx.delay_feedback, 0.45);
        assert_eq!(app.fx.delay_mix, 0.2);

        app.handle_command("delay mix=+0.1");
        assert_eq!(app.fx.delay_mix, 0.3);
        assert_eq!(app.phrases.last().unwrap().src, "delay mix +0.1");

        app.handle_command("delay feedback=+0.01t");
        assert_eq!(app.fx.delay_feedback_step_per_tick, 0.01);
        assert_eq!(app.phrases.last().unwrap().src, "delay feedback +0.01t");

        app.handle_command("fx off");
        assert!(!app.fx.reverb_enabled);
        assert!(!app.fx.delay_enabled);
    }

    #[test]
    fn jump_times_one_is_noop_and_not_added() {
        let (tx, _rx) = bounded(16);
        let mut app = App::new(tx);
        app.handle_command("d bayati 4444");

        app.handle_command("j 0 1");

        assert_eq!(app.phrases.len(), 1);
        assert_eq!(
            app.message.as_deref(),
            Some("✗ jump ×1 is a no-op; use j <id> 2 or omit the jump")
        );
    }

    #[test]
    fn load_tab_completion_lists_and_completes_mq_files() {
        let _guard = session_test_lock();
        let (tx, _rx) = bounded(16);
        let mut app = App::new(tx);
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("maqam-complete-{suffix}"));
        fs::create_dir(&root).unwrap();
        let old_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&root).unwrap();
        struct CwdGuard(PathBuf);
        impl Drop for CwdGuard {
            fn drop(&mut self) {
                let _ = std::env::set_current_dir(&self.0);
            }
        }
        let _cwd_guard = CwdGuard(old_cwd);
        fs::write("alpha.mq", "MAQAM_SESSION_V3\n").unwrap();
        fs::write("alpine.mq", "MAQAM_SESSION_V3\n").unwrap();
        fs::create_dir("sets").unwrap();
        fs::write("sets/alphaDeep.mq", "MAQAM_SESSION_V3\n").unwrap();

        app.input = "load al".to_string();
        app.cursor_pos = app.input.chars().count();
        app.complete_input();
        assert_eq!(app.input, "load alp");
        assert_eq!(
            app.message.as_deref(),
            Some("load: alpha.mq  alpine.mq  sets/alphaDeep.mq")
        );

        app.complete_input();
        assert_eq!(app.input, "load alp");
        assert_eq!(
            app.message.as_deref(),
            Some("load: alpha.mq  alpine.mq  sets/alphaDeep.mq")
        );

        app.input = "load alphaD".to_string();
        app.cursor_pos = app.input.chars().count();
        app.complete_input();
        assert_eq!(app.input, "load sets/alphaDeep.mq");
        assert!(app.message.is_none());
    }

    #[test]
    fn edit_tab_completion_fills_current_timeline_value() {
        let (tx, _rx) = bounded(16);
        let mut app = App::new(tx);

        app.handle_command("d bayati 332 r3");
        app.input = "edit 0".to_string();
        app.cursor_pos = app.input.chars().count();
        app.complete_input();
        assert_eq!(app.input, "edit 0 d bayati 332 r3");
        assert!(app.message.is_none());

        app.handle_command("vcf bass cut=900 res=0.65");
        app.input = "edit 1 ".to_string();
        app.cursor_pos = app.input.chars().count();
        app.complete_input();
        assert_eq!(app.input, "edit 1 vcf bass cut 900 res 0.65");
        assert!(app.message.is_none());
    }

    #[test]
    fn command_metadata_drives_parameter_completion() {
        let (tx, _rx) = bounded(16);
        let mut app = App::new(tx);

        app.input = "vcf".to_string();
        app.cursor_pos = app.input.chars().count();
        app.complete_input();
        assert_eq!(app.input, "vcf cut ");
        assert!(app.message.is_none());

        app.input = "vcf mic ".to_string();
        app.cursor_pos = app.input.chars().count();
        app.complete_input();
        assert_eq!(app.input, "vcf mic cut ");

        app.input = "vcf mic cut ".to_string();
        app.cursor_pos = app.input.chars().count();
        app.complete_input();
        assert_eq!(app.input, "vcf mic cut ");
        assert_eq!(
            app.message.as_deref(),
            Some("vcf cut <10..22000 Hz|+n|-n|+nt>")
        );

        app.input = "vcf bass wave s".to_string();
        app.cursor_pos = app.input.chars().count();
        app.complete_input();
        assert_eq!(app.input, "vcf bass wave sin ");
        assert!(app.message.is_none());

        app.input = "i 4 vcf bass ".to_string();
        app.cursor_pos = app.input.chars().count();
        app.complete_input();
        assert_eq!(app.input, "i 4 vcf bass cut ");

        app.input = "edit 4 sym mic ".to_string();
        app.cursor_pos = app.input.chars().count();
        app.complete_input();
        assert_eq!(app.input, "edit 4 sym mic decay ");
    }

    #[test]
    fn llm_missing_key_error_tells_user_what_to_do() {
        let _guard = session_test_lock();
        let old_key = std::env::var("OPENAI_API_KEY").ok();
        std::env::remove_var("OPENAI_API_KEY");

        let (tx, _rx) = bounded(16);
        let mut app = App::new(tx);
        app.handle_command("chatgpt: what is a jins?");

        assert_eq!(
            app.message.as_deref(),
            Some("✗ environment variable OPENAI_API_KEY needs to be set to talk to chatgpt")
        );

        if let Some(key) = old_key {
            std::env::set_var("OPENAI_API_KEY", key);
        }
    }

    #[test]
    fn llm_edit_intent_requires_prefixed_llm_prompt() {
        assert!(llm_prompt_is_edit_request(
            "let's do an e minor that does a d major hemiola turnaround"
        ));
        assert!(llm_prompt_is_edit_request("add in sympathetics"));
        assert!(llm_prompt_is_edit_request("how do i get sympathetics?"));
        assert!(!llm_prompt_is_edit_request("can i have NAM A2?"));
        assert!(llm_prompt_is_edit_request("add NAM A2 on input"));
        assert!(!llm_prompt_is_edit_request(
            "what are the valid values for sym decay?"
        ));

        let (tx, _rx) = bounded(16);
        let mut app = App::new(tx);
        app.handle_command("let's do an e minor that does a d major hemiola turnaround");
        assert!(app
            .message
            .as_deref()
            .is_some_and(|message| message.starts_with("✗ unknown pitch")));
    }

    #[test]
    fn llm_edit_commands_reject_save() {
        let commands = extract_tool_commands(&serde_json::json!({
            "commands": ["e minor 332", "save default.mq"]
        }))
        .unwrap();
        assert_eq!(commands, vec!["e minor 332", "save default.mq"]);

        let parsed = command::parse("save default.mq").unwrap();
        assert!(!llm_edit_command_allowed(&parsed));
        let parsed = command::parse("e minor 332").unwrap();
        assert!(llm_edit_command_allowed(&parsed));

        let (tx, _rx) = bounded(16);
        let mut app = App::new(tx);
        app.apply_llm_edit_commands(commands);
        assert!(app.phrases.is_empty());
        assert!(app
            .message
            .as_deref()
            .is_some_and(|message| message.contains("cannot run save/load")));

        let commands = extract_tool_commands(&serde_json::json!({
            "commands": ["e minor 332; d major 332332; save default.mq"]
        }))
        .unwrap();
        assert_eq!(
            commands,
            vec!["e minor 332", "d major 332332", "save default.mq"]
        );
    }

    #[test]
    fn llm_edit_commands_reject_nam_as_live_state() {
        let commands = vec!["nam load metallica".to_string()];
        let (tx, _rx) = bounded(16);
        let mut app = App::new(tx);
        app.apply_llm_edit_commands(commands);
        assert!(app.phrases.is_empty());
        assert!(app.message.as_deref().is_some_and(|message| {
            message.contains("NAM is live input state")
                && message.contains("nam import FILENAME.nam as name")
        }));
    }

    #[test]
    fn llm_edit_tool_arguments_extract_commands() {
        let commands = extract_tool_commands(&serde_json::json!({
            "commands": ["e minor 4444", "d major 332332"]
        }))
        .unwrap();

        assert_eq!(commands, vec!["e minor 4444", "d major 332332"]);

        let err = extract_tool_commands(&serde_json::json!({"commands": []}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("edit tool returned no commands"));
    }

    #[test]
    fn llm_edit_tool_splits_repeated_sym_command_noun() {
        let commands = extract_tool_commands(&serde_json::json!({
            "commands": ["sym on sym decay 0.999 drive 2"]
        }))
        .unwrap();

        assert_eq!(commands, vec!["sym on", "sym decay 0.999 drive 2"]);
    }

    #[test]
    fn llm_system_prompt_specifies_command_language() {
        let prompt = llm_system_prompt("0: d bayati 4444\n1: vcf mic cut 1200 res 0.6");
        let reference = command::language_reference();
        assert!(prompt.contains("bpm 180"));
        assert!(prompt.contains("never `set tempo 180`"));
        assert!(prompt.contains("`<root> <jins> [rhythm]`"));
        assert!(prompt.contains("there is no time signature setting"));
        assert!(prompt.contains("grouping rhythm chunks"));
        assert!(prompt.contains("for 7/8 use 43"));
        assert!(prompt.contains("4433 is two 7/8 bars"));
        assert!(prompt.contains("16 bars in 7/8 is usually `<root> <jins> 43 r16`"));
        assert!(prompt.contains("use r8"));
        assert!(prompt.contains("use jumps only to restart a multi-row section"));
        assert!(prompt.contains("do not use a jump where one phrase repeat like r16"));
        assert!(prompt.contains("jump times count passes through the jump row"));
        assert!(prompt.contains("for a restart after 16 bars"));
        assert!(prompt.contains("count 16 steps in time, not 16 phrase rows"));
        assert!(prompt.contains("a jump with times 1 is a no-op"));
        assert!(prompt.contains("do not generate `j <id> 1`"));
        assert!(prompt.contains("about a dozen timeline rows"));
        assert!(prompt.contains("sym gain <0..512>"));
        assert!(prompt.contains("if the user says `add in sympathetics`, make the edit"));
        assert!(prompt.contains("sym decay 0.999 drive 2 kanun 0.5 bass 0.5"));
        assert!(prompt.contains("practical edit values are usually 0.5..8"));
        assert!(prompt.contains("values above 16 are extreme"));
        assert!(prompt.contains("amount/source sends have hard range 0..512"));
        assert!(prompt.contains("VCF"));
        assert!(prompt.contains("Current score context:"));
        assert!(prompt.contains("0: d bayati 4444"));
        assert!(prompt.contains("1: vcf mic cut 1200 res 0.6"));
        assert!(prompt.contains("LLM behavior:"));
        assert!(
            prompt.contains("prior user prompts and prior assistant answers/tool-command results")
        );
        assert!(reference.contains("Nouns:"));
        assert!(reference.contains("filter cutoff frequency"));
        assert!(reference.contains("sympathetic resonator excitation drive"));
        assert!(prompt.contains("Never return save"));
    }

    #[test]
    fn llm_message_payloads_include_history_and_current_prompt() {
        let history = vec![
            LlmChatMessage {
                role: LlmRole::User,
                content: "what is sym?".into(),
            },
            LlmChatMessage {
                role: LlmRole::Assistant,
                content: "sym controls sympathetics".into(),
            },
        ];

        let messages = openai_messages(&history, "how do i filter mic?", "0: d bayati 4444");
        assert_eq!(
            messages[0].pointer("/role").and_then(|v| v.as_str()),
            Some("system")
        );
        assert!(messages[0]
            .pointer("/content")
            .and_then(|v| v.as_str())
            .is_some_and(|content| content.contains("0: d bayati 4444")));
        assert_eq!(
            messages[1].pointer("/role").and_then(|v| v.as_str()),
            Some("user")
        );
        assert_eq!(
            messages[1].pointer("/content").and_then(|v| v.as_str()),
            Some("what is sym?")
        );
        assert_eq!(
            messages[2].pointer("/role").and_then(|v| v.as_str()),
            Some("assistant")
        );
        assert_eq!(
            messages[3].pointer("/content").and_then(|v| v.as_str()),
            Some("how do i filter mic?")
        );

        let anthropic = anthropic_messages(&history, "next question");
        assert_eq!(anthropic.len(), 3);
        assert_eq!(
            anthropic[2].pointer("/content").and_then(|v| v.as_str()),
            Some("next question")
        );
    }

    #[test]
    fn llm_answer_cleanup_preserves_response_lines() {
        assert_eq!(
            clean_llm_answer(" first   line\n\n second   line "),
            "first line\nsecond line"
        );
    }

    #[test]
    fn nam_search_helpers_extract_real_links() {
        let html = r#"
            <a class="result__a" href="/l/?uddg=https%3A%2F%2Ftonehunt.org%2Fmodels%2Fabc&amp;rut=x">
                Metallica NAM capture
            </a>
            <a href="https://example.com/manual">Ignore me</a>
            <a href="https://example.com/model.nam">Direct NAM</a>
        "#;

        let links = extract_search_links(html, 8);

        assert_eq!(
            links,
            vec![
                SearchResult {
                    title: "Metallica NAM capture".into(),
                    url: "https://tonehunt.org/models/abc".into(),
                },
                SearchResult {
                    title: "Direct NAM".into(),
                    url: "https://example.com/model.nam".into(),
                },
            ]
        );
        assert_eq!(
            url_query_encode("Metallica Mark IIC+ NAM"),
            "Metallica+Mark+IIC%2B+NAM"
        );
    }

    #[test]
    fn nam_search_extracts_direct_links_from_result_pages() {
        let html = r#"
            <a href="/downloads/mesa.nam">Mesa</a>
            <a href="//cdn.example.test/amps/5150.nam?download=1">5150</a>
        "#;

        let links = extract_direct_nam_links_from_html(html, "https://example.test/models/abc", 8);

        assert_eq!(
            links,
            vec![
                SearchResult {
                    title: "mesa.nam".into(),
                    url: "https://example.test/downloads/mesa.nam".into(),
                },
                SearchResult {
                    title: "5150.nam?download=1".into(),
                    url: "https://cdn.example.test/amps/5150.nam?download=1".into(),
                },
            ]
        );
    }

    #[test]
    fn nam_research_is_automatic_for_amp_discovery_prompts() {
        assert!(llm_prompt_needs_nam_research(
            "find me a Metallica NAM amp capture"
        ));
        assert!(llm_prompt_needs_nam_research(
            "where can i get a Mesa amp model?"
        ));
        assert!(!llm_prompt_needs_nam_research("how do i turn NAM off?"));
        assert!(!llm_prompt_needs_nam_research("how do i set vcf mic?"));
    }

    #[test]
    fn nam_tool_requires_a_query() {
        let err = execute_llm_tool("find_nam_captures", &serde_json::json!({}))
            .expect_err("missing query should be rejected before network access");

        assert!(err.contains("needs a non-empty query"));
    }

    #[test]
    fn llm_history_remembers_exact_sent_prompt_and_returned_commands() {
        let (tx, _rx) = bounded(16);
        let mut app = App::new(tx);
        let sent = llm_edit_prompt("make a d major turnaround");
        let returned = ["d major 332".to_string(), "g major 332".to_string()];

        assert!(sent.contains("make the change with the apply_maqam_commands tool"));
        assert!(sent.contains("Never concatenate two commands without a separator"));

        app.remember_llm_exchange(sent.clone(), returned.join("\n"));

        assert_eq!(app.llm_history.len(), 2);
        assert_eq!(app.llm_history[0].content, sent);
        assert_eq!(app.llm_history[1].content, "d major 332\ng major 332");
    }

    #[test]
    fn llm_score_context_reports_time_steps_not_just_phrase_rows() {
        let (tx, _rx) = bounded(16);
        let mut app = App::new(tx);
        app.handle_command("d bayati 43 r16");

        let context = app.llm_score_context();

        assert!(context.contains("0: d bayati 43 r16"));
        assert!(context.contains("time steps 0..15"));
        assert!(context.contains("16 repeat pass(es)"));
        assert!(context.contains("rhythm groups 43"));
        assert!(context.contains("7 subdivisions per pass"));
    }

    #[test]
    fn llm_repeated_phrase_suggestions_are_minimized_compactly() {
        let minimized = minimize_repeated_llm_phrase_commands(
            vec![
                "e minor 43".into(),
                "e minor 43".into(),
                "e minor 43".into(),
                "e minor 43".into(),
            ],
            7,
        );
        assert_eq!(minimized, vec!["e minor 43 r4"]);

        let minimized = minimize_repeated_llm_phrase_commands(
            vec![
                "e minor 332".into(),
                "d major 332332".into(),
                "e minor 332".into(),
                "d major 332332".into(),
                "e minor 332".into(),
                "d major 332332".into(),
            ],
            7,
        );

        assert_eq!(minimized, vec!["e minor 332", "d major 332332", "j 7 3"]);

        let minimized = minimize_repeated_llm_phrase_commands(
            vec![
                "bpm 140".into(),
                "e minor 332".into(),
                "d major 332332".into(),
                "e minor 332".into(),
                "d major 332332".into(),
            ],
            7,
        );
        assert_eq!(
            minimized,
            vec!["bpm 140", "e minor 332", "d major 332332", "j 8 2"]
        );

        let untouched = minimize_repeated_llm_phrase_commands(
            vec![
                "e minor 332".into(),
                "bpm 140".into(),
                "e minor 332".into(),
                "bpm 140".into(),
            ],
            0,
        );
        assert_eq!(
            untouched,
            vec!["e minor 332", "bpm 140", "e minor 332", "bpm 140"]
        );
    }

    #[test]
    fn llm_edit_rolls_back_when_an_applied_command_fails() {
        let (tx, _rx) = bounded(64);
        let mut app = App::new(tx);
        app.handle_command("d bayati 4444");
        let original = app
            .phrases
            .iter()
            .map(|phrase| phrase.display_src())
            .collect::<Vec<_>>();

        app.apply_llm_edit_commands(vec!["e minor 332".into(), "edit 999 d major 332".into()]);

        assert_eq!(
            app.phrases
                .iter()
                .map(|phrase| phrase.display_src())
                .collect::<Vec<_>>(),
            original
        );
        assert_eq!(app.next_phrase_id, 1);
        assert!(app
            .message
            .as_deref()
            .is_some_and(|message| message.contains("restored the previous phrases")));
    }

    #[test]
    fn phrase_completion_uses_current_phrase_transition_rules() {
        let (tx, _rx) = bounded(16);
        let mut app = App::new(tx);

        app.handle_command("d bayati 4444");
        app.input = "c ".to_string();
        app.cursor_pos = app.input.chars().count();
        app.complete_input();
        assert_eq!(app.input, "c rast 4444");

        app.handle_command("e minor 332 r2");
        app.input = "g ".to_string();
        app.cursor_pos = app.input.chars().count();
        app.complete_input();
        assert_eq!(app.input, "g major 332 r2");
    }

    #[test]
    fn loads_rewritten_v1_session_with_custom_jins() {
        let _guard = session_test_lock();
        let (tx, _rx) = bounded(32);
        let mut app = App::new(tx);
        app.load_session_v1(
            [
                "create saba2 1/1 13/12 6/5 5/4",
                "vol 1",
                "bpm 180",
                "s 2",
                "P|2|1|d bayati, f hijaz 4444",
                "J|3|0|3",
                "P|4|1|a saba, c hijaz",
                "P|5|1|a saba2, c hijaz",
                "J|6|4|4",
                "P|7|1|g rast 664664",
                "J|8|7|4",
            ]
            .into_iter(),
        )
        .unwrap();

        assert_eq!(app.phrases.len(), 9);
        assert_eq!(app.next_phrase_id, 9);
    }

    #[test]
    fn bundled_v3_sessions_load() {
        let _guard = session_test_lock();

        for name in ["magiccarpet.mq", "growl.mq"] {
            let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(name);
            let source = fs::read_to_string(&path).unwrap();
            assert!(source.starts_with("MAQAM_SESSION_V3\n"), "{name} is not V3");

            let (tx, _rx) = bounded(32);
            let mut app = App::new(tx);
            app.load_session(path.to_str().unwrap())
                .unwrap_or_else(|error| panic!("{name} failed to load: {error}"));

            assert!(!app.phrases.is_empty(), "{name} loaded no timeline entries");
            assert!(app
                .phrases
                .iter()
                .all(|phrase| phrase.id < app.next_phrase_id));
        }
    }

    #[test]
    fn recording_errors_appear_in_response_area() {
        let (audio_tx, _audio_rx) = bounded(1);
        let mut app = App::new(audio_tx);
        let (result_tx, result_rx) = bounded(1);
        app.rec_rx = Some(result_rx);
        result_tx
            .send(Err("generated source background failed".to_string()))
            .unwrap();

        app.tick();

        assert_eq!(
            app.message.as_deref(),
            Some("✗ generated source background failed")
        );
        assert!(app.rec_rx.is_none());
    }

    #[test]
    #[ignore]
    fn offline_carpet_video_smoke_test() {
        let _guard = session_test_lock();
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("magiccarpet.mq");
        let (tx, _rx) = bounded(32);
        let mut app = App::new(tx);
        app.load_session(path.to_str().unwrap()).unwrap();
        let (bpm, sustain, vcf, fx) = app.sequence_start_settings();
        let output =
            crate::record::record_cycle(app.phrases.clone(), bpm, sustain, vcf, fx, 1).unwrap();
        assert!(Path::new(&output).exists());
        let _ = fs::remove_file(output);
    }
}
