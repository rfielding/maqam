// record.rs — offline synthesis + ffmpeg MP4 with phrase overlay

use std::io::Write;
use std::collections::HashMap;
use std::process::Command;

use crate::sequencer::Phrase;
use crate::synth::{evolve_bar, spawn_phrase_start, spawn_sub_bass, spawn_voices, Milestone, Voice};

const SR: f64 = 44100.0;

// ── Sequence expansion ────────────────────────────────────────────────────────
//
// Returns one entry per distinct phrase position in execution order,
// following jump logic — does NOT expand phrase.repeat (the render loop does).

fn expand_one_cycle(phrases: &[Phrase]) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    let mut cur: usize      = 0;
    let mut jc:  HashMap<usize, usize> = HashMap::new();
    let max_items = phrases.len() * 512 + 1;

    while out.len() < max_items {
        if cur >= phrases.len() { break; }

        let phrase = &phrases[cur];

        if let Some(js) = &phrase.jump {
            let pid       = phrase.id;
            let remaining = jc.entry(pid).or_insert(js.times.saturating_sub(1));
            if *remaining > 0 {
                *remaining -= 1;
                let target   = js.to_pos.min(phrases.len().saturating_sub(1));
                let jump_pos = cur;
                let ids: Vec<usize> = phrases[target..jump_pos].iter()
                    .filter_map(|p| p.jump.as_ref().map(|_| p.id))
                    .collect();
                for id in ids { jc.remove(&id); }
                cur = target;
            } else {
                jc.remove(&pid);
                cur += 1;
            }
            continue;
        }

        // Musical phrase — one entry (render loop handles repeat plays)
        out.push(cur);
        cur += 1;
        if cur >= phrases.len() { break; }
    }
    out
}

// ── PhraseMoment log ──────────────────────────────────────────────────────────

struct PhraseMoment {
    sample:     usize,
    phrase_idx: usize,
    play_num:   usize,
}

// ── Public entry point ────────────────────────────────────────────────────────

