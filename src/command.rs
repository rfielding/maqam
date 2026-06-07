// command.rs — mini-language parser

use crate::tuning::{Maqam, Pitch};

pub struct JinsSpec {
    pub src: String,
    pub root: Pitch,
    pub maqam: Maqam,
    pub groups: Option<Vec<u8>>,
}

#[derive(Clone, Copy, Debug)]
pub enum ValueChange {
    Set(f64),
    Add(f64),
    Mul(f64),
    Div(f64),
}

impl ValueChange {
    fn parse(token: &str, usage: &str) -> Result<Self, String> {
        let t = token.trim();
        if t.len() >= 2 {
            let (op, rest) = t.split_at(1);
            let n = rest.parse::<f64>().map_err(|_| usage.to_string())?;
            return match op {
                "+" => Ok(ValueChange::Add(n)),
                "-" => Ok(ValueChange::Add(-n)),
                "*" => Ok(ValueChange::Mul(n)),
                "/" => Ok(ValueChange::Div(n)),
                _ => Ok(ValueChange::Set(
                    t.parse::<f64>().map_err(|_| usage.to_string())?,
                )),
            };
        }
        Ok(ValueChange::Set(
            t.parse::<f64>().map_err(|_| usage.to_string())?,
        ))
    }

    pub fn apply(self, current: f64) -> Result<f64, String> {
        match self {
            ValueChange::Set(n) => Ok(n),
            ValueChange::Add(n) => Ok(current + n),
            ValueChange::Mul(n) => Ok(current * n),
            ValueChange::Div(n) => {
                if n == 0.0 {
                    return Err("division by zero".into());
                }
                Ok(current / n)
            }
        }
    }
}

#[allow(dead_code)]
pub enum Cmd {
    AddPhrase {
        specs: Vec<JinsSpec>,
        repeat: usize,
    },
    Jump {
        to: isize,
        times: usize,
    },
    Insert {
        before: isize,
        specs: Vec<JinsSpec>,
        repeat: usize,
    },
    InsertBpm {
        before: isize,
        change: ValueChange,
    },
    InsertSustain {
        before: isize,
        change: ValueChange,
    },
    MoveUp(isize),
    MoveDown(isize),
    Edit {
        id: isize,
        specs: Vec<JinsSpec>,
        repeat: usize,
    },
    EditJump {
        id: isize,
        to: isize,
        times: usize,
    },
    EditBpm {
        id: isize,
        change: ValueChange,
    },
    EditSustain {
        id: isize,
        change: ValueChange,
    },
    InsertJump {
        before: isize,
        to: isize,
        times: usize,
    },
    DeleteBars(Vec<isize>),
    Rotate,
    SetBpm(ValueChange),
    SetSustain(ValueChange),
    SetVol(f32),
    Record(usize),
    TogglePause {
        start_id: Option<isize>,
    },
    ListJins,
    AuditionJins {
        specs: Vec<JinsSpec>,
    },
    CreateJins {
        name: String,
        ratios: Vec<(u32, u32)>,
    },
    DeleteJins {
        name: String,
    },
    Save {
        path: Option<String>,
    },
    Load {
        path: String,
    },
    Clear,
    Help,
    Quit,
}

