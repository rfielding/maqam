// app.rs — application state, phrases as top-level units

use crossbeam_channel::Sender;
use crate::command::{self, Cmd, JinsSpec};
use crate::record;
use crate::sequencer::{build_phrase, AudioCmd, BarSpec, Phrase};

pub struct App {
    pub phrases:     Vec<Phrase>,
    pub input:       String,
    pub message:     Option<String>,
    pub bpm:         f64,
    pub sustain:     f64,
    pub vol:         f32,
    pub paused:      bool,
    pub should_quit:    bool,
    pub last_recording: Option<String>,  // persists across commands
    pub history:     Vec<String>,
    pub history_pos: Option<usize>,
    pub saved_input: String,
    next_phrase_id:  usize,
    last_rhythm:     Vec<u8>,
    audio_tx:        Sender<AudioCmd>,
}

impl App {
    pub fn new(audio_tx: Sender<AudioCmd>) -> Self {
        App {
            phrases:        Vec::new(),
            input:          String::new(),
            message:        Some("<root> <maqam> [groups][,…] [r<N>]  │  bpm s x<N> clear ?".into()),
            bpm:            120.0,
            sustain:        1.25,
            vol:            1.0,
            paused:         false,
            should_quit:    false,
            last_recording: None,
            history:        Vec::new(),
            history_pos:    None,
            saved_input:    String::new(),
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
        if let Some(i) = self.history_pos { self.input = self.history[i].clone(); }
    }

    pub fn history_down(&mut self) {
        match self.history_pos {
            None => {}
            Some(i) if i + 1 >= self.history.len() => {
                self.history_pos = None;
                self.input       = self.saved_input.clone();
            }
            Some(i) => {
                self.history_pos = Some(i + 1);
                self.input = self.history[i + 1].clone();
            }
        }
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
            Cmd::Help  => {
                self.message = Some(
                    "<root> <maqam> [groups][,…] [r<N>]  │  bpm <n>  s <n>  x<N>  clear  ?"
                        .into(),
                );
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
                    // Move last phrase to front, shift everything else down
                    let last = self.phrases.pop().unwrap();
                    self.phrases.insert(0, last);
                    let _ = self.audio_tx.send(AudioCmd::Clear);
                    for p in &self.phrases {
                        let _ = self.audio_tx.send(AudioCmd::AddPhrase(p.clone()));
                    }
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

            Cmd::DeleteBars(ids) => {
                let mut not_found = Vec::new();
                for id in &ids {
                    if self.phrases.iter().any(|p| p.id == *id) {
                        self.phrases.retain(|p| p.id != *id);
                        let _ = self.audio_tx.send(AudioCmd::RemovePhrase(*id));
                    } else {
                        not_found.push(*id);
                    }
                }
                if !not_found.is_empty() {
                    let s: Vec<String> = not_found.iter().map(|i| i.to_string()).collect();
                    self.message = Some(format!("✗ no phrase {}", s.join(" ")));
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
