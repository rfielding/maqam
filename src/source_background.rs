use std::process::{Command, Stdio};

pub fn replace_video_with_generated_source(path: &str) -> anyhow::Result<bool> {
    let tmp = format!("{path}.source-background.mp4");
    let source = "testsrc2=s=1280x720:r=30,eq=brightness=-0.48:saturation=1.65";
    let filter = "[1:v]format=rgba,lumakey=threshold=0.13:tolerance=0.10:softness=0.04[fg];[0:v][fg]overlay=format=auto[v]";
    let status = Command::new("ffmpeg")
        .args(["-y", "-f", "lavfi", "-i", source, "-i", path])
        .args(["-filter_complex", filter, "-map", "[v]", "-map", "1:a"])
        .args(["-c:v", "libx264", "-crf", "18", "-pix_fmt", "yuv420p"])
        .args(["-c:a", "copy", "-shortest", &tmp])
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
