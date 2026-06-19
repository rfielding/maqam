// midi_clock.rs — receive MIDI clock (0xF8) from external gear and sync BPM
//
// Usage (TUI): clockin /dev/snd/midiC2D0
//
// Listens for MIDI timing clock bytes (0xF8). There are 24 pulses per quarter
// note. We accumulate intervals over 8 pulses (= 1/3 beat) and emit SetBpm
// whenever the rolling average changes by more than 0.5 BPM.

use crossbeam_channel::Sender;
use std::fs::File;
use std::io::Read;
use std::time::Instant;

use crate::sequencer::AudioCmd;

pub fn start_clock_receiver(device: String, tx: Sender<AudioCmd>) {
    std::thread::Builder::new()
        .name("midi-clockin".into())
        .spawn(move || {
            if let Err(e) = run_receiver(&device, tx) {
                eprintln!("[clockin] error: {e}");
            }
        })
        .expect("spawn midi-clockin");
}

fn run_receiver(device: &str, tx: Sender<AudioCmd>) -> anyhow::Result<()> {
    let mut file = File::open(device)?;
    let mut buf = [0u8; 1];

    // Ring buffer of the last 8 inter-pulse intervals (in microseconds)
    const WINDOW: usize = 8;
    let mut intervals = [0u64; WINDOW];
    let mut idx = 0usize;
    let mut filled = 0usize;
    let mut last_tick = Instant::now();
    let mut last_bpm = 0.0f64;

    loop {
        file.read_exact(&mut buf)?;
        match buf[0] {
            // MIDI timing clock
            0xF8 => {
                let now = Instant::now();
                let micros = now.duration_since(last_tick).as_micros() as u64;
                last_tick = now;

                // Ignore first tick (no valid interval yet)
                if micros == 0 {
                    continue;
                }

                intervals[idx % WINDOW] = micros;
                idx += 1;
                if filled < WINDOW {
                    filled += 1;
                }
                if filled < 2 {
                    continue;
                }

                // Average over available samples
                let sum: u64 = intervals[..filled].iter().sum();
                let avg_micros = sum as f64 / filled as f64;

                // 24 pulses per quarter note → BPM
                let bpm = 60_000_000.0 / (avg_micros * 24.0);

                // Only send if changed by > 0.5 BPM to avoid flooding
                if (bpm - last_bpm).abs() > 0.5 {
                    let _ = tx.send(AudioCmd::SetBpm(bpm));
                    last_bpm = bpm;
                }
            }
            // MIDI stop / start / continue — ignore for now
            0xFA | 0xFB | 0xFC => {}
            _ => {}
        }
    }
}
