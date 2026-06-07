// record.rs - recorder entry point.

#[path = "record_old.rs"]
mod record_old;

use crate::sequencer::Phrase;

pub fn record_cycle(
    phrases: Vec<Phrase>,
    bpm: f64,
    sustain: f64,
    cycle_repeat: usize,
) -> anyhow::Result<String> {
    record_old::record_cycle(phrases, bpm, sustain, cycle_repeat)
}
