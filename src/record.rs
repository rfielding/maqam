// record.rs - recorder entry point.
//
// The old recorder already knows how to build the timing/HUD/subtitle ASS file.
// The carpet pass still post-processes the finished MP4, so immediately burn the
// same ASS HUD back onto the carpet result. This is a temporary bridge until the
// generated carpet is wired into record_old.rs before its subtitle pass.

#[path = "record_old.rs"]
mod record_old;

use std::process::{Command, Stdio};

use crate::sequencer::Phrase;

fn temp_path(name: &str) -> String {
    let mut p = std::env::temp_dir();
    p.push(name);
    p.to_string_lossy().replace('\\', "/")
}

fn burn_hud_back_onto(path: &str) -> anyhow::Result<bool> {
    let ass_path = temp_path("maqam-live.ass");
    if !std::path::Path::new(&ass_path).exists() {
        eprintln!("carpet-guided-background: HUD restore skipped; {ass_path} missing");
        return Ok(false);
    }

    let tmp = format!("{path}.hud.mp4");
    let filter = format!("subtitles={ass_path}");
    let status = Command::new("ffmpeg")
        .args(["-y", "-i", path, "-vf", &filter])
        .args(["-map", "0:v", "-map", "0:a?"])
        .args(["-c:v", "libx264", "-crf", "18", "-pix_fmt", "yuv420p"])
        .args(["-c:a", "copy", "-movflags", "+faststart", &tmp])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    match status {
        Ok(s) if s.success() => {
            std::fs::rename(&tmp, path)?;
            Ok(true)
        }
        Ok(_) => {
            let _ = std::fs::remove_file(&tmp);
            Ok(false)
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!("HUD restore requires ffmpeg on your PATH")
        }
        Err(err) => Err(err.into()),
    }
}

pub fn record_cycle(
    phrases: Vec<Phrase>,
    bpm: f64,
    sustain: f64,
    cycle_repeat: usize,
) -> anyhow::Result<String> {
    let path = record_old::record_cycle(phrases.clone(), bpm, sustain, cycle_repeat)?;

    if crate::source_background::replace_video_with_generated_source_for_phrases(&path, &phrases).unwrap_or(false) {
        eprintln!("carpet-guided-background: replaced video with generated carpet source");
        if burn_hud_back_onto(&path)? {
            eprintln!("carpet-guided-background: restored HUD/subtitles over carpet source");
        } else {
            eprintln!("carpet-guided-background: HUD/subtitle restore skipped");
        }
    } else {
        eprintln!("carpet-guided-background: generated source background skipped");
    }

    Ok(path)
}
