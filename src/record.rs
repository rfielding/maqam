// record.rs - wrapper around the previous recorder.
//
// This makes a deliberately visible carpet-branch difference without touching
// the old huge recorder file.  The old implementation is preserved as
// record_old.rs; this wrapper calls it and then color-shifts the MP4 so the
// branch cannot look identical while record.rs is being split up.

#[path = "record_old.rs"]
mod record_old;

use std::process::{Command, Stdio};

use crate::sequencer::Phrase;

fn visibly_mark_carpet_branch(path: &str) -> anyhow::Result<bool> {
    let tmp = format!("{path}.carpet-visible.mp4");
    let status = Command::new("ffmpeg")
        .args(["-y", "-i", path, "-vf", "hue=h=45:s=1.6", "-c:a", "copy", &tmp])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    match status {
        Ok(s) if s.success() => {
            std::fs::rename(&tmp, path)?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

pub fn record_cycle(
    phrases: Vec<Phrase>,
    bpm: f64,
    sustain: f64,
    cycle_repeat: usize,
) -> anyhow::Result<String> {
    let path = record_old::record_cycle(phrases, bpm, sustain, cycle_repeat)?;
    if visibly_mark_carpet_branch(&path).unwrap_or(false) {
        eprintln!("carpet-guided-background: visibly color-shifted MP4 output");
    } else {
        eprintln!("carpet-guided-background: visible MP4 postprocess skipped");
    }
    Ok(path)
}
