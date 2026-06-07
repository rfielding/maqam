// record.rs - recorder entry point.
//
// The old recorder already knows how to build the timing/HUD/subtitle ASS file.
// The carpet pass currently writes a generated PPM, then post-processes the MP4.
// Until the PPM is wired directly into record_old.rs, force that PPM to be the
// video layer and then burn the existing ASS HUD back on top.

#[path = "record_old.rs"]
mod record_old;

use std::path::PathBuf;
use std::process::Command;

use crate::sequencer::Phrase;

fn cwd_path(name: &str) -> String {
    let mut p = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    p.push(name);
    p.to_string_lossy().replace('\\', "/")
}

fn temp_path(name: &str) -> String {
    let mut p = std::env::temp_dir();
    p.push(name);
    p.to_string_lossy().replace('\\', "/")
}

fn command_failure(step: &str, output: &std::process::Output) -> anyhow::Error {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr.trim();
    if detail.is_empty() {
        anyhow::anyhow!("{step} failed with status {}", output.status)
    } else {
        anyhow::anyhow!("{step} failed: {detail}")
    }
}

fn overwrite_video_with_last_carpet_source(path: &str) -> anyhow::Result<()> {
    let ppm = cwd_path("carpet.ppm");
    if !std::path::Path::new(&ppm).exists() {
        anyhow::bail!("generated carpet image is missing: {ppm}");
    }

    let tmp = format!("{path}.carpet-only.mp4");
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-loop",
            "1",
            "-framerate",
            "30",
            "-i",
            &ppm,
            "-i",
            path,
        ])
        .args(["-map", "0:v", "-map", "1:a?"])
        .args(["-c:v", "libx264", "-crf", "18", "-pix_fmt", "yuv420p"])
        .args(["-c:a", "copy", "-shortest", "-movflags", "+faststart", &tmp])
        .output();

    match status {
        Ok(output) if output.status.success() => {
            std::fs::rename(&tmp, path)?;
            Ok(())
        }
        Ok(output) => {
            let _ = std::fs::remove_file(&tmp);
            Err(command_failure("carpet source replacement", &output))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!("carpet source replacement requires ffmpeg on your PATH")
        }
        Err(err) => Err(err.into()),
    }
}

fn burn_hud_back_onto(path: &str) -> anyhow::Result<()> {
    let ass_path = temp_path("maqam-live.ass");
    if !std::path::Path::new(&ass_path).exists() {
        anyhow::bail!("HUD subtitle file is missing: {ass_path}");
    }

    let tmp = format!("{path}.hud.mp4");
    let filter = format!("subtitles={ass_path}");
    let status = Command::new("ffmpeg")
        .args(["-y", "-i", path, "-vf", &filter])
        .args(["-map", "0:v", "-map", "0:a?"])
        .args(["-c:v", "libx264", "-crf", "18", "-pix_fmt", "yuv420p"])
        .args(["-c:a", "copy", "-movflags", "+faststart", &tmp])
        .output();

    match status {
        Ok(output) if output.status.success() => {
            std::fs::rename(&tmp, path)?;
            Ok(())
        }
        Ok(output) => {
            let _ = std::fs::remove_file(&tmp);
            Err(command_failure("HUD restore", &output))
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

    crate::source_background::replace_video_with_generated_source_for_phrases(&path, &phrases)?;
    overwrite_video_with_last_carpet_source(&path)?;
    burn_hud_back_onto(&path)?;

    Ok(path)
}