pub fn parse(raw: &str) -> Result<Cmd, String> {
    let input = raw.trim();
    if input.is_empty() {
        return Err("empty".into());
    }

    // ── Exact keyword matches ─────────────────────────────────────────────
    match input {
        "q" | "quit" => return Ok(Cmd::Quit),
        "?" | "help" => return Ok(Cmd::Help),
        "clear" => return Ok(Cmd::Clear),
        "rot" => return Ok(Cmd::Rotate),
        "m" => return Ok(Cmd::Record(1)),
        _ => {}
    }

    // ── m<N> / m <N> ─────────────────────────────────────────────────────
    {
        let mut it = input.split_whitespace();
        if let Some(tok) = it.next() {
            let tl = tok.to_ascii_lowercase();
            if tl.starts_with('m') && !tl.starts_with("ma") {
                let d = &tl[1..];
                let repeat: usize = if !d.is_empty() {
                    d.parse().unwrap_or(1)
                } else {
                    it.next().and_then(|s| s.parse().ok()).unwrap_or(1)
                };
                return Ok(Cmd::Record(repeat.max(1)));
            }
        }
    }

    let first = input.split_whitespace().next().unwrap_or("");
    let alpha: String = first
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    let digits: String = first
        .chars()
        .skip_while(|c| c.is_ascii_alphabetic())
        .collect();
    let al = alpha.to_ascii_lowercase();

    // ── PAUSE: z [phrase-id] ──────────────────────────────────────────────
    if al == "z" {
        let start_id: Option<isize> = if !digits.is_empty() {
            Some(parse_id_ref(&digits, "usage: z [phrase-id]")?)
        } else {
            input
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse::<isize>().ok())
        };
        return Ok(Cmd::TogglePause { start_id });
    }

    // ── JUMP: j <pos> [<times>] ───────────────────────────────────────────
    if al == "j" {
        let to: isize = if !digits.is_empty() {
            parse_id_ref(&digits, "usage: j <pos> [times]")?
        } else {
            input
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse::<isize>().ok())
                .ok_or("usage: j <pos> [times]")?
        };
        let times_idx = if digits.is_empty() { 2 } else { 1 };
        let times: usize = input
            .split_whitespace()
            .nth(times_idx)
            .and_then(|s| s.parse().ok())
            .unwrap_or(1);
        return Ok(Cmd::Jump {
            to,
            times: times.max(1),
        });
    }

    // ── EDIT: edit <id> <cmd> ─────────────────────────────────────────────
    if al == "edit" {
        let mut toks = input.splitn(3, char::is_whitespace);
        toks.next(); // skip "edit"
        let id: isize = toks
            .next()
            .and_then(|s| s.parse().ok())
            .ok_or("usage: edit <id> <phrase|j target [times]|bpm n|s n>")?;
        let rest = toks.next().unwrap_or("").trim();
        if rest.is_empty() {
            return Err("usage: edit <id> <phrase|j target [times]|bpm n|s n>".into());
        }

        return match parse(rest)? {
            Cmd::AddPhrase { specs, repeat } => Ok(Cmd::Edit { id, specs, repeat }),
            Cmd::Jump { to, times } => Ok(Cmd::EditJump { id, to, times }),
            Cmd::SetBpm(change) => Ok(Cmd::EditBpm { id, change }),
            Cmd::SetSustain(change) => Ok(Cmd::EditSustain { id, change }),
            _ => Err("unsupported command after edit".into()),
        };
    }

    // ── REORDER: up/down <id> ─────────────────────────────────────────────
    if al == "up" {
        let id = input
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse::<isize>().ok())
            .ok_or("usage: up <id>")?;
        return Ok(Cmd::MoveUp(id));
    }
    if al == "down" {
        let id = input
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse::<isize>().ok())
            .ok_or("usage: down <id>")?;
        return Ok(Cmd::MoveDown(id));
    }

    // ── INSERT: i<pos> <cmd> ──────────────────────────────────────────────
    if al == "i" {
        let before: isize;
        let rest: &str;
        if !digits.is_empty() {
            before = parse_id_ref(&digits, "usage: i<pos> <phrase|j target times|bpm n|s n>")?;
            rest = input[first.len()..].trim();
        } else {
            let mut toks = input.splitn(3, char::is_whitespace);
            toks.next();
            before = toks
                .next()
                .and_then(|s| s.parse::<isize>().ok())
                .ok_or("usage: i <pos> <phrase|j target times|bpm n|s n>")?;
            rest = toks.next().unwrap_or("").trim();
        }
        if rest.is_empty() {
            return Err("usage: i <pos> <phrase|j target times|bpm n|s n>".into());
        }

        return match parse(rest)? {
            Cmd::AddPhrase { specs, repeat } => Ok(Cmd::Insert {
                before,
                specs,
                repeat,
            }),
            Cmd::Jump { to, times } => Ok(Cmd::InsertJump { before, to, times }),
            Cmd::SetBpm(change) => Ok(Cmd::InsertBpm { before, change }),
            Cmd::SetSustain(change) => Ok(Cmd::InsertSustain { before, change }),
            _ => Err("unsupported command after insert".into()),
        };
    }

    // ── BPM ───────────────────────────────────────────────────────────────
    if al == "bpm" {
        let tok = input
            .split_whitespace()
            .nth(1)
            .ok_or("usage: bpm <tempo|*k|/k|+n|-n>")?;
        let change = ValueChange::parse(tok, "usage: bpm <tempo|*k|/k|+n|-n>")?;
        return Ok(Cmd::SetBpm(change));
    }

    // ── SUSTAIN ───────────────────────────────────────────────────────────
    if (al == "s" || al == "sus") && digits.is_empty() {
        let tok = input
            .split_whitespace()
            .nth(1)
            .ok_or("usage: s <secs|*k|/k|+n|-n>")?;
        let change = ValueChange::parse(tok, "usage: s <secs|*k|/k|+n|-n>")?;
        return Ok(Cmd::SetSustain(change));
    }

    // ── VOL ───────────────────────────────────────────────────────────────
    if al == "vol" {
        let n: f32 = if !digits.is_empty() {
            digits.parse().unwrap_or(1.0)
        } else {
            input
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(1.0)
        };
        if !(0.0..=2.0).contains(&n) {
            return Err(format!("vol {n} out of range 0–2"));
        }
        return Ok(Cmd::SetVol(n));
    }

    // ── DELETE: x<N> [N …] ───────────────────────────────────────────────
    if al == "x" {
        let mut ids: Vec<isize> = Vec::new();
        if !digits.is_empty() {
            ids.push(parse_id_ref(&digits, "usage: x<N> [N …]")?);
        }
        let mut toks = input.split_whitespace();
        toks.next();
        for tok in toks {
            ids.push(tok.parse().map_err(|_| format!("bad id '{tok}'"))?);
        }
        if ids.is_empty() {
            return Err("usage: x<N>".into());
        }
        return Ok(Cmd::DeleteBars(ids));
    }

    // ── LS: list all jins ─────────────────────────────────────────────────
    if input == "ls" {
        return Ok(Cmd::ListJins);
    }

    // ── AUDITION: audition <phrase-spec> ─────────────────────────────────
    if al == "audition" {
        let rest = input
            .splitn(2, char::is_whitespace)
            .nth(1)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or("usage: audition <Name> | audition <root> <Name> [, <root> <Name> ...]")?;

        let specs: Result<Vec<JinsSpec>, String> =
            if Pitch::parse(rest.split_whitespace().next().unwrap_or("")).is_some() {
                rest.split(',').map(|p| parse_jins_spec(p.trim())).collect()
            } else {
                let maqam = Maqam::parse(rest).ok_or_else(|| format!("unknown maqam '{rest}'"))?;
                Ok(vec![JinsSpec {
                    src: format!("d {}", maqam.name()),
                    root: Pitch {
                        letter: 'd',
                        accidental: 0,
                        octave: 4,
                    },
                    maqam,
                    groups: None,
                }])
            };
        return Ok(Cmd::AuditionJins { specs: specs? });
    }

    // ── CREATE: create <Name> <p/q> <p/q> … ──────────────────────────────
    if al == "create" {
        let mut toks = input.split_whitespace();
        toks.next(); // skip "create"
        let name = toks
            .next()
            .ok_or("usage: create <Name> <ratios…>")?
            .to_string();
        let ratios: Result<Vec<(u32, u32)>, String> = toks
            .map(|t| parse_ratio(t).ok_or_else(|| format!("bad ratio '{t}'")))
            .collect();
        let ratios = ratios?;
        if ratios.is_empty() {
            return Err("need at least one ratio".into());
        }
        return Ok(Cmd::CreateJins { name, ratios });
    }

    // ── DELETE: delete <Name> ─────────────────────────────────────────────
    if al == "delete" {
        let name = input
            .split_whitespace()
            .nth(1)
            .ok_or("usage: delete <Name>")?
            .to_string();
        return Ok(Cmd::DeleteJins { name });
    }

    // ── SAVE / LOAD ───────────────────────────────────────────────────────
    if al == "save" {
        let path = input
            .splitn(2, char::is_whitespace)
            .nth(1)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        return Ok(Cmd::Save { path });
    }
    if al == "load" {
        let path = input
            .splitn(2, char::is_whitespace)
            .nth(1)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .ok_or("usage: load <path>")?
            .to_string();
        let _ = crate::session_v3::downgrade_v3_file_to_v2_for_current_loader(&path);
        return Ok(Cmd::Load { path });
    }

    // ── ADD PHRASE ────────────────────────────────────────────────────────
    let (phrase_part, repeat) = strip_repeat(input);
    if phrase_part.is_empty() {
        return Err("empty phrase".into());
    }
    let specs: Result<Vec<JinsSpec>, String> = phrase_part
        .split(',')
        .map(|p| parse_jins_spec(p.trim()))
        .collect();
    Ok(Cmd::AddPhrase {
        specs: specs?,
        repeat,
    })
}

