// app.rs — application state, phrases as top-level units

use crossbeam_channel::Sender;
use crate::command::{self, Cmd, JinsSpec};
use crate::record;
use crate::sequencer::{build_phrase, AudioCmd, BarSpec, Phrase};

pub struct App {
    pub phrases:     Vec<Phrase>,
    pub input:       String,
    pub message:     Option<String>,
    pub show_help:    bool,
    pub bpm:         f64,
    pub sustain:     f64,
    pub vol:         f32,
    pub paused:      bool,
    pub should_quit:    bool,
    pub last_recording: Option<String>,  // persists across commands
    pub history:     Vec<String>,
    pub history_pos: Option<usize>,
    pub saved_input: String,
    pub cursor_pos:  usize,   // char index into input
    next_phrase_id:  usize,
    last_rhythm:     Vec<u8>,
    audio_tx:        Sender<AudioCmd>,
}

impl App {
    pub fn new(audio_tx: Sender<AudioCmd>) -> Self {
        App {
            phrases:        Vec::new(),
            input:          String::new(),
            message:        Some("? for help".into()),
            show_help:      false,
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
            next_phrase_id: 0,
            last_rhythm:    vec![3, 3, 2],
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
        // Convert char-index to byte-index
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


    // ── Commands ──────────────────────────────────────────────────────────

    pub fn handle_command(&mut self, raw: &str) {
        // Semicolons act as line separators — execute each part in order.
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
        match cmd {
            Cmd::Quit  => self.should_quit = true,
            Cmd::Help => { self.show_help = true; }
            Cmd::Jump { to, times } => {
                // Verify target id exists
                if !self.phrases.iter().any(|p| p.id == to) {
                    self.message = Some(format!("✗ no phrase id {to}"));
                    return;
                }
                let id = self.next_phrase_id;
                self.next_phrase_id += 1;
                // Store target_id directly — audio thread resolves position at runtime
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

                // `before` is a stable phrase.id — find its list position
                let pos = self.phrases.iter().position(|p| p.id == before)
                    .unwrap_or(self.phrases.len());
                self.phrases.insert(pos, phrase.clone());

                // Insert without disrupting playback
                let _ = self.audio_tx.send(AudioCmd::InsertPhrase { pos, phrase });
                self.message = Some(format!("inserted at {pos}"));
            }

            Cmd::InsertJump { before, to, times } => {
                // Verify target exists
                if !self.phrases.iter().any(|p| p.id == to) {
                    self.message = Some(format!("✗ no phrase id {to}")); return;
                }
                let insert_pos = self.phrases.iter().position(|p| p.id == before)
                    .unwrap_or(self.phrases.len());

                let id    = self.next_phrase_id;
                self.next_phrase_id += 1;
                // Store target phrase id — audio thread resolves position
                let entry = crate::sequencer::build_jump_entry(id, to, times);

                self.phrases.insert(insert_pos, entry.clone());
                // Insert without disrupting playback
                let _ = self.audio_tx.send(AudioCmd::InsertPhrase { pos: insert_pos, phrase: entry });
                self.message = None;
            }

            Cmd::TogglePause => {
                self.paused = !self.paused;
                let _ = self.audio_tx.send(AudioCmd::SetPaused(self.paused));
                self.message = Some(if self.paused { "⏸ paused".into() } else { "▶ playing".into() });
            }

            Cmd::SetVol(v) => {
                self.vol = v;
                let _ = self.audio_tx.send(AudioCmd::SetVol(v));
                self.message = Some(format!("vol → {v:.2}"));
            }

            Cmd::Record(reps) => {
                self.message = Some(format!("rendering {}×…", reps));
                let phrases = self.phrases.clone();
                let bpm     = self.bpm;
                let sustain = self.sustain;
                match record::record_cycle(&phrases, bpm, sustain, reps) {
                    Ok(path) => {
                        self.last_recording = Some(path.clone());
                        self.message = Some(format!("saved → {path}"));
                    }
                    Err(e) => self.message = Some(format!("✗ {e}")),
                }
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

            Cmd::Clear => {
                self.phrases.clear();
                let _ = self.audio_tx.send(AudioCmd::Clear);
                self.message = Some("cleared".into());
            }
            Cmd::SetBpm(bpm) => {
                self.bpm = bpm;
                let _ = self.audio_tx.send(AudioCmd::SetBpm(bpm));
                self.message = Some(format!("BPM → {bpm}"));
            }
            Cmd::SetSustain(secs) => {
                self.sustain = secs;
                let _ = self.audio_tx.send(AudioCmd::SetSustain(secs));
                self.message = Some(format!("sustain → {secs:.2}s"));
            }

            Cmd::Edit { id, specs, repeat } => {
                // Block editing the currently playing phrase
                let cur_pos    = crate::CUR_PHRASE.load(std::sync::atomic::Ordering::Relaxed);
                let playing_id = self.phrases.get(cur_pos).map(|p| p.id);
                if playing_id == Some(id) {
                    self.message = Some(format!("✗ phrase {id} is playing — cannot edit"));
                    return;
                }
                // Find the phrase
                let pos = match self.phrases.iter().position(|p| p.id == id) {
                    Some(p) => p,
                    None    => { self.message = Some(format!("✗ no phrase id {id}")); return; }
                };
                if self.phrases[pos].jump.is_some() {
                    self.message = Some(format!("✗ id {id} is a jump entry — use x then j"));
                    return;
                }
                let resolved = match resolve_rhythms(specs, &self.last_rhythm) {
                    Ok(r)  => r,
                    Err(e) => { self.message = Some(format!("✗ {e}")); return; }
                };
                if let Some(r) = resolved.last() { self.last_rhythm = r.groups.clone(); }
                let src = resolved.iter().map(|s| s.src.as_str()).collect::<Vec<_>>().join(", ");
                let mut phrase = build_phrase(id, src, resolved, 4, repeat.max(1));
                phrase.id = id; // preserve original id
                self.phrases[pos] = phrase.clone();
                let _ = self.audio_tx.send(AudioCmd::ReplacePhrase(phrase));
                self.message = Some(format!("edited {id}"));
            }

            Cmd::DeleteBars(ids) => {
                // Delete by stable phrase.id (what the TUI shows)
                let mut not_found = Vec::new();
                for id in &ids {
                    if let Some(pos) = self.phrases.iter().position(|p| p.id == *id) {
                        let removed = self.phrases.remove(pos);
                        let _ = self.audio_tx.send(AudioCmd::RemovePhrase(removed.id));
                    } else {
                        not_found.push(*id);
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
                        .map(|p| p.bar.total_subdivs)
                        .sum();
                    let count = self.phrases.len().max(1);
                    (total / count / 2).clamp(2, 4)
                };

                // Build src from the original tokens
                let src = resolved.iter()
                    .map(|s| s.src.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");

                let id     = self.next_phrase_id;
                self.next_phrase_id += 1;
                let phrase = build_phrase(id, src, resolved, peak, repeat.max(1));
                let _ = self.audio_tx.send(AudioCmd::AddPhrase(phrase.clone()));
                self.phrases.push(phrase);
                self.message = None;
            }
        }
    }
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
