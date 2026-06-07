// record.rs - recorder entry point.
//
// Do not post-process the completed MP4 here. record_old.rs already burns the
// HUD/subtitles into the video. Replacing or blending after that is what caused
// the generated carpet to erase the text overlay. The carpet background needs
// to be wired into record_old.rs before the subtitle pass, not after it.

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
