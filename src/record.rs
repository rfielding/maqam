// record.rs — offline render to MP4

use std::collections::HashMap;
use std::io::Write;
use std::process::{Command, Stdio};

use crate::sequencer::Phrase;
use crate::synth::{evolve_bar, spawn_phrase_start, spawn_sub_bass, spawn_voices, Milestone, Voice};

const SR: f64 = 44100.0;

// ── Sequence expansion ────────────────────────────────────────────────────────

fn expand_one_cycle(phrases: &[Phrase]) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    let mut cur: usize = 0;
    let mut jc: HashMap<usize, usize> = HashMap::new();
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
                let ids: Vec<usize> = phrases[target..cur].iter()
                    .filter_map(|p| p.jump.as_ref().map(|_| p.id)).collect();
                for id in ids { jc.remove(&id); }
                cur = target;
            } else {
                jc.remove(&pid);
                cur += 1;
            }
            continue;
        }
        out.push(cur);
        cur += 1;
        if cur >= phrases.len() { break; }
    }
    out
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub fn record_cycle(
    phrases:      &[Phrase],
    bpm:          f64,
    sustain:      f64,
    cycle_repeat: usize,
) -> anyhow::Result<String> {
    if phrases.is_empty() {
        return Err(anyhow::anyhow!("nothing to record"));
    }

    let subdiv_secs    = 60.0 / (bpm * 2.0);
    let subdiv_samples = SR * subdiv_secs;

    let bar_samples_for = |idx: usize| -> usize {
        ((subdiv_samples * phrases[idx].bar.total_subdivs as f64).round() as usize).max(1)
    };

    let one_cycle_seq = expand_one_cycle(phrases);
    if one_cycle_seq.is_empty() {
        return Err(anyhow::anyhow!("no musical phrases to render"));
    }

    let cycles       = cycle_repeat.max(1);
    let tail_samples = (SR * (sustain + 1.0)) as usize;

    // Build flat play sequence: (phrase_idx, play_num)
    let mut full_seq: Vec<(usize, usize)> = Vec::new();
    for _ in 0..cycles {
        for &idx in &one_cycle_seq {
            for play in 0..phrases[idx].repeat.max(1) {
                full_seq.push((idx, play));
            }
        }
    }

    // ── Render ────────────────────────────────────────────────────────────────
    let mut phrases_v = phrases.to_vec();
    let mut voices: Vec<Voice> = Vec::new();
    let mut left_buf:  Vec<f32> = Vec::new();
    let mut right_buf: Vec<f32> = Vec::new();

    for (seq_pos, &(phrase_idx, play_num)) in full_seq.iter().enumerate() {
        let bs       = bar_samples_for(phrase_idx);
        let is_first = play_num == 0;
        let repeats  = phrases_v[phrase_idx].repeat.max(1);

        if is_first {
            let root_hz = phrases_v[phrase_idx].bar.root_hz;
            let phrase_secs = (phrases_v[phrase_idx].bar.total_subdivs as f64
                * subdiv_secs * repeats as f64).min(3.0);
            spawn_phrase_start(root_hz, sustain, &mut voices);
            spawn_sub_bass(root_hz, phrase_secs, &mut voices);
        }

        let total_subdivs = phrases_v[phrase_idx].bar.total_subdivs;
        let mut bar_pos: usize = 0;
        let mut last_subdiv: Option<usize> = None;

        for _ in 0..bs {
            let ev = if total_subdivs > 0 {
                let curr = ((bar_pos as f64 / subdiv_samples) as usize).min(total_subdivs - 1);
                let ev = if last_subdiv != Some(curr) {
                    last_subdiv = Some(curr);
                    let is_last_play   = play_num + 1 >= repeats;
                    let is_last_subdiv = curr + 1 >= total_subdivs;
                    let milestone = if is_first && curr == 0 { Milestone::PhraseStart }
                                   else if is_last_play && is_last_subdiv { Milestone::Turnaround }
                                   else { Milestone::None };
                    phrases_v[phrase_idx].bar.events.get(curr).copied().map(|e| (e, milestone))
                } else { None };
                bar_pos += 1;
                ev
            } else { None };

            if let Some((ev, milestone)) = ev {
                spawn_voices(ev, sustain, &mut voices, milestone,
                             &phrases_v[phrase_idx].bar.frequencies);
            }

            let (mut l, mut r) = (0f32, 0f32);
            for v in voices.iter_mut() {
                let s     = v.sample(SR);
                let angle = (v.pan + 1.0) * std::f32::consts::FRAC_PI_4;
                l += s * angle.cos();
                r += s * angle.sin();
            }
            left_buf.push(l);
            right_buf.push(r);
            voices.retain(|v| !v.done);
        }

        evolve_bar(&mut phrases_v[phrase_idx].bar, true);
        let _ = seq_pos; // suppress warning
    }

    // Final "1"
    if let Some(&(first_idx, _)) = full_seq.first() {
        let root_hz = phrases_v[first_idx].bar.root_hz;
        spawn_phrase_start(root_hz, sustain, &mut voices);
        spawn_sub_bass(root_hz, sustain.min(2.0), &mut voices);
    }
    for _ in 0..tail_samples {
        let (mut l, mut r) = (0f32, 0f32);
        for v in voices.iter_mut() {
            let s     = v.sample(SR);
            let angle = (v.pan + 1.0) * std::f32::consts::FRAC_PI_4;
            l += s * angle.cos();
            r += s * angle.sin();
        }
        left_buf.push(l);
        right_buf.push(r);
        voices.retain(|v| !v.done);
    }

    // ── Normalize + write 16-bit PCM WAV ─────────────────────────────────────
    // Normalize to 90% of full scale so the waveform is clearly visible.
    let peak = left_buf.iter().chain(right_buf.iter())
        .map(|s| s.abs()).fold(0f32, f32::max);
    let gain = if peak > 0.001 { 0.9 / peak } else { 1.0 };

    let wav_path = "/tmp/maqam-live.wav";
    {
        let n     = left_buf.len() as u32;
        let sr    = SR as u32;
        let dl    = n * 4; // 2 channels × 2 bytes per sample
        let mut f = std::fs::File::create(wav_path)?;
        // RIFF header
        f.write_all(b"RIFF")?; f.write_all(&(36 + dl).to_le_bytes())?;
        f.write_all(b"WAVE")?;
        // fmt chunk — PCM s16
        f.write_all(b"fmt ")?;
        f.write_all(&16u32.to_le_bytes())?; // chunk size
        f.write_all(&1u16.to_le_bytes())?;  // PCM
        f.write_all(&2u16.to_le_bytes())?;  // stereo
        f.write_all(&sr.to_le_bytes())?;
        f.write_all(&(sr * 4).to_le_bytes())?; // byte rate
        f.write_all(&4u16.to_le_bytes())?;  // block align
        f.write_all(&16u16.to_le_bytes())?; // bits
        // data chunk
        f.write_all(b"data")?;
        f.write_all(&dl.to_le_bytes())?;
        for i in 0..left_buf.len() {
            let l = (left_buf[i]  * gain * 32767.0).clamp(-32768.0, 32767.0) as i16;
            let r = (right_buf[i] * gain * 32767.0).clamp(-32768.0, 32767.0) as i16;
            f.write_all(&l.to_le_bytes())?;
            f.write_all(&r.to_le_bytes())?;
        }
        f.flush()?;
        f.sync_all()?;
    }

    // ── ASS subtitle file ─────────────────────────────────────────────────────
    let ass_path = "/tmp/maqam-live.ass";
    {
        let total_secs = left_buf.len() as f64 / SR;
        let mut f      = std::fs::File::create(ass_path)?;
        writeln!(f, "[Script Info]")?;
        writeln!(f, "ScriptType: v4.00+")?;
        writeln!(f, "PlayResX: 1280")?;
        writeln!(f, "PlayResY: 720")?;
        writeln!(f, "WrapStyle: 0")?;
        writeln!(f, "[V4+ Styles]")?;
        writeln!(f, "Format: Name,Fontname,Fontsize,PrimaryColour,SecondaryColour,OutlineColour,BackColour,Bold,Italic,Underline,Strikeout,ScaleX,ScaleY,Spacing,Angle,BorderStyle,Outline,Shadow,Alignment,MarginL,MarginR,MarginV,Encoding")?;
        writeln!(f, "Style: Default,Courier New,22,&H00FFFFFF,&H00FFFFFF,&H00000000,&H80000000,0,0,0,0,100,100,0,0,1,1,0,7,20,20,10,1")?;
        writeln!(f, "[Events]")?;
        writeln!(f, "Format: Layer,Start,End,Style,Name,MarginL,MarginR,MarginV,Effect,Text")?;

        let mut sample: usize = 0;
        for (i, &(phrase_idx, play_num)) in full_seq.iter().enumerate() {
            let bs      = bar_samples_for(phrase_idx);
            let start_s = sample as f64 / SR;
            let end_s   = if i + 1 < full_seq.len() {
                (sample + bs) as f64 / SR
            } else { total_secs };

            // Cycle counter: which pass through the full sequence are we on
        let one_len: usize = one_cycle_seq.iter()
            .map(|&idx| phrases[idx].repeat.max(1)).sum();
        let cycle_num  = if one_len > 0 { i / one_len } else { 0 };
        let cycle_disp = if cycles > 1 {
            format!("  cycle {}/{}", cycle_num + 1, cycles)
        } else { String::new() };

        let mut lines: Vec<String> = Vec::new();
        // Header line: shows overall position
        let tag_hdr = "{\\c&H606060&\\i1}";
        let tag_rst = "{\\r}";
        lines.push(format!("{tag_hdr}bpm:{} sus:{:.1}s{}{tag_rst}",
            (60.0 / (subdiv_secs * 2.0)) as u32, sustain, cycle_disp));
        for (pi, p) in phrases.iter().enumerate() {
                if let Some(js) = &p.jump {
                    let tag = "{\\c&H707070&}";
                    lines.push(format!("{tag}  {:>2}: >>{}  x{}", p.id, js.to_pos, js.times));
                } else {
                    let active = pi == phrase_idx;
                    if active {
                        let ctr = if p.repeat > 1 {
                            format!("  [{}/{}]", play_num + 1, p.repeat)
                        } else { String::new() };
                        let tag_on  = "{\\c&H00E8FFB4&\\b1}";
                        let tag_off = "{\\r}";
                        lines.push(format!("{tag_on}> {:>2}: {}{ctr}{tag_off}", p.id, p.src, ctr=ctr));
                    } else {
                        let tag = "{\\c&H909090&}";
                        lines.push(format!("{tag}  {:>2}: {}", p.id, p.src));
                    }
                }
            }
            let text = lines.join("\\N");
            let h  = |s: f64| -> String {
                let hh = (s/3600.0) as u32;
                let mm = ((s%3600.0)/60.0) as u32;
                let ss = (s%60.0) as u32;
                let cs = ((s%1.0)*100.0) as u32;
                format!("{hh}:{mm:02}:{ss:02}.{cs:02}")
            };
            writeln!(f, "Dialogue: 0,{},{},Default,,0,0,0,,{}", h(start_s), h(end_s), text)?;
            sample += bs;
        }
        f.flush()?;
    }

    // ── ffmpeg ────────────────────────────────────────────────────────────────
    let ts  = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let out = format!("{}/maqam-{ts}.mp4",
        std::env::var("HOME").unwrap_or_else(|_| ".".into()));

    let sh_path = "/tmp/maqam-render.sh";

    // Primary: waveform + subtitle overlay
    let sh = format!(concat!(
        "#!/bin/sh
",
        "ffmpeg -y -i '{wav}' \
",
        "  -filter_complex '[0:a]showwaves=s=1280x720:mode=cline:rate=30:colors=white[wv];",
                             "[wv]subtitles={ass}[v]' \
",
        "  -map '[v]' -map '0:a' \
",
        "  -c:v libx264 -crf 18 -c:a aac -b:a 256k -r 30 '{out}' \
",
        "  > /tmp/maqam-ffmpeg.log 2>&1
"
    ), wav=wav_path, ass=ass_path, out=out);

    std::fs::write(sh_path, &sh)?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(sh_path, std::fs::Permissions::from_mode(0o755))?;
    }

    let ok = Command::new("sh").arg(sh_path)
        .stdout(Stdio::null()).stderr(Stdio::null())
        .status().map(|s| s.success()).unwrap_or(false);

    if !ok {
        // Fallback: waveform only (no subtitles)
        let sh2 = format!(concat!(
            "#!/bin/sh
",
            "ffmpeg -y -i '{wav}' \
",
            "  -filter_complex '[0:a]showwaves=s=1280x720:mode=cline:rate=30:colors=white[v]' \
",
            "  -map '[v]' -map '0:a' \
",
            "  -c:v libx264 -crf 18 -c:a aac -b:a 256k -r 30 '{out}' \
",
            "  >> /tmp/maqam-ffmpeg.log 2>&1
"
        ), wav=wav_path, out=out);
        std::fs::write(sh_path, &sh2)?;
        Command::new("sh").arg(sh_path)
            .stdout(Stdio::null()).stderr(Stdio::null())
            .status()?;
    }

    Ok(out)
}
