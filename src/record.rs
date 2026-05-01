// record.rs — offline synthesis + ffmpeg MP4 with phrase overlay

use std::io::Write;
use std::process::Command;

use crate::sequencer::Phrase;
use crate::synth::{evolve_bar, spawn_phrase_start, spawn_sub_bass, spawn_voices, Voice};

const SR: f64 = 44100.0;

// ── Phrase event log ──────────────────────────────────────────────────────────

struct PhraseMoment {
    sample:     usize,
    phrase_idx: usize,
    play_num:   usize,   // which play of this phrase (0-based)
}

// ── Public entry point ────────────────────────────────────────────────────────

pub fn record_cycle(
    phrases:       &[Phrase],
    bpm:           f64,
    sustain:       f64,
    cycle_repeat:  usize,
) -> anyhow::Result<String> {
    if phrases.is_empty() {
        return Err(anyhow::anyhow!("nothing to record — add some phrases first"));
    }

    let subdiv_secs    = 60.0 / (bpm * 2.0);
    let subdiv_samples = SR * subdiv_secs;

    let bar_samples: Vec<usize> = phrases.iter()
        .map(|p| ((subdiv_samples * p.bar.total_subdivs as f64).round() as usize).max(1))
        .collect();

    let one_cycle: usize = phrases.iter().enumerate()
        .map(|(i, p)| bar_samples[i] * p.repeat)
        .sum();
    let cycle_samples = one_cycle * cycle_repeat.max(1);
    let tail_samples  = (SR * (sustain + 0.6)) as usize;
    let total_samples = cycle_samples + tail_samples;  // audio frames
    let total_stereo  = total_samples * 2 + tail_samples * 2; // L+R samples (tail added separately)

    let mut phrases_v: Vec<Phrase> = phrases.to_vec();

    let mut voices:      Vec<Voice>         = Vec::new();
    let mut buffer:      Vec<f32>           = Vec::with_capacity(total_stereo);
    let mut log:         Vec<PhraseMoment>  = Vec::new();

    let mut cur_phrase:  usize       = 0;
    let mut plays_done:  usize       = 0;
    let mut bar_pos:     usize       = 0;
    let mut last_subdiv: Option<usize> = None;
    let mut cycles_done: usize       = 0;

    // Log the very start
    log.push(PhraseMoment { sample: 0, phrase_idx: 0, play_num: 0 });

    for sample_n in 0..cycle_samples {
        let event = if sample_n < cycle_samples
                    && cycles_done < cycle_repeat.max(1)
                    && cur_phrase < phrases_v.len()
        {
            let bar = &phrases_v[cur_phrase].bar;
            let bs  = bar_samples[cur_phrase];

            let curr = if bar.total_subdivs > 0 {
                ((bar_pos as f64 / subdiv_samples) as usize).min(bar.total_subdivs - 1)
            } else { 0 };

            let is_phrase_start = last_subdiv.is_none() && curr == 0
                && plays_done == 0;
            let ev = if last_subdiv != Some(curr) {
                last_subdiv = Some(curr);
                bar.events.get(curr).copied()
            } else { None };

            if is_phrase_start {
                let root_hz     = phrases_v[cur_phrase].bar.root.to_hz();
                let phrase_secs = phrases_v[cur_phrase].bar.total_subdivs as f64
                                * subdiv_secs
                                * phrases_v[cur_phrase].repeat as f64;
                spawn_phrase_start(root_hz, sustain, &mut voices);
                spawn_sub_bass(root_hz, phrase_secs, &mut voices);
            }

            bar_pos += 1;
            if bar_pos >= bs {
                bar_pos = 0; last_subdiv = None;
                // Evolve after every play — same as real-time
                evolve_bar(&mut phrases_v[cur_phrase].bar, true);
                plays_done += 1;
                if plays_done >= phrases_v[cur_phrase].repeat {
                    plays_done = 0;
                    cur_phrase += 1;
                    if cur_phrase >= phrases_v.len() {
                        cur_phrase  = 0;
                        cycles_done += 1;
                    }
                    // Log phrase transition
                    if cycles_done < cycle_repeat.max(1) {
                        log.push(PhraseMoment {
                            sample:     sample_n,
                            phrase_idx: cur_phrase,
                            play_num:   plays_done,
                        });
                    }
                } else {
                    // Log repeat increment
                    log.push(PhraseMoment {
                        sample:     sample_n,
                        phrase_idx: cur_phrase,
                        play_num:   plays_done,
                    });
                }
            }
            ev
        } else { None };

        if let Some(ev) = event {
            spawn_voices(ev, sustain, &mut voices, false);
        }

        // Stereo mix
        let (mut left, mut right) = (0f32, 0f32);
        for v in voices.iter_mut() {
            let s     = v.sample(SR);
            let angle = (v.pan + 1.0) * std::f32::consts::FRAC_PI_4;
            left  += s * angle.cos();
            right += s * angle.sin();
        }
        buffer.push(left.clamp(-1.0, 1.0));
        buffer.push(right.clamp(-1.0, 1.0));
        voices.retain(|v| !v.done);
    }

    // ── Write files ───────────────────────────────────────────────────────────
    // ── Final "1" — root of first phrase, struck once at cycle end ───────────
    {
        let root_hz     = phrases_v[0].bar.root.to_hz();
        let phrase_secs = phrases_v[0].bar.total_subdivs as f64
                        * subdiv_secs
                        * phrases_v[0].repeat as f64;
        spawn_phrase_start(root_hz, sustain, &mut voices);
        spawn_sub_bass(root_hz, phrase_secs.min(sustain + 0.5), &mut voices);
    }
    // Render the tail (voices already spawned above will decay through it)
    for _ in 0..tail_samples {
        let (mut left, mut right) = (0f32, 0f32);
        for v in voices.iter_mut() {
            let s     = v.sample(SR);
            let angle = (v.pan + 1.0) * std::f32::consts::FRAC_PI_4;
            left  += s * angle.cos();
            right += s * angle.sin();
        }
        buffer.push(left.clamp(-1.0, 1.0));
        buffer.push(right.clamp(-1.0, 1.0));
        voices.retain(|v| !v.done);
    }

    let wav_path = "/tmp/maqam-live.wav";
    let ass_path = "/tmp/maqam-live.ass";
    let home     = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let ts       = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default().as_secs();
    let mp4_path = format!("{home}/maqam-{ts}.mp4");

    write_wav_stereo(wav_path, &buffer)?;
    write_ass(ass_path, &log, &phrases_v, total_samples)?;

    // ── ffmpeg ────────────────────────────────────────────────────────────────
    let filter = format!(
        "color=c=0x121216:s=1280x720:r=30[bg];\
         [0:a]aformat=channel_layouts=mono,\
         showwaves=s=1280x480:mode=cline:rate=30:colors=0x64C8AA,\
         format=rgba[wave];\
         [bg][wave]overlay=0:240:shortest=1[bw];\
         [bw]subtitles={ass_path}:force_style='FontName=Monospace'[v]"
    );

    let out = Command::new("ffmpeg")
        .args([
            "-y", "-i", wav_path,
            "-filter_complex", &filter,
            "-map", "[v]", "-map", "0:a",
            "-c:v", "libx264", "-c:a", "aac",
            "-shortest", &mp4_path,
        ])
        .output()?;

    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let snippet: String = err.lines().rev().take(3)
            .collect::<Vec<_>>().into_iter().rev()
            .collect::<Vec<_>>().join(" | ");
        return Err(anyhow::anyhow!("ffmpeg: {snippet}"));
    }

    Ok(mp4_path)
}

