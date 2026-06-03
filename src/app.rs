// app.rs — application state, phrases as top-level units

use crossbeam_channel::Sender;
use std::fs;
use std::path::{Path, PathBuf};
use crate::command::{self, Cmd, JinsSpec, ValueChange};
use crate::record;
use crate::sequencer::{build_control_entry, build_phrase, AudioCmd, BarSpec, ControlSpec, Phrase};

pub struct App {
    pub phrases:     Vec<Phrase>,
    pub input:       String,
    pub message:     Option<String>,
    pub show_help:    bool,
    pub show_jins:    bool,
    pub help_scroll:  u16,
    pub jins_scroll:  u16,
    pub bpm:         f64,
    pub sustain:     f64,
    pub vol:         f32,
    pub paused:      bool,
    pub should_quit:    bool,
    pub last_recording: Option<String>,
    pub history:     Vec<String>,
    pub history_pos: Option<usize>,
    pub saved_input: String,
    pub cursor_pos:  usize,
    pub rec_rx:      Option<crossbeam_channel::Receiver<Result<String, String>>>,
    session_path:    Option<String>,
    next_phrase_id:  usize,
    last_rhythm:     Vec<u8>,
    auditioning_jins: bool,
    audio_tx:        Sender<AudioCmd>,
}

impl App {
    pub fn new(audio_tx: Sender<AudioCmd>) -> Self {
        App {
            phrases:        Vec::new(),
            input:          String::new(),
            message:        Some("? for help".into()),
            show_help:      false,
            show_jins:      false,
            help_scroll:    0,
            jins_scroll:    0,
            bpm:            120.0,
            sustain:        1.25,
            vol:            1.0,
            paused:         false,
            should_quit:    false,
            last_recording: None,
            history:        Vec::new(),
            history_pos:    None,
            saved_input:    String::new(),
            cursor_pos:     0,
            rec_rx:         None,
            session_path:   None,
            next_phrase_id: 0,
            last_rhythm:    vec![3, 3, 2],
            auditioning_jins: false,
            audio_tx,
        }
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

    pub fn history_up(&mut self) {
        if self.history.is_empty() { return; }
        match self.history_pos {
            None    => { self.saved_input = self.input.clone();
                         self.history_pos = Some(self.history.len() - 1); }
            Some(0) => {}
            Some(i) => { self.history_pos = Some(i - 1); }
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
                self.input       = self.saved_input.clone();
                self.cursor_pos  = self.input.chars().count();
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
        if self.cursor_pos > 0 { self.cursor_pos -= 1; }
    }

    pub fn cursor_right(&mut self) {
        let n = self.input.chars().count();
        if self.cursor_pos < n { self.cursor_pos += 1; }
    }

    pub fn cursor_home(&mut self) { self.cursor_pos = 0; }

    pub fn cursor_end(&mut self) {
        self.cursor_pos = self.input.chars().count();
    }

    pub fn insert_char(&mut self, ch: char) {
        let byte_pos: usize = self.input.char_indices()
            .nth(self.cursor_pos)
            .map(|(b, _)| b)
            .unwrap_or(self.input.len());
        self.input.insert(byte_pos, ch);
        self.cursor_pos += 1;
    }

    pub fn backspace(&mut self) {
        if self.cursor_pos == 0 { return; }
        let byte_pos: usize = self.input.char_indices()
            .nth(self.cursor_pos - 1)
            .map(|(b, _)| b)
            .unwrap_or(self.input.len().saturating_sub(1));
        self.input.remove(byte_pos);
        self.cursor_pos -= 1;
    }

    pub fn delete_char(&mut self) {
        let n = self.input.chars().count();
        if self.cursor_pos >= n { return; }
        let byte_pos: usize = self.input.char_indices()
            .nth(self.cursor_pos)
            .map(|(b, _)| b)
            .unwrap_or(self.input.len());
        self.input.remove(byte_pos);
    }

    pub fn complete_input(&mut self) {
        let Some((cmd, arg_start, partial)) = completion_target(&self.input) else { return; };
        let matches = mq_matches(&partial);
        if matches.is_empty() {
            self.message = Some("✗ no .mq matches".into());
            return;
        }

        let common = longest_common_prefix(&matches);
        let replacement = if matches.len() == 1 {
            matches[0].clone()
        } else if common.len() > partial.len() {
            common
        } else {
            self.message = Some(format!("{}: {}", cmd, matches.join("  ")));
            return;
        };

        self.input.replace_range(arg_start..self.input.len(), &replacement);
        self.cursor_pos = self.input.chars().count();
        self.message = None;
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

    // ── Render thread poll ────────────────────────────────────────────────

    pub fn tick(&mut self) {
        if let Some(rx) = &self.rec_rx {
            if let Ok(result) = rx.try_recv() {
                match result {
                    Ok(path) => {
                        self.last_recording = Some(path.clone());
                        self.message        = Some(format!("saved → {path}"));
                    }
                    Err(e) => {
                        self.message = Some(format!("✗ {e}"));
                    }
                }
                self.rec_rx = None;
            }
        }
    }

    fn resync_audio_sequence(&mut self, focus_id: Option<usize>) {
        let target_pos = focus_id.and_then(|id| self.phrases.iter().position(|p| p.id == id)).unwrap_or(0);
        let _ = self.audio_tx.send(AudioCmd::Clear);
        let _ = self.audio_tx.send(AudioCmd::SetBpm(self.bpm));
        let _ = self.audio_tx.send(AudioCmd::SetSustain(self.sustain));
        let _ = self.audio_tx.send(AudioCmd::SetVol(self.vol));
        let _ = self.audio_tx.send(AudioCmd::SetPaused(self.paused));
        for p in self.phrases.iter().cloned() {
            let _ = self.audio_tx.send(AudioCmd::AddPhrase(p));
        }
        if !self.phrases.is_empty() {
            let _ = self.audio_tx.send(AudioCmd::SetCurPhrase(target_pos.min(self.phrases.len() - 1)));
        }
        self.auditioning_jins = false;
    }

    fn resolve_id_ref(&self, id_ref: isize) -> Option<usize> {
        resolve_id_ref_in_phrases(&self.phrases, id_ref)
    }

    fn audition_jins(&mut self, name: &str) -> Result<(), String> {
        let maqam = crate::tuning::Maqam::parse(name)
            .ok_or_else(|| format!("unknown jins '{name}'"))?;
        let display_name = maqam.name().to_string();
        let root = crate::tuning::Pitch { letter: 'd', accidental: 0, octave: 4 };
        let spec = BarSpec {
            src: format!("d {display_name}"),
            root,
            maqam,
            groups: vec![1],
        };
        let mut phrase = build_phrase(usize::MAX, format!("[preview] d {display_name}"), vec![spec], 4, 1);
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
            if part.is_empty() { continue; }
            match command::parse(part) {
                Ok(cmd)  => self.execute(cmd),
                Err(msg) => { self.message = Some(format!("✗ {msg}")); return; }
            }
        }
    }

    fn execute(&mut self, cmd: Cmd) {
        let keep_audition = matches!(&cmd, Cmd::CreateJins { .. } | Cmd::AuditionJins { .. } | Cmd::Help | Cmd::ListJins);
        if self.auditioning_jins && !keep_audition {
            self.resync_audio_sequence(None);
        }
        match cmd {
            Cmd::Quit  => self.should_quit = true,
            Cmd::Help => { self.show_help = true; }
            Cmd::Jump { to, times } => {
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

            Cmd::Insert { before, specs, repeat } => {
                if specs.is_empty() {
                    self.message = Some("✗ empty phrase".into());
                    return;
                }
                let resolved = match resolve_rhythms(specs, &self.last_rhythm) {
                    Ok(r)  => r,
                    Err(e) => { self.message = Some(format!("✗ {e}")); return; }
                };
                if let Some(r) = resolved.last() { self.last_rhythm = r.groups.clone(); }
                let peak = 4usize;
                let src = resolved.iter().map(|s| s.src.as_str()).collect::<Vec<_>>().join(", ");
                let id  = self.next_phrase_id;
                self.next_phrase_id += 1;
                let phrase = build_phrase(id, src, resolved, peak, repeat.max(1));
                let pos = match self.resolve_id_ref(before) {
                    Some(before_id) => self.phrases.iter().position(|p| p.id == before_id).unwrap_or(self.phrases.len()),
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
                let Some(to) = self.resolve_id_ref(to) else {
                    self.message = Some(format!("✗ no phrase id {to}")); return;
                };
                if !self.phrases.iter().any(|p| p.id == to) {
                    self.message = Some(format!("✗ no phrase id {to}")); return;
                }
                let insert_pos = match self.resolve_id_ref(before) {
                    Some(before_id) => self.phrases.iter().position(|p| p.id == before_id).unwrap_or(self.phrases.len()),
                    None => {
                        self.message = Some(format!("✗ no phrase id {before}")); return;
                    }
                };
                let id    = self.next_phrase_id;
                self.next_phrase_id += 1;
                let entry = crate::sequencer::build_jump_entry(id, to, times);
                self.phrases.insert(insert_pos, entry.clone());
                let _ = self.audio_tx.send(AudioCmd::InsertPhrase { pos: insert_pos, phrase: entry });
                self.message = None;
            }

            Cmd::InsertBpm { before, change } => {
                let bpm = match apply_bpm_change(self.bpm, change) {
                    Ok(v) => v,
                    Err(e) => { self.message = Some(format!("✗ {e}")); return; }
                };
                let insert_pos = match self.resolve_id_ref(before) {
                    Some(before_id) => self.phrases.iter().position(|p| p.id == before_id).unwrap_or(self.phrases.len()),
                    None => {
                        self.message = Some(format!("✗ no phrase id {before}")); return;
                    }
                };
                let id = self.next_phrase_id;
                self.next_phrase_id += 1;
                let entry = build_control_entry(id, format!("bpm {bpm}"), ControlSpec::SetBpm(bpm));
                self.phrases.insert(insert_pos, entry.clone());
                let _ = self.audio_tx.send(AudioCmd::InsertPhrase { pos: insert_pos, phrase: entry });
                self.message = Some(format!("inserted bpm at {insert_pos}"));
            }

            Cmd::InsertSustain { before, change } => {
                let secs = match apply_sustain_change(self.sustain, change) {
                    Ok(v) => v,
                    Err(e) => { self.message = Some(format!("✗ {e}")); return; }
                };
                let insert_pos = match self.resolve_id_ref(before) {
                    Some(before_id) => self.phrases.iter().position(|p| p.id == before_id).unwrap_or(self.phrases.len()),
                    None => {
                        self.message = Some(format!("✗ no phrase id {before}")); return;
                    }
                };
                let id = self.next_phrase_id;
                self.next_phrase_id += 1;
                let entry = build_control_entry(id, format!("s {secs}"), ControlSpec::SetSustain(secs));
                self.phrases.insert(insert_pos, entry.clone());
                let _ = self.audio_tx.send(AudioCmd::InsertPhrase { pos: insert_pos, phrase: entry });
                self.message = Some(format!("inserted sustain at {insert_pos}"));
            }

            Cmd::TogglePause { start_id } => {
                if let Some(id) = start_id {
                    let Some(id) = self.resolve_id_ref(id) else {
                        self.message = Some(format!("✗ no phrase id {id}"));
                        return;
                    };
                    // z <id>: seek to phrase, no pause toggle
                    match self.phrases.iter().position(|p| p.id == id) {
                        Some(pos) => {
                            let _ = self.audio_tx.send(AudioCmd::SetCurPhrase(pos));
                            self.message = Some(format!("→ phrase {id}"));
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
                    self.message = Some(if self.paused { "⏸ paused".into() } else { "▶ playing".into() });
                }
            }

            Cmd::SetVol(v) => {
                self.vol = v;
                let _ = self.audio_tx.send(AudioCmd::SetVol(v));
                self.message = Some(format!("vol → {v:.2}"));
            }

            Cmd::Record(reps) => {
                if crate::REC_ACTIVE.load(std::sync::atomic::Ordering::Relaxed) {
                    self.message = Some("✗ already rendering".into());
                    return;
                }
                let phrases = self.phrases.clone();
                let bpm     = self.bpm;
                let sustain = self.sustain;
                let (tx, rx) = crossbeam_channel::bounded(1);
                self.rec_rx  = Some(rx);
                self.message = Some(format!("◉ rendering {}×…", reps));
                std::thread::spawn(move || {
                    let result = record::record_cycle(phrases, bpm, sustain, reps)
                        .map_err(|e| e.to_string());
                    let _ = tx.send(result);
                });
            }

            Cmd::Rotate => {
                if self.phrases.len() < 2 {
                    self.message = Some("nothing to rotate".into());
                } else {
                    let last = self.phrases.pop().unwrap();
                    self.phrases.insert(0, last);
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
                let focus_id = if self.paused {
                    Some(id)
                } else {
                    let cur_pos = crate::CUR_PHRASE.load(std::sync::atomic::Ordering::Relaxed);
                    self.phrases.get(cur_pos).map(|p| p.id).or(Some(id))
                };
                self.phrases.swap(pos - 1, pos);
                self.resync_audio_sequence(focus_id);
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
                let focus_id = if self.paused {
                    Some(id)
                } else {
                    let cur_pos = crate::CUR_PHRASE.load(std::sync::atomic::Ordering::Relaxed);
                    self.phrases.get(cur_pos).map(|p| p.id).or(Some(id))
                };
                self.phrases.swap(pos, pos + 1);
                self.resync_audio_sequence(focus_id);
                self.message = Some(format!("moved {id} down"));
            }

            Cmd::ListJins => { self.show_jins = true; }

            Cmd::AuditionJins { name } => {
                match self.audition_jins(&name) {
                    Ok(()) => self.message = Some(format!("auditioning {name}")),
                    Err(e) => self.message = Some(format!("✗ {e}")),
                }
            }

            Cmd::CreateJins { name, ratios } => {
                match crate::tuning::Maqam::create(&name, ratios) {
                    Ok(()) => self.message = Some(format!("created jins {name}")),
                    Err(e) => self.message = Some(format!("✗ {e}")),
                }
            }

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

            Cmd::Load { path } => {
                match self.load_session(&path) {
                    Ok(()) => {
                        self.session_path = Some(path.clone());
                        self.message = Some(format!("loaded session ← {path}"));
                    }
                    Err(e) => self.message = Some(format!("✗ load failed: {e}")),
                }
            }

            Cmd::Clear => {
                self.phrases.clear();
                self.next_phrase_id = 0;
                let _ = self.audio_tx.send(AudioCmd::Clear);
                self.message = Some("cleared".into());
            }
            Cmd::SetBpm(change) => {
                let bpm = match apply_bpm_change(self.bpm, change) {
                    Ok(v) => v,
                    Err(e) => { self.message = Some(format!("✗ {e}")); return; }
                };
                let id = self.next_phrase_id;
                self.next_phrase_id += 1;
                let entry = build_control_entry(id, format!("bpm {bpm}"), ControlSpec::SetBpm(bpm));
                self.phrases.push(entry.clone());
                self.bpm = bpm;
                let _ = self.audio_tx.send(AudioCmd::AddPhrase(entry));
                let _ = self.audio_tx.send(AudioCmd::SetBpm(bpm));
                self.message = Some(format!("BPM line → {bpm:.2}"));
            }
            Cmd::SetSustain(change) => {
                let secs = match apply_sustain_change(self.sustain, change) {
                    Ok(v) => v,
                    Err(e) => { self.message = Some(format!("✗ {e}")); return; }
                };
                let id = self.next_phrase_id;
                self.next_phrase_id += 1;
                let entry = build_control_entry(id, format!("s {secs}"), ControlSpec::SetSustain(secs));
                self.phrases.push(entry.clone());
                self.sustain = secs;
                let _ = self.audio_tx.send(AudioCmd::AddPhrase(entry));
                let _ = self.audio_tx.send(AudioCmd::SetSustain(secs));
                self.message = Some(format!("s line → {secs:.2}s"));
            }

            Cmd::EditJump { id, to, times } => {
                let Some(id) = self.resolve_id_ref(id) else {
                    self.message = Some(format!("✗ no phrase id {id}")); return;
                };
                let Some(to) = self.resolve_id_ref(to) else {
                    self.message = Some(format!("✗ no phrase id {to}")); return;
                };
                if !self.phrases.iter().any(|p| p.id == to) {
                    self.message = Some(format!("✗ no phrase id {to}")); return;
                }
                let cur_pos    = crate::CUR_PHRASE.load(std::sync::atomic::Ordering::Relaxed);
                let playing_id = self.phrases.get(cur_pos).map(|p| p.id);
                if playing_id == Some(id) {
                    self.message = Some(format!("✗ phrase {id} is playing — cannot edit"));
                    return;
                }
                let pos = match self.phrases.iter().position(|p| p.id == id) {
                    Some(p) => p,
                    None    => { self.message = Some(format!("✗ no phrase id {id}")); return; }
                };
                let mut entry = crate::sequencer::build_jump_entry(id, to, times);
                entry.id = id; // preserve the original id
                self.phrases[pos] = entry.clone();
                let _ = self.audio_tx.send(AudioCmd::ReplacePhrase(entry));
                self.message = Some(format!("edited {id} → jump to {to} ×{times}"));
            }

            Cmd::Edit { id, specs, repeat } => {
                let Some(id) = self.resolve_id_ref(id) else {
                    self.message = Some(format!("✗ no phrase id {id}")); return;
                };
                let cur_pos    = crate::CUR_PHRASE.load(std::sync::atomic::Ordering::Relaxed);
                let playing_id = self.phrases.get(cur_pos).map(|p| p.id);
                if playing_id == Some(id) {
                    self.message = Some(format!("✗ phrase {id} is playing — cannot edit"));
                    return;
                }
                let pos = match self.phrases.iter().position(|p| p.id == id) {
                    Some(p) => p,
                    None    => { self.message = Some(format!("✗ no phrase id {id}")); return; }
                };
                let resolved = match resolve_rhythms(specs, &self.last_rhythm) {
                    Ok(r)  => r,
                    Err(e) => { self.message = Some(format!("✗ {e}")); return; }
                };
                if let Some(r) = resolved.last() { self.last_rhythm = r.groups.clone(); }
                let src = resolved.iter().map(|s| s.src.as_str()).collect::<Vec<_>>().join(", ");
                let mut phrase = build_phrase(id, src, resolved, 4, repeat.max(1));
                phrase.id = id;
                self.phrases[pos] = phrase.clone();
                let _ = self.audio_tx.send(AudioCmd::ReplacePhrase(phrase));
                self.message = Some(format!("edited {id}"));
            }

            Cmd::EditBpm { id, change } => {
                let Some(id) = self.resolve_id_ref(id) else {
                    self.message = Some(format!("✗ no phrase id {id}")); return;
                };
                let cur_pos    = crate::CUR_PHRASE.load(std::sync::atomic::Ordering::Relaxed);
                let playing_id = self.phrases.get(cur_pos).map(|p| p.id);
                if playing_id == Some(id) {
                    self.message = Some(format!("✗ phrase {id} is playing — cannot edit"));
                    return;
                }
                let pos = match self.phrases.iter().position(|p| p.id == id) {
                    Some(p) => p,
                    None    => { self.message = Some(format!("✗ no phrase id {id}")); return; }
                };
                let bpm = match apply_bpm_change(self.bpm, change) {
                    Ok(v) => v,
                    Err(e) => { self.message = Some(format!("✗ {e}")); return; }
                };
                let entry = build_control_entry(id, format!("bpm {bpm}"), ControlSpec::SetBpm(bpm));
                self.phrases[pos] = entry.clone();
                let _ = self.audio_tx.send(AudioCmd::ReplacePhrase(entry));
                self.message = Some(format!("edited {id} → bpm {bpm:.2}"));
            }

            Cmd::EditSustain { id, change } => {
                let Some(id) = self.resolve_id_ref(id) else {
                    self.message = Some(format!("✗ no phrase id {id}")); return;
                };
                let cur_pos    = crate::CUR_PHRASE.load(std::sync::atomic::Ordering::Relaxed);
                let playing_id = self.phrases.get(cur_pos).map(|p| p.id);
                if playing_id == Some(id) {
                    self.message = Some(format!("✗ phrase {id} is playing — cannot edit"));
                    return;
                }
                let pos = match self.phrases.iter().position(|p| p.id == id) {
                    Some(p) => p,
                    None    => { self.message = Some(format!("✗ no phrase id {id}")); return; }
                };
                let secs = match apply_sustain_change(self.sustain, change) {
                    Ok(v) => v,
                    Err(e) => { self.message = Some(format!("✗ {e}")); return; }
                };
                let entry = build_control_entry(id, format!("s {secs}"), ControlSpec::SetSustain(secs));
                self.phrases[pos] = entry.clone();
                let _ = self.audio_tx.send(AudioCmd::ReplacePhrase(entry));
                self.message = Some(format!("edited {id} → s {secs:.2}s"));
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

            Cmd::AddPhrase { specs, repeat } => {
                if specs.is_empty() {
                    self.message = Some("✗ empty phrase".into());
                    return;
                }
                let resolved = match resolve_rhythms(specs, &self.last_rhythm) {
                    Ok(r)  => r,
                    Err(e) => { self.message = Some(format!("✗ {e}")); return; }
                };
                if let Some(r) = resolved.last() { self.last_rhythm = r.groups.clone(); }
                let peak: usize = if self.phrases.is_empty() { 4 } else {
                    let total: usize = self.phrases.iter()
                        .map(|p| p.bar.total_subdivs).sum();
                    let count = self.phrases.len().max(1);
                    (total / count / 2).clamp(2, 4)
                };
                let src = resolved.iter()
                    .map(|s| s.src.as_str())
                    .collect::<Vec<_>>().join(", ");
                let id     = self.next_phrase_id;
                self.next_phrase_id += 1;
                let phrase = build_phrase(id, src, resolved, peak, repeat.max(1));
                let _ = self.audio_tx.send(AudioCmd::AddPhrase(phrase.clone()));
                self.phrases.push(phrase);
                self.message = None;
            }
        }
    }

    fn save_session(&self, path: &str) -> Result<(), String> {
        let mut out = String::new();
        out.push_str("MAQAM_SESSION_V2\n");
        for (name, ratios) in crate::tuning::Maqam::list_custom() {
            let ratios_s = ratios.iter()
                .map(|(p, q)| format!("{p}/{q}"))
                .collect::<Vec<_>>()
                .join(" ");
            out.push_str(&format!("create {name} {ratios_s}\n"));
        }
        out.push_str(&format!("vol {}\n", self.vol));
        for p in &self.phrases {
            if let Some(j) = &p.jump {
                out.push_str(&format!("j {} {}\n", j.target_id, j.times));
            } else if let Some(ctrl) = p.control {
                match ctrl {
                    ControlSpec::SetBpm(v) => out.push_str(&format!("bpm {v}\n")),
                    ControlSpec::SetSustain(v) => out.push_str(&format!("s {v}\n")),
                }
            } else {
                if p.repeat > 1 {
                    out.push_str(&format!("{} r{}\n", p.src, p.repeat));
                } else {
                    out.push_str(&format!("{}\n", p.src));
                }
            }
        }
        fs::write(path, out).map_err(|e| e.to_string())
    }

    fn load_session(&mut self, path: &str) -> Result<(), String> {
        let src = fs::read_to_string(path).map_err(|e| e.to_string())?;
        let mut lines = src.lines();
        let Some(header) = lines.next() else { return Err("empty file".into()); };
        let header = header.trim();
        if header == "MAQAM_SESSION_V2" {
            return self.load_session_v2(lines);
        }
        if header == "MAQAM_SESSION_V1" {
            return self.load_session_v1(lines);
        }
        Err("bad header (expected MAQAM_SESSION_V2 or MAQAM_SESSION_V1)".into())
    }

    fn load_session_v1<'a, I>(&mut self, lines: I) -> Result<(), String>
    where
        I: Iterator<Item = &'a str>
    {
        let mut new_bpm = self.bpm;
        let mut new_sustain = self.sustain;
        let mut new_vol = self.vol;
        let mut loaded: Vec<Phrase> = Vec::new();
        let mut max_id = 0usize;
        let mut last_rhythm = vec![3, 3, 2];

        for (line_idx, raw_line) in lines.enumerate() {
            let line_no = line_idx + 2;
            let line = raw_line.trim();
            if line.is_empty() { continue; }

            if line.starts_with("bpm ") {
                let parsed = command::parse(line).map_err(|e| format!("line {line_no}: {e}"))?;
                let Cmd::SetBpm(change) = parsed else {
                    return Err(format!("line {line_no}: expected bpm line"));
                };
                new_bpm = apply_bpm_change(new_bpm, change)
                    .map_err(|e| format!("line {line_no}: {e}"))?;
                let entry = build_control_entry(max_id, format!("bpm {new_bpm}"), ControlSpec::SetBpm(new_bpm));
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
                let entry = build_control_entry(max_id, format!("s {new_sustain}"), ControlSpec::SetSustain(new_sustain));
                loaded.push(entry);
                max_id += 1;
                continue;
            }
            if let Some(v) = line.strip_prefix("vol ") {
                new_vol = v.trim().parse::<f32>()
                    .map_err(|_| format!("line {line_no}: bad volume"))?;
                if !(0.0..=2.0).contains(&new_vol) {
                    return Err(format!("line {line_no}: volume out of range"));
                }
                continue;
            }

            if let Some(payload) = line.strip_prefix("J|") {
                let mut parts = payload.splitn(3, '|');
                let id = parts.next().ok_or(format!("line {line_no}: missing jump id"))?
                    .parse::<usize>().map_err(|_| format!("line {line_no}: bad jump id"))?;
                let target = parts.next().ok_or(format!("line {line_no}: missing jump target"))?
                    .parse::<usize>().map_err(|_| format!("line {line_no}: bad jump target"))?;
                let times = parts.next().ok_or(format!("line {line_no}: missing jump times"))?
                    .parse::<usize>().map_err(|_| format!("line {line_no}: bad jump times"))?;
                max_id = max_id.max(id);
                loaded.push(crate::sequencer::build_jump_entry(id, target, times.max(1)));
                continue;
            }

            if let Some(payload) = line.strip_prefix("P|") {
                let mut parts = payload.splitn(3, '|');
                let id = parts.next().ok_or(format!("line {line_no}: missing phrase id"))?
                    .parse::<usize>().map_err(|_| format!("line {line_no}: bad phrase id"))?;
                let repeat = parts.next().ok_or(format!("line {line_no}: missing repeat"))?
                    .parse::<usize>().map_err(|_| format!("line {line_no}: bad repeat"))?;
                let src = parts.next().ok_or(format!("line {line_no}: missing phrase source"))?;
                let cmd_src = if repeat > 1 {
                    format!("{src} r{repeat}")
                } else {
                    src.to_string()
                };
                let parsed = command::parse(&cmd_src)
                    .map_err(|e| format!("line {line_no}: {e}"))?;
                let (specs, rep) = match parsed {
                    Cmd::AddPhrase { specs, repeat } => (specs, repeat),
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
        self.vol = new_vol;
        self.paused = false;

        let _ = self.audio_tx.send(AudioCmd::Clear);
        let _ = self.audio_tx.send(AudioCmd::SetBpm(self.bpm));
        let _ = self.audio_tx.send(AudioCmd::SetSustain(self.sustain));
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
        I: Iterator<Item = &'a str>
    {
        crate::tuning::Maqam::reset_to_defaults();
        let mut new_bpm = 120.0f64;
        let mut new_sustain = 1.25f64;
        let mut new_vol = 1.0f32;
        let mut loaded: Vec<Phrase> = Vec::new();
        let mut next_id = 0usize;
        let mut last_rhythm = vec![3, 3, 2];

        for (line_idx, raw_line) in lines.enumerate() {
            let line_no = line_idx + 2;
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') { continue; }
            let cmd = command::parse(line).map_err(|e| format!("line {line_no}: {e}"))?;
            match cmd {
                Cmd::SetBpm(change) => {
                    new_bpm = apply_bpm_change(new_bpm, change)
                        .map_err(|e| format!("line {line_no}: {e}"))?;
                    let entry = build_control_entry(next_id, format!("bpm {new_bpm}"), ControlSpec::SetBpm(new_bpm));
                    next_id += 1;
                    loaded.push(entry);
                }
                Cmd::SetSustain(change) => {
                    new_sustain = apply_sustain_change(new_sustain, change)
                        .map_err(|e| format!("line {line_no}: {e}"))?;
                    let entry = build_control_entry(next_id, format!("s {new_sustain}"), ControlSpec::SetSustain(new_sustain));
                    next_id += 1;
                    loaded.push(entry);
                }
                Cmd::SetVol(v) => {
                    new_vol = v;
                }
                Cmd::AddPhrase { specs, repeat } => {
                    let resolved = resolve_rhythms(specs, &last_rhythm)
                        .map_err(|e| format!("line {line_no}: {e}"))?;
                    if let Some(r) = resolved.last() {
                        last_rhythm = r.groups.clone();
                    }
                    let src = resolved.iter().map(|s| s.src.as_str()).collect::<Vec<_>>().join(", ");
                    let phrase = build_phrase(next_id, src, resolved, 4, repeat.max(1));
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
        self.vol = new_vol;
        self.paused = false;

        let _ = self.audio_tx.send(AudioCmd::Clear);
        let _ = self.audio_tx.send(AudioCmd::SetBpm(self.bpm));
        let _ = self.audio_tx.send(AudioCmd::SetSustain(self.sustain));
        let _ = self.audio_tx.send(AudioCmd::SetVol(self.vol));
        let _ = self.audio_tx.send(AudioCmd::SetPaused(false));
        for p in loaded {
            let _ = self.audio_tx.send(AudioCmd::AddPhrase(p));
        }
        let _ = self.audio_tx.send(AudioCmd::SetCurPhrase(0));
        Ok(())
    }
}

fn completion_target(input: &str) -> Option<(&str, usize, String)> {
    let trimmed = input.trim_start();
    let leading_ws = input.len().saturating_sub(trimmed.len());
    let (cmd, rest) = trimmed.split_once(char::is_whitespace).unwrap_or((trimmed, ""));
    if cmd != "save" && cmd != "load" { return None; }
    let rest_start = leading_ws + cmd.len();
    let arg = rest.trim_start();
    let arg_start = input.len().saturating_sub(arg.len());
    Some((cmd, arg_start.max(rest_start), arg.to_string()))
}

fn mq_matches(partial: &str) -> Vec<String> {
    let partial_path = Path::new(partial);
    let (dir, prefix) = match partial_path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => (
            PathBuf::from(parent),
            partial_path.file_name().and_then(|s| s.to_str()).unwrap_or(""),
        ),
        _ => (PathBuf::from("."), partial),
    };

    let mut matches: Vec<String> = fs::read_dir(&dir)
        .ok()
        .into_iter()
        .flat_map(|it| it.filter_map(Result::ok))
        .filter_map(|entry| {
            let path = entry.path();
            if entry.file_type().ok()?.is_dir() { return None; }
            if path.extension().and_then(|s| s.to_str()) != Some("mq") { return None; }
            let name = path.file_name()?.to_str()?;
            if !name.starts_with(prefix) { return None; }
            if dir == Path::new(".") {
                Some(name.to_string())
            } else {
                Some(dir.join(name).to_string_lossy().replace('\\', "/"))
            }
        })
        .collect();
    matches.sort();
    matches
}

fn longest_common_prefix(items: &[String]) -> String {
    let Some(first) = items.first() else { return String::new(); };
    let mut prefix = first.clone();
    for item in &items[1..] {
        let mut keep = 0usize;
        for (a, b) in prefix.chars().zip(item.chars()) {
            if a != b { break; }
            keep += a.len_utf8();
        }
        prefix.truncate(keep);
        if prefix.is_empty() { break; }
    }
    prefix
}

fn resolve_rhythms(specs: Vec<JinsSpec>, default: &[u8]) -> Result<Vec<BarSpec>, String> {
    let n = specs.len();
    let mut groups: Vec<Option<Vec<u8>>> = specs.iter().map(|s| s.groups.clone()).collect();
    let mut carry: Option<Vec<u8>> = None;
    for i in (0..n).rev() {
        if groups[i].is_some() { carry = groups[i].clone(); }
        else                   { groups[i] = carry.clone(); }
    }
    let fallback = default.to_vec();
    Ok(specs.into_iter().zip(groups)
        .map(|(spec, grp)| BarSpec {
            src:    spec.src,
            root:   spec.root,
            maqam:  spec.maqam,
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

fn resolve_id_ref_in_phrases(phrases: &[Phrase], id_ref: isize) -> Option<usize> {
    if id_ref >= 0 {
        return Some(id_ref as usize);
    }
    let back = id_ref.unsigned_abs();
    if back == 0 || back > phrases.len() {
        return None;
    }
    phrases.get(phrases.len() - back).map(|p| p.id)
}
