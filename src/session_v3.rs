// session_v3.rs - explicit-ID session serialization helpers.
//
// V3 is the format the carpet renderer needs: every timeline line has a stable
// id.  The renderer must consume loaded Phrase values, not infer ids from raw
// anonymous text lines.

use crate::sequencer::{ControlSpec, Phrase};

pub const HEADER: &str = "MAQAM_SESSION_V3";

fn escape_field(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace('\n', "\\n")
}

pub fn serialize_session_v3(phrases: &[Phrase], vol: f32) -> String {
    let mut out = String::new();
    out.push_str(HEADER);
    out.push('\n');

    for (name, ratios) in crate::tuning::Maqam::list_custom() {
        let ratios_s = ratios
            .iter()
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
            out.push_str(&format!(
                "P|{}|{}|{}\n",
                p.id,
                p.repeat.max(1),
                escape_field(&p.src)
            ));
        }
    }

    out
}

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
    if esc {
        cur.push('\\');
    }
    fields.push(cur);
    fields
}
