use std::process::{Command, Stdio};

pub fn replace_video_with_generated_source(path: &str) -> anyhow::Result<bool> {
    let tmp = format!("{path}.source-background.mp4");
    let source = "testsrc2=s=1280x720:r=30,eq=brightness=-0.45:saturation=1.8";
    let status = Command::new("ffmpeg")
        .args(["-y", "-f", "lavfi", "-i", source, "-i", path])
        .args(["-map", "0:v", "-map", "1:a", "-c:v", "libx264", "-crf", "18"])
        .args(["-pix_fmt", "yuv420p", "-c:a", "copy", "-shortest", &tmp])
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