pub fn record_cycle(
    phrases:      &[Phrase],
    bpm:          f64,
    sustain:      f64,
    cycle_repeat: usize,
) -> anyhow::Result<String> {
    if phrases.is_empty() {
        return Err(anyhow::anyhow!("nothing to record — add some phrases first"));
    }

    let subdiv_secs    = 60.0 / (bpm * 2.0);
    let subdiv_samples = SR * subdiv_secs;

    let bar_samples_for = |idx: usize| -> usize {
        ((subdiv_samples * phrases[idx].bar.total_subdivs as f64).round() as usize).max(1)
    };

    // One cycle: sequence expanded by jump logic, each phrase played repeat times
    let one_cycle_seq: Vec<usize> = expand_one_cycle(phrases);
    let one_cycle_samples: usize  = one_cycle_seq.iter()
        .map(|&i| bar_samples_for(i) * phrases[i].repeat.max(1))
        .sum();

    let cycles        = cycle_repeat.max(1);
    let cycle_samples = one_cycle_samples * cycles;
    let tail_samples  = (SR * (sustain + 0.6)) as usize;

    let mut phrases_v: Vec<Phrase>       = phrases.to_vec();
    let mut voices:    Vec<Voice>        = Vec::new();
    let mut buffer:    Vec<f32>          = Vec::with_capacity((cycle_samples + tail_samples) * 2);
    let mut log:       Vec<PhraseMoment> = Vec::new();

    // Flat play list: (phrase_idx, play_num) — used for log + rendering
    let mut full_seq: Vec<(usize, usize)> = Vec::new();
    for _ in 0..cycles {
        for &idx in &one_cycle_seq {
            for play in 0..phrases[idx].repeat.max(1) {
                full_seq.push((idx, play));
            }
        }
    }

    if !full_seq.is_empty() {
        log.push(PhraseMoment { sample: 0, phrase_idx: full_seq[0].0, play_num: 0 });
    }

    let mut sample_n: usize = 0;

    for (seq_pos, &(phrase_idx, play_num)) in full_seq.iter().enumerate() {
        let bs           = bar_samples_for(phrase_idx);
        let is_first     = play_num == 0;
        let repeat_count = phrases_v[phrase_idx].repeat.max(1);
        let is_last_play = play_num + 1 >= repeat_count;

        // PhraseStart: only on first play of each phrase group
        if is_first {
            let root_hz     = phrases_v[phrase_idx].bar.root_hz;
            let phrase_secs = (phrases_v[phrase_idx].bar.total_subdivs as f64
                              * subdiv_secs * repeat_count as f64).min(3.0);
            spawn_phrase_start(root_hz, sustain, &mut voices);
            spawn_sub_bass(root_hz, phrase_secs, &mut voices);
        }

        // Log phrase transitions and repeat increments
        if seq_pos > 0 {
            log.push(PhraseMoment { sample: sample_n, phrase_idx, play_num });
        }

        // Render this bar
        let mut bar_pos:     usize        = 0;
        let mut last_subdiv: Option<usize> = None;
        let total_subdivs   = phrases_v[phrase_idx].bar.total_subdivs;

        for _ in 0..bs {
            let ev = if total_subdivs > 0 {
                let curr = ((bar_pos as f64 / subdiv_samples) as usize)
                    .min(total_subdivs - 1);
                let ev = if last_subdiv != Some(curr) {
                    last_subdiv = Some(curr);
                    // Turnaround milestone on last subdivision of last play
                    let is_last_subdiv = curr + 1 >= total_subdivs;
                    let milestone = if is_last_play && is_last_subdiv {
                        Milestone::Turnaround
                    } else if is_first && curr == 0 {
                        Milestone::PhraseStart
                    } else {
                        Milestone::None
                    };
                    phrases_v[phrase_idx].bar.events.get(curr).copied()
                        .map(|e| (e, milestone))
                } else { None };
                bar_pos += 1;
                ev
            } else { None };

            if let Some((ev, milestone)) = ev {
                spawn_voices(ev, sustain, &mut voices, milestone,
                             &phrases_v[phrase_idx].bar.frequencies);
            }

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
            sample_n += 1;
        }

        // Evolve after each play (same as audio.rs)
        evolve_bar(&mut phrases_v[phrase_idx].bar, true);

        // Fade all voices at phrase boundary (not between repeats of same phrase)
        if is_last_play {
            for v in voices.iter_mut() {
                if v.release_frames.is_none() {
                    v.release_frames = Some(441); // 10ms
                }
            }
        }
    }

    // Final "1"
    if let Some(&(first_idx, _)) = full_seq.first() {
        let root_hz     = phrases_v[first_idx].bar.root_hz;
        let phrase_secs = (phrases_v[first_idx].bar.total_subdivs as f64 * subdiv_secs)
                         .min(sustain + 0.5);
        spawn_phrase_start(root_hz, sustain, &mut voices);
        spawn_sub_bass(root_hz, phrase_secs, &mut voices);
    }

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
    write_wav_stereo(wav_path, &buffer)?;

    let total_frames = (buffer.len() / 2) as f64 / SR;
    let ass_path     = "/tmp/maqam-live.ass";
    let musical: Vec<usize> = phrases.iter().enumerate()
        .filter(|(_, p)| p.jump.is_none() && p.bar.total_subdivs > 0)
        .map(|(i, _)| i)
        .collect();
    write_ass(ass_path, &log, phrases, &musical, total_frames)?;

    let ts  = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let out = format!("{}/maqam-{ts}.mp4",
        std::env::var("HOME").unwrap_or_else(|_| ".".into()));

    // Try with ASS subtitle overlay first; fall back to plain waveform if
    // libass is not available, then bare audio-only if video also fails.
    let filter_with_subs = format!(
        "[0:a]showwaves=s=1280x720:mode=cline:rate=30:colors=0x64C8AA:bgcolor=0x121216[wv];         [wv]subtitles=filename=\'{}\':force_style=\'Fontsize=20,PrimaryColour=&H00B4FFE8&\'[v]",
        ass_path
    );
    let filter_no_subs =
        "[0:a]showwaves=s=1280x720:mode=cline:rate=30:colors=0x64C8AA:bgcolor=0x121216[v]"
            .to_string();

    let mut ran_ok = false;
    for filter in [&filter_with_subs, &filter_no_subs] {
        let status = Command::new("ffmpeg")
            .args(["-y", "-i", wav_path,
                   "-filter_complex", filter,
                   "-map", "[v]", "-map", "0:a",
                   "-c:v", "libx264", "-c:a", "aac", "-b:a", "192k",
                   "-r", "30", &out])
            .output()?;
        if status.status.success() {
            ran_ok = true;
            break;
        }
        // If the subtitle filter failed, log and retry without it
        // Suppress stderr — eprintln corrupts the ratatui TUI
    }
    if !ran_ok {
        // Last resort: audio-only MP4
        let status = Command::new("ffmpeg")
            .args(["-y", "-i", wav_path,
                   "-c:a", "aac", "-b:a", "192k", &out])
            .output()?;
        if !status.status.success() {
            return Err(anyhow::anyhow!("ffmpeg failed: {}",
                String::from_utf8_lossy(&status.stderr)));
        }
    }
    Ok(out)
}