// ── ASS subtitle generation ───────────────────────────────────────────────────
// Shows all phrases; the currently executing one is bright teal, others dimmed.

fn write_ass(
    path:    &str,
    log:     &[PhraseMoment],
    phrases: &[Phrase],
    total_samples: usize,
) -> anyhow::Result<()> {
    let mut f = std::fs::File::create(path)?;

    // Header
    writeln!(f, "[Script Info]")?;
    writeln!(f, "ScriptType: v4.00+")?;
    writeln!(f, "PlayResX: 1280")?;
    writeln!(f, "PlayResY: 720")?;   // top 240px strip reserved for text
    writeln!(f, "WrapStyle: 0")?;
    writeln!(f)?;
    writeln!(f, "[V4+ Styles]")?;
    writeln!(f, "Format: Name,Fontname,Fontsize,PrimaryColour,SecondaryColour,\
                 OutlineColour,BackColour,Bold,Italic,Underline,Strikeout,\
                 ScaleX,ScaleY,Spacing,Angle,BorderStyle,Outline,Shadow,\
                 Alignment,MarginL,MarginR,MarginV,Encoding")?;
    // Teal primary, dark background box, monospace
    writeln!(f, "Style: Default,Monospace,22,&H00AAC864,&H000000FF,\
                 &H00000000,&HAA121216,0,0,0,0,\
                 100,100,0,0,3,1,0,7,20,20,10,1")?;
    writeln!(f)?;
    writeln!(f, "[Events]")?;
    writeln!(f, "Format: Layer,Start,End,Style,Name,MarginL,MarginR,MarginV,Effect,Text")?;

    let total_secs = total_samples as f64 / SR;

    for (i, moment) in log.iter().enumerate() {
        let t_start = moment.sample as f64 / SR;
        let t_end   = if i + 1 < log.len() {
            log[i + 1].sample as f64 / SR
        } else {
            total_secs
        };

        if (t_end - t_start) < 0.01 { continue; }

        let text = render_phrase_list(phrases, moment.phrase_idx, moment.play_num);

        writeln!(f, "Dialogue: 0,{},{},Default,,0,0,0,,{}",
            ass_time(t_start), ass_time(t_end), text)?;
    }

    Ok(())
}

