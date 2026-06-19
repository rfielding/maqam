// midi.rs — MIDI output with clean voice state machine
//
// All voice state in one shared struct.
// Single writer thread for all MIDI bytes.
// Timer thread fires note-offs and marks channels free.

use std::fs::OpenOptions;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const BASE_NOTE: u8   = 74;
const BEND_SEMIS: f64 = 2.0;
const DRUM_CH: u8     = 9;
const SUSTAIN_MS: u64 = 1200;

pub enum MidiEvent {
    Note { hz: f64 },
    SetBpm(f64),
    AllOff,
}

enum RawMsg {
    Bytes(Vec<u8>),
}

#[derive(Clone)]
struct Voice {
    note:     u8,
    fire_at:  Instant,
}

struct State {
    // per melodic channel: None = free, Some(voice) = busy
    voices: Vec<Option<Voice>>,
    // the melodic channel numbers (skipping drum ch)
    chs: Vec<u8>,
}

impl State {
    fn new() -> Self {
        let chs: Vec<u8> = (0u8..16).filter(|&c| c != DRUM_CH).collect();
        let voices = vec![None; chs.len()];
        State { voices, chs }
    }

    // find a free slot, or steal the oldest busy one
    fn alloc(&mut self) -> usize {
        // prefer free
        if let Some(i) = self.voices.iter().position(|v| v.is_none()) {
            return i;
        }
        // steal oldest
        self.voices.iter().enumerate()
            .filter_map(|(i, v)| v.as_ref().map(|vv| (i, vv.fire_at)))
            .min_by_key(|&(_, t)| t)
            .map(|(i, _)| i)
            .unwrap_or(0)
    }

    fn ch(&self, vi: usize) -> u8 {
        self.chs[vi]
    }
}

pub struct MidiOutput {
    pub tx: crossbeam_channel::Sender<MidiEvent>,
    device_path: String,
}

impl MidiOutput {
    pub fn hard_reset(&self) {
        if let Ok(mut f) = OpenOptions::new().write(true).open(&self.device_path) {
            let mut b = Vec::new();
            for ch in 0u8..16 {
                b.extend_from_slice(&[0xB0 | ch, 123, 0]);
                b.extend_from_slice(&[0xE0 | ch, 0x00, 0x40]);
            }
            let _ = f.write_all(&b);
        }
    }

    pub fn start(device_path: String, _initial_bpm: f64) -> anyhow::Result<Self> {
        let (tx, rx) = crossbeam_channel::bounded::<MidiEvent>(1024);

        // single writer
        let (raw_tx, raw_rx) = crossbeam_channel::bounded::<RawMsg>(8192);
        let dev = device_path.clone();
        thread::spawn(move || {
            let mut file = OpenOptions::new().write(true).open(&dev)
                .expect("cannot open MIDI device");
            while let Ok(RawMsg::Bytes(b)) = raw_rx.recv() {
                let _ = file.write_all(&b);
            }
        });

        // shared voice state
        let state = Arc::new(Mutex::new(State::new()));

        // initial reset
        {
            let st = state.lock().unwrap();
            let mut b = Vec::new();
            for &ch in &st.chs {
                b.extend_from_slice(&[0xB0 | ch, 123, 0]);
                b.extend_from_slice(&[0xE0 | ch, 0x00, 0x40]);
            }
            let _ = raw_tx.send(RawMsg::Bytes(b));
        }

        // note-off timer thread
        let state2   = Arc::clone(&state);
        let raw_tx2  = raw_tx.clone();
        thread::spawn(move || {
            loop {
                thread::sleep(Duration::from_millis(5));
                let now = Instant::now();
                let mut st = state2.lock().unwrap();
                for vi in 0..st.voices.len() {
                    if let Some(ref v) = st.voices[vi].clone() {
                        if now >= v.fire_at {
                            let ch   = st.chs[vi];
                            let note = v.note;
                            st.voices[vi] = None; // mark free
                            let _ = raw_tx2.send(RawMsg::Bytes(vec![
                                0x90 | ch, note, 0,        // note-on vel=0 = silence
                                0xE0 | ch, 0x00, 0x40,     // bend center
                            ]));
                        }
                    }
                }
            }
        });

        // event thread
        let state3  = Arc::clone(&state);
        let raw_tx3 = raw_tx.clone();
        thread::spawn(move || {
            while let Ok(ev) = rx.recv() {
                match ev {
                    MidiEvent::SetBpm(_) => {}

                    MidiEvent::Note { hz } => {
                        let (note, bend) = hz_to_midi(hz);
                        let lsb = (bend & 0x7F) as u8;
                        let msb = ((bend >> 7) & 0x7F) as u8;

                        let mut st = state3.lock().unwrap();
                        let vi     = st.alloc();
                        let ch     = st.ch(vi);
                        let old    = st.voices[vi].clone();

                        // 1. silence (vel=0) — always, whether stealing or fresh
                        let mut bytes = Vec::new();
                        let old_note = old.as_ref().map(|v| v.note).unwrap_or(note);
                        bytes.extend_from_slice(&[0x90 | ch, old_note, 0]);
                        // 2. pitch bend while silent
                        bytes.extend_from_slice(&[0xE0 | ch, lsb, msb]);
                        // 3. note-on with velocity
                        bytes.extend_from_slice(&[0x90 | ch, note, 100]);

                        st.voices[vi] = Some(Voice {
                            note,
                            fire_at: Instant::now() + Duration::from_millis(SUSTAIN_MS),
                        });
                        drop(st);

                        let _ = raw_tx3.send(RawMsg::Bytes(bytes));
                    }

                    MidiEvent::AllOff => {
                        let mut st = state3.lock().unwrap();
                        let mut b  = Vec::new();
                        for ch in 0u8..16 {
                            b.extend_from_slice(&[0xB0 | ch, 123, 0]);
                            b.extend_from_slice(&[0xE0 | ch, 0x00, 0x40]);
                        }
                        for v in st.voices.iter_mut() { *v = None; }
                        drop(st);
                        let _ = raw_tx3.send(RawMsg::Bytes(b));
                    }
                }
            }
        });

        Ok(MidiOutput { tx, device_path })
    }
}

fn hz_to_midi(hz: f64) -> (u8, u16) {
    use crate::tuning::D_HZ;
    if !hz.is_finite() || hz <= 0.0 {
        return (BASE_NOTE, 8192);
    }
    let mut r = hz / D_HZ;
    if r <= 0.0 { return (BASE_NOTE, 8192); }
    let mut guard = 0;
    while r >= 2.0 && guard < 20 { r /= 2.0; guard += 1; }
    guard = 0;
    while r < 1.0  && guard < 20 { r *= 2.0; guard += 1; }
    let semitones = r.log2() * 12.0;
    let nearest   = semitones.round();
    let remainder = semitones - nearest;
    let n = (BASE_NOTE as i32 + nearest as i32).clamp(0, 127) as u8;
    let bend_norm = (remainder / BEND_SEMIS).clamp(-1.0, 1.0);
    let bend14 = ((bend_norm * 8191.0) as i32 + 8192).clamp(0, 16383) as u16;
    (n, bend14)
}
