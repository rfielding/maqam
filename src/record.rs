// record.rs - wrapper around the previous recorder.
//
// Checkpoint: the audio and timing still come from the old recorder, but the
// final MP4 video stream is replaced by an unmistakable generated source.
// This proves the background layer is under repo control before we wire the
// real carpet source into record_old.rs.

#[path = "record_old.rs"]
mod record_old;

use crate::sequencer::Phrase;

pub fn record_cycle(
    phrases: Vec<Phrase>,
    bpm: f64,
    sustain: f64,
    cycle_repeat: usize,
) -> anyhow::Result<String> {
    let path = record_old::record_cycle(phrases, bpm, sustain, cycle_repeat)?;
    if crate::source_background::replace_video_with_generated_source(&path).unwrap_or(false) {
        eprintln!("carpet-guided-background: replaced video with generated source background");
    } else {
        eprintln!("carpet-guided-background: generated source background skipped");
    }
    Ok(path)
}