/// Render all phrases as one ASS text block.
/// Current phrase is bright teal; others are dimmed grey.
/// Uses ASS override tags for per-phrase coloring.
fn render_phrase_list(phrases: &[Phrase], cur: usize, play_num: usize) -> String {
    let mut out = String::new();
    for (i, phrase) in phrases.iter().enumerate() {
        let rhythm   = phrase.rhythm_display();
        let maqams   = phrase.bar.maqam_names.join("+");
        let rep_info = if phrase.repeat > 1 {
            format!("  [{}/{}]", play_num + 1, phrase.repeat)
        } else {
            String::new()
        };

        // Escape ASS special chars: { } \ and newlines
        let src = phrase.src.replace('\\', "").replace('{', "").replace('}', "");

        let line = format!("{}: {}  {}  {}{}",
            i, src, rhythm, maqams, rep_info);

        if i == cur {
            // Bright teal, bold
            out.push_str(&format!("{{\\c&H00AAC864&\\b1}}{line}{{\\b0\\r}}"));
        } else {
            // Dimmed grey
            out.push_str(&format!("{{\\c&H00505060&}}{line}{{\\r}}"));
        }

        // ASS line break (not the last)
        if i + 1 < phrases.len() {
            out.push_str("\\N");
        }
    }
    out
}

fn ass_time(secs: f64) -> String {
    let s   = secs as u64;
    let ms  = ((secs - s as f64) * 100.0) as u64;
    let h   = s / 3600;
    let m   = (s % 3600) / 60;
    let sec = s % 60;
    format!("{h}:{m:02}:{sec:02}.{ms:02}")
}

// ── WAV writer ────────────────────────────────────────────────────────────────

/// Write interleaved stereo 16-bit PCM WAV.
fn write_wav_stereo(path: &str, samples: &[f32]) -> anyhow::Result<()> {
    let n_frames = (samples.len() / 2) as u32;  // L+R pairs
    let sr       = SR as u32;
    let channels = 2u16;
    let data_len = n_frames * 4;  // 2 channels × 2 bytes
    let mut f = std::fs::File::create(path)?;
    f.write_all(b"RIFF")?; f.write_all(&(36 + data_len).to_le_bytes())?;
    f.write_all(b"WAVE")?; f.write_all(b"fmt ")?;
    f.write_all(&16u32.to_le_bytes())?;          // chunk size
    f.write_all(&1u16.to_le_bytes())?;           // PCM
    f.write_all(&channels.to_le_bytes())?;       // stereo
    f.write_all(&sr.to_le_bytes())?;             // sample rate
    f.write_all(&(sr * 4).to_le_bytes())?;       // byte rate
    f.write_all(&4u16.to_le_bytes())?;           // block align
    f.write_all(&16u16.to_le_bytes())?;          // bits/sample
    f.write_all(b"data")?;
    f.write_all(&data_len.to_le_bytes())?;
    for &s in samples {
        f.write_all(&((s * 32767.0).clamp(-32768.0, 32767.0) as i16).to_le_bytes())?;
    }
    Ok(())
}
