// session_v3.rs - explicit-ID session serialization helpers.
//
// V3 is the format the carpet renderer needs: every timeline line has a stable
// id.  The renderer must consume loaded Phrase values, not infer ids from raw
// anonymous text lines.

use std::fs;
use std::path::Path;

use crate::sequencer::{ControlSpec, Phrase};

pub const HEADER: &str = "MAQAM_SESSION_V3";

fn escape_field(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace('\n', "\\n")
}

#[allow(dead_code)]
pub fn serialize_session_v3(phrases: &[Phrase], vol: f32) -> String {
    let mut out = String::new();
    out.push_str(HEADER);
    out.push('\n');

    for (name, ratios) in crate::tuning::Maqam::list_custom() {
        let ratios_s = ratios.iter()
            .map(|(p, q)| format!("{p}/{q}"))
            .collect::<Vec<_>>()
            .join(" ");
        out.push_str(&format!("create {name} {ratios_s}\n"));
    }

    out.push_str(&format!("vol {vol}\n"));

    for p in phrases {
        if let Some(j) = &p.jump {
            out.push_str(&format!("J|{}|{}|{}\n", p.id, j.target_id, j.times));
        } else if let Some(ctrl) = p.control {
            match ctrl {
                ControlSpec::SetBpm(v) => out.push_str(&format!("B|{}|{}\n", p.id, v)),
                ControlSpec::SetSustain(v) => out.push_str(&format!("S|{}|{}\n", p.id, v)),
            }
        } else {
            out.push_str(&format!("P|{}|{}|{}\n", p.id, p.repeat.max(1), escape_field(&p.src)));
        }
    }

    out
}

#[allow(dead_code)]
pub fn split_escaped_fields(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut cur = String::new();
    let mut esc = false;
    for ch in line.chars() {
        if esc {
            match ch {
                'n' => cur.push('\n'),
                '|' => cur.push('|'),
                '\\' => cur.push('\\'),
                other => {
                    cur.push('\\');
                    cur.push(other);
                }
            }
            esc = false;
        } else if ch == '\\' {
            esc = true;
        } else if ch == '|' {
            fields.push(cur);
            cur = String::new();
        } else {
            cur.push(ch);
        }
    }
    if esc { cur.push('\\'); }
    fields.push(cur);
    fields
}

fn split_repeat_suffix(line: &str) -> (String, usize) {
    let trimmed = line.trim_end();
    if let Some((head, tail)) = trimmed.rsplit_once(char::is_whitespace) {
        if let Some(n) = tail.strip_prefix('r') {
            if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) {
                if let Ok(repeat) = n.parse::<usize>() {
                    return (head.trim_end().to_string(), repeat.max(1));
                }
            }
        }
    }
    (trimmed.to_string(), 1)
}

pub fn upgrade_v2_text_to_v3(src: &str) -> Option<String> {
    let mut lines = src.lines();
    let header = lines.next()?.trim();
    if header == HEADER { return None; }
    if header != "MAQAM_SESSION_V2" { return None; }

    let mut out = String::new();
    out.push_str(HEADER);
    out.push('\n');
    let mut id = 0usize;

    for raw in lines {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        if line.starts_with("create ") || line.starts_with("vol ") {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if let Some(v) = line.strip_prefix("bpm ") {
            out.push_str(&format!("B|{}|{}\n", id, v.trim()));
            id += 1;
            continue;
        }
        if let Some(v) = line.strip_prefix("s ").or_else(|| line.strip_prefix("sus ")) {
            out.push_str(&format!("S|{}|{}\n", id, v.trim()));
            id += 1;
            continue;
        }
        if let Some(v) = line.strip_prefix("j ") {
            let mut parts = v.split_whitespace();
            if let (Some(target), Some(times)) = (parts.next(), parts.next()) {
                out.push_str(&format!("J|{}|{}|{}\n", id, target, times));
                id += 1;
                continue;
            }
        }
        let (src_line, repeat) = split_repeat_suffix(line);
        out.push_str(&format!("P|{}|{}|{}\n", id, repeat, escape_field(&src_line)));
        id += 1;
    }

    Some(out)
}

pub fn downgrade_v3_text_to_v1_for_current_loader(src: &str) -> Option<String> {
    let mut lines = src.lines();
    let header = lines.next()?.trim();
    if header != HEADER { return None; }

    let mut out = String::new();
    out.push_str("MAQAM_SESSION_V1\n");

    for raw in lines {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        if line.starts_with("create ") || line.starts_with("vol ") {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let fields = split_escaped_fields(line);
        match fields.first().map(|s| s.as_str()) {
            Some("B") if fields.len() >= 3 => {
                out.push_str(&format!("bpm {}\n", fields[2].trim()));
            }
            Some("S") if fields.len() >= 3 => {
                out.push_str(&format!("s {}\n", fields[2].trim()));
            }
            Some("J") if fields.len() >= 4 => {
                out.push_str(&format!("J|{}|{}|{}\n", fields[1].trim(), fields[2].trim(), fields[3].trim()));
            }
            Some("P") if fields.len() >= 4 => {
                out.push_str(&format!("P|{}|{}|{}\n", fields[1].trim(), fields[2].trim(), escape_field(fields[3].trim())));
            }
            _ => return None,
        }
    }

    Some(out)
}

pub fn normalize_v2_file_to_v3(path: impl AsRef<Path>) -> Result<bool, String> {
    let path = path.as_ref();
    let src = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let Some(upgraded) = upgrade_v2_text_to_v3(&src) else { return Ok(false); };
    fs::write(path, upgraded).map_err(|e| e.to_string())?;
    Ok(true)
}

pub fn downgrade_v3_file_to_v2_for_current_loader(path: impl AsRef<Path>) -> Result<bool, String> {
    let path = path.as_ref();
    let src = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let Some(downgraded) = downgrade_v3_text_to_v1_for_current_loader(&src) else { return Ok(false); };
    let backup = path.with_extension("mq.v3.bak");
    let _ = fs::write(&backup, &src);
    fs::write(path, downgraded).map_err(|e| e.to_string())?;
    Ok(true)
}

pub fn saved_path_from_message(msg: Option<&String>) -> Option<String> {
    let msg = msg?;
    let path = msg.strip_prefix("saved session → ")?;
    Some(path.trim().to_string())
}

pub fn normalize_saved_message(msg: Option<&String>) {
    if let Some(path) = saved_path_from_message(msg) {
        let _ = normalize_v2_file_to_v3(path);
    }
}
