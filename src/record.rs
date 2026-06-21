// record.rs - recorder entry point.

#[path = "record_old.rs"]
mod record_old;

use crate::fx::FxSettings;
use crate::sequencer::Phrase;
use crate::vcf::VcfBank;

pub fn record_cycle(
    phrases: Vec<Phrase>,
    bpm: f64,
    sustain: f64,
    vcf: VcfBank,
    fx: FxSettings,
    cycle_repeat: usize,
) -> anyhow::Result<String> {
    record_old::record_cycle(phrases, bpm, sustain, vcf, fx, cycle_repeat)
}