fn write_ass(
    path:            &str,
    log:             &[PhraseMoment],
    phrases:         &[Phrase],
    musical_indices: &[usize],
    total_secs:      f64,
) -> anyhow::Result<()> {
    let mut f = std::fs::File::create(path)?;
    writeln!(f, "[Script Info]\nScriptType: v4.00+\nPlayResX: 1280\nPlayResY: 720")?;
    writeln!(f, "[V4+ Styles]")?;
    writeln!(f, "Format: Name, Fontname, Fontsize, PrimaryColour, Bold, Alignment, MarginL, MarginT, MarginR, MarginV, BorderStyle, Outline, Shadow, Encoding")?;
    writeln!(f, "Style: Default,Consolas,22,&H00FFFFFF,0,7,20,10,20,10,1,1,0,1")?;
    writeln!(f, "[Events]")?;
    writeln!(f, "Format: Layer, Start, End, Style, Text")?;

    for (i, moment) in log.iter().enumerate() {
        let start_s = moment.sample as f64 / SR;
        let end_s   = if i + 1 < log.len() { log[i+1].sample as f64 / SR } else { total_secs };
        let start   = fmt_ass_time(start_s);
        let end     = fmt_ass_time(end_s);

        let mut lines: Vec<String> = Vec::new();
        for &mi in musical_indices {
            let p         = &phrases[mi];
            let is_active = mi == moment.phrase_idx;
            let style     = if is_active { "{\\c&H00B4FFE8&\\b1}" } else { "{\\c&H808080&\\b0}" };
            let marker    = if is_active { ">" } else { " " };
            let ctr       = if is_active && p.repeat > 1 {
                format!(" [{}/{}]", moment.play_num + 1, p.repeat)
            } else { String::new() };
            lines.push(format!("{style}{marker} {:>2}: {}{ctr}",
                p.id, p.src.chars().take(32).collect::<String>()));
        }
        writeln!(f, "Dialogue: 0,{start},{end},Default,{}", lines.join("\\N"))?;
    }
    Ok(())
}

fn fmt_ass_time(secs: f64) -> String {
    let h  = (secs / 3600.0) as u32;
    let m  = ((secs % 3600.0) / 60.0) as u32;
    let s  = (secs % 60.0) as u32;
    let cs = ((secs % 1.0) * 100.0) as u32;
    format!("{h}:{m:02}:{s:02}.{cs:02}")
}

fn write_wav_stereo(path: &str, samples: &[f32]) -> anyhow::Result<()> {
    let n_frames = (samples.len() / 2) as u32;
    let sr       = SR as u32;
    let data_len = n_frames * 4;
    let mut f = std::fs::File::create(path)?;
    f.write_all(b"RIFF")?; f.write_all(&(36 + data_len).to_le_bytes())?;
    f.write_all(b"WAVE")?; f.write_all(b"fmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?;
    f.write_all(&2u16.to_le_bytes())?;
    f.write_all(&sr.to_le_bytes())?;
    f.write_all(&(sr * 4).to_le_bytes())?;
    f.write_all(&4u16.to_le_bytes())?;
    f.write_all(&16u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&data_len.to_le_bytes())?;
    for &s in samples {
        f.write_all(&((s * 32767.0).clamp(-32768.0, 32767.0) as i16).to_le_bytes())?;
    }
    Ok(())
}