fn parse_id_ref(token: &str, usage: &str) -> Result<isize, String> {
    token.parse::<isize>().map_err(|_| usage.to_string())
}

fn strip_repeat(input: &str) -> (&str, usize) {
    let toks: Vec<&str> = input.split_whitespace().collect();
    if toks.is_empty() {
        return (input, 1);
    }
    let last = *toks.last().unwrap();
    let la = last.to_ascii_lowercase();
    let la_a: String = la.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
    let la_d: String = la.chars().skip_while(|c| c.is_ascii_alphabetic()).collect();
    let (is_r, num_s): (bool, &str) = if la_a == "r" && !la_d.is_empty() {
        (true, &la_d)
    } else if la_a.is_empty() && !la_d.is_empty() {
        (false, &la_d)
    } else {
        return (input, 1);
    };
    let Ok(n) = num_s.parse::<usize>() else {
        return (input, 1);
    };
    if !is_r && n > 20 {
        return (input, 1);
    }
    let trimmed = input.trim_end();
    let pos = trimmed.rfind(last).unwrap_or(trimmed.len());
    let remaining = trimmed[..pos].trim_end();
    (remaining, n.max(1))
}

fn parse_jins_spec(part: &str) -> Result<JinsSpec, String> {
    let mut toks = part.split_whitespace();
    let root_tok = toks.next().ok_or("missing pitch")?;
    let root = Pitch::parse(root_tok).ok_or_else(|| format!("unknown pitch '{root_tok}'"))?;
    let maq_tok = toks.next().ok_or("missing maqam")?;
    let maqam = Maqam::parse(maq_tok)
        .ok_or_else(|| format!("unknown maqam '{maq_tok}'  (nah bay hij rast kurd saba ajam)"))?;
    let groups = match toks.next() {
        None => None,
        Some(tok) => {
            let g: Vec<u8> = tok
                .chars()
                .filter(|c| c.is_ascii_digit() && *c != '0')
                .map(|c| c as u8 - b'0')
                .collect();
            if g.is_empty() {
                return Err(format!("rhythm '{tok}' must be non-zero digits"));
            }
            Some(g)
        }
    };
    Ok(JinsSpec {
        src: part.trim().to_string(),
        root,
        maqam,
        groups,
    })
}

fn parse_ratio(s: &str) -> Option<(u32, u32)> {
    let mut parts = s.splitn(2, '/');
    let p = parts.next()?.parse::<u32>().ok()?;
    let q = parts
        .next()
        .map(|q| q.parse::<u32>().ok())
        .flatten()
        .unwrap_or(1);
    if q == 0 {
        return None;
    }
    Some((p, q))
}
