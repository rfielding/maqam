// record.rs - wrapper around the previous recorder.
//
// Checkpoint: the audio and timing still come from the old recorder, but the
// final MP4 is recomposited with a source background generated from the loaded
// Phrase list.  Phrase ids now drive the deterministic Hilbert carpet skeleton.

#[path = "record_old.rs"]
mod record_old;

use crate::sequencer::Phrase;

pub fn record_cycle(
    phrases: Vec<Phrase>,
    bpm: f64,
    sustain: f64,
    cycle_repeat: usize,
) -> anyhow::Result<String> {
    let path = record_old::record_cycle(phrases.clone(), bpm, sustain, cycle_repeat)?;
    if crate::source_background::replace_video_with_generated_source_for_phrases(&path, &phrases).unwrap_or(false) {
        eprintln!("carpet-guided-background: replaced video with Hilbert phrase-id carpet background");
    } else {
        eprintln!("carpet-guided-background: generated source background skipped");
    }
    Ok(path)
}
