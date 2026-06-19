// midi_clockout.rs — send MIDI clock (0xF8) to external gear, slaved to maqam-live BPM
//
// Usage (TUI): clockout /dev/snd/midiC2D0
//
// Sends 24 MIDI timing clock pulses (0xF8) per quarter note at the current BPM.
// BPM updates are received over an mpsc channel; the sender thread adjusts its
// sleep interval on the fly so the output tracks bpm changes without glitching.
//
// Also sends:
//   0xFA  (MIDI Start)  when clockout begins
//   0xFC  (MIDI Stop)   when clockout is cancelled (future: on `clockout stop`)

use crossbeam_channel::Receiver;
use std::fs::OpenOptions;
use std::io::Write;
use std::time::{Duration, Instant};

/// Call this from app.rs when the user types `clockout <device>`.
/// Returns a Sender<f64> so app can push new BPM values as they change.
pub fn start_clock_sender(device: String, initial_bpm: f64) -> crossbeam_channel::Sender<f64> {
    let (bpm_tx, bpm_rx) = crossbeam_channel::bounded::<f64>(8);

    std::thread::Builder::new()
        .name("midi-clockout".into())
        .spawn(move || {
            if let Err(e) = run_sender(&device, initial_bpm, bpm_rx) {
                eprintln!("[clockout] error: {e}");
            }
        })
        .expect("spawn midi-clockout");

    bpm_tx
}

fn pulse_interval(bpm: f64) -> Duration {
    // 24 pulses per quarter note
    Duration::from_secs_f64(60.0 / (bpm * 24.0))
}

fn run_sender(device: &str, initial_bpm: f64, bpm_rx: Receiver<f64>) -> anyhow::Result<()> {
    let mut file = OpenOptions::new().write(true).open(device)?;

    // Send MIDI Start
    file.write_all(&[0xFA])?;
    file.flush()?;

    let mut bpm = initial_bpm.max(1.0);
    let mut interval = pulse_interval(bpm);
    let mut next_tick = Instant::now() + interval;

    loop {
        // Drain any pending BPM updates (non-blocking)
        while let Ok(new_bpm) = bpm_rx.try_recv() {
            if (new_bpm - bpm).abs() > 0.1 && new_bpm > 1.0 {
                bpm = new_bpm;
                interval = pulse_interval(bpm);
            }
        }

        let now = Instant::now();
        if now >= next_tick {
            // Send MIDI Timing Clock
            file.write_all(&[0xF8])?;
            file.flush()?;
            // Schedule next tick relative to when it *should* have fired,
            // not when we actually woke up — this prevents drift accumulation.
            next_tick += interval;
            // If we're badly behind (e.g. system load spike), reset to now
            // rather than trying to fire a burst of catch-up pulses.
            if Instant::now() > next_tick + interval {
                next_tick = Instant::now() + interval;
            }
        } else {
            // Sleep until next tick, but wake early to check for BPM changes.
            let sleep = (next_tick - now).min(Duration::from_millis(2));
            std::thread::sleep(sleep);
        }
    }
}
