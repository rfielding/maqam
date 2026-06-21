// command.rs — mini-language parser

use crate::fx::FxSettings;
use crate::tuning::{Maqam, Pitch};
use crate::vcf::{VcfBank, VcfSettings, VcfTarget, VcoWave};

pub struct JinsSpec {
    pub src: String,
    pub root: Pitch,
    pub maqam: Maqam,
    pub groups: Option<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct VcfChange {
    pub enabled: Option<bool>,
    pub target: Option<VcfTarget>,
    pub cutoff_hz: Option<ValueChange>,
    pub resonance: Option<ValueChange>,
    pub drive: Option<ValueChange>,
    pub wave: Option<VcoWave>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FxChange {
    pub reverb_enabled: Option<bool>,
    pub reverb_mix: Option<ValueChange>,
    pub reverb_decay: Option<ValueChange>,
    pub delay_enabled: Option<bool>,
    pub delay_time_secs: Option<ValueChange>,
    pub delay_feedback: Option<ValueChange>,
    pub delay_mix: Option<ValueChange>,
}

#[derive(Clone, Copy, Debug)]
pub enum ValueChange {
    Set(f64),
    Add(f64),
    Mul(f64),
    Div(f64),
    Tick(f64),
}

impl ValueChange {
    fn parse(token: &str, usage: &str) -> Result<Self, String> {
        let t = token.trim();
        if let Some(step) = t.strip_suffix('t') {
            let n = step.parse::<f64>().map_err(|_| usage.to_string())?;
            return Ok(ValueChange::Tick(n));
        }
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
            ValueChange::Tick(_) => Ok(current),
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
    InsertVcf {
        before: isize,
        change: VcfChange,
    },
    InsertFx {
        before: isize,
        change: FxChange,
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
    EditVcf {
        id: isize,
        change: VcfChange,
    },
    EditFx {
        id: isize,
        change: FxChange,
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
    SetVcf(VcfChange),
    SetFx(FxChange),
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
            Cmd::SetVcf(change) => Ok(Cmd::EditVcf { id, change }),
            Cmd::SetFx(change) => Ok(Cmd::EditFx { id, change }),
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
            Cmd::SetVcf(change) => Ok(Cmd::InsertVcf { before, change }),
            Cmd::SetFx(change) => Ok(Cmd::InsertFx { before, change }),
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

    // ── VCF ──────────────────────────────────────────────────────────────
    if matches!(
        al.as_str(),
        "vcf" | "filter" | "filt" | "cut" | "cutoff" | "res" | "q" | "drive" | "drv"
    ) && digits.is_empty()
    {
        return Ok(Cmd::SetVcf(parse_vcf_change(input)?));
    }

    // ── FX ───────────────────────────────────────────────────────────────
    if matches!(al.as_str(), "fx" | "reverb" | "rev" | "delay" | "pingpong") && digits.is_empty() {
        return Ok(Cmd::SetFx(parse_fx_change(input)?));
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

fn parse_vcf_change(input: &str) -> Result<VcfChange, String> {
    let usage = "usage: vcf [all|bass|kanun|kick] <cutoff> [res] [drive] | vcf [target] off | vcf bass cut=<hz|+n|-n|+nt> res=<0..1|+n|-n|+nt> drive=<n|+n|-n|+nt> wave=<shape> | cut <hz> | res <0..1> | drive <n>";
    let mut toks = input.split_whitespace();
    let head = toks.next().unwrap_or("").to_ascii_lowercase();
    let mut out = VcfChange::default();

    if matches!(head.as_str(), "cut" | "cutoff" | "filt" | "filter") {
        let tok = toks.next().ok_or(usage)?;
        out.enabled = Some(true);
        out.cutoff_hz = Some(ValueChange::parse(tok, usage)?);
        return Ok(out);
    }
    if matches!(head.as_str(), "res" | "q") {
        let tok = toks.next().ok_or(usage)?;
        out.enabled = Some(true);
        out.resonance = Some(ValueChange::parse(tok, usage)?);
        return Ok(out);
    }
    if matches!(head.as_str(), "drive" | "drv") {
        let tok = toks.next().ok_or(usage)?;
        out.enabled = Some(true);
        out.drive = Some(ValueChange::parse(tok, usage)?);
        return Ok(out);
    }

    let mut rest: Vec<&str> = toks.collect();
    if rest.is_empty() {
        return Err(usage.into());
    }
    if let Some(target) = rest.first().and_then(|tok| VcfTarget::parse(tok)) {
        out.target = Some(target);
        rest.remove(0);
        if rest.is_empty() {
            return Err(usage.into());
        }
    }
    if rest.len() == 1 && rest[0].eq_ignore_ascii_case("off") {
        out.enabled = Some(false);
        if out.target.is_none() {
            out.target = Some(VcfTarget::All);
        }
        return Ok(out);
    }

    let has_named = rest.iter().any(|tok| {
        tok.contains('=')
            || matches!(
                tok.to_ascii_lowercase().as_str(),
                "cut"
                    | "cutoff"
                    | "freq"
                    | "frequency"
                    | "res"
                    | "q"
                    | "reso"
                    | "resonance"
                    | "drive"
                    | "drv"
                    | "wave"
                    | "wav"
                    | "shape"
            )
    });

    if !has_named {
        out.enabled = Some(true);
        if rest.is_empty() {
            return Err(usage.into());
        }
        out.cutoff_hz = Some(ValueChange::parse(rest[0], usage)?);
        if let Some(tok) = rest.get(1) {
            out.resonance = Some(ValueChange::parse(tok, usage)?);
        }
        if let Some(tok) = rest.get(2) {
            out.drive = Some(ValueChange::parse(tok, usage)?);
        }
        if rest.len() > 3 {
            return Err(usage.into());
        }
        return Ok(out);
    }

    let mut positional = Vec::new();
    while let Some(tok) = rest.first() {
        let lower = tok.to_ascii_lowercase();
        let is_named_token = tok.contains('=')
            || matches!(
                lower.as_str(),
                "cut"
                    | "cutoff"
                    | "freq"
                    | "frequency"
                    | "res"
                    | "q"
                    | "reso"
                    | "resonance"
                    | "drive"
                    | "drv"
                    | "wave"
                    | "wav"
                    | "shape"
            );
        if is_named_token {
            break;
        }
        positional.push(rest.remove(0));
        if positional.len() > 3 {
            return Err(usage.into());
        }
    }
    if !positional.is_empty() {
        out.enabled = Some(true);
        out.cutoff_hz = Some(ValueChange::parse(positional[0], usage)?);
        if let Some(tok) = positional.get(1) {
            out.resonance = Some(ValueChange::parse(tok, usage)?);
        }
        if let Some(tok) = positional.get(2) {
            out.drive = Some(ValueChange::parse(tok, usage)?);
        }
    }

    let mut i = 0usize;
    while i < rest.len() {
        out.enabled = Some(true);
        let tok = rest[i];
        let (name, value) = if let Some((name, value)) = tok.split_once('=') {
            (name.to_ascii_lowercase(), value)
        } else {
            let name = tok.to_ascii_lowercase();
            i += 1;
            let value = rest.get(i).ok_or(usage)?;
            (name, *value)
        };
        match name.as_str() {
            "cut" | "cutoff" | "freq" | "frequency" => {
                out.cutoff_hz = Some(ValueChange::parse(value, usage)?)
            }
            "res" | "q" | "reso" | "resonance" => {
                out.resonance = Some(ValueChange::parse(value, usage)?)
            }
            "drive" | "drv" => out.drive = Some(ValueChange::parse(value, usage)?),
            "wave" | "wav" | "shape" => {
                out.wave = Some(VcoWave::parse(value).ok_or(usage)?);
            }
            _ => return Err(format!("unknown vcf parameter '{name}'")),
        }
        i += 1;
    }

    Ok(out)
}

pub fn apply_vcf_change(current: VcfBank, change: VcfChange) -> Result<VcfSettings, String> {
    let target = change.target.unwrap_or(current.focus);
    let current = current.get(target);
    let mut cutoff_step_per_tick = current.cutoff_step_per_tick;
    let cutoff_hz = match change.cutoff_hz {
        Some(ValueChange::Tick(step)) => {
            cutoff_step_per_tick = step as f32;
            current.cutoff_hz
        }
        Some(ValueChange::Add(0.0)) => {
            cutoff_step_per_tick = 0.0;
            current.cutoff_hz
        }
        Some(change) => change.apply(current.cutoff_hz as f64)? as f32,
        None => current.cutoff_hz,
    };
    if !(10.0..=22_000.0).contains(&cutoff_hz) {
        return Err(format!("vcf cutoff {cutoff_hz} Hz out of range 10..22000"));
    }

    let mut resonance_step_per_tick = current.resonance_step_per_tick;
    let resonance = match change.resonance {
        Some(ValueChange::Tick(step)) => {
            resonance_step_per_tick = step as f32;
            current.resonance
        }
        Some(ValueChange::Add(0.0)) => {
            resonance_step_per_tick = 0.0;
            current.resonance
        }
        Some(change) => change.apply(current.resonance as f64)? as f32,
        None => current.resonance,
    };
    if !(0.0..=0.98).contains(&resonance) {
        return Err(format!("vcf resonance {resonance} out of range 0..0.98"));
    }

    let mut drive_step_per_tick = current.drive_step_per_tick;
    let drive = match change.drive {
        Some(ValueChange::Tick(step)) => {
            drive_step_per_tick = step as f32;
            current.drive
        }
        Some(ValueChange::Add(0.0)) => {
            drive_step_per_tick = 0.0;
            current.drive
        }
        Some(change) => change.apply(current.drive as f64)? as f32,
        None => current.drive,
    };
    if !(0.1..=12.0).contains(&drive) {
        return Err(format!("vcf drive {drive} out of range 0.1..12"));
    }

    Ok(VcfSettings {
        enabled: change.enabled.unwrap_or(current.enabled),
        target,
        cutoff_hz,
        resonance,
        drive,
        cutoff_step_per_tick,
        resonance_step_per_tick,
        drive_step_per_tick,
        wave: change.wave.unwrap_or(current.wave),
    })
}

fn parse_fx_change(input: &str) -> Result<FxChange, String> {
    let usage = "usage: reverb mix=<0..1> decay=<0..0.98> | delay time=<secs> feedback=<0..0.95> mix=<0..1> | pingpong ... | reverb off | delay off | fx off";
    let mut toks = input.split_whitespace();
    let head = toks.next().unwrap_or("").to_ascii_lowercase();
    let rest: Vec<&str> = toks.collect();
    let mut out = FxChange::default();

    if head == "fx" {
        if rest.len() == 1 && rest[0].eq_ignore_ascii_case("off") {
            out.reverb_enabled = Some(false);
            out.delay_enabled = Some(false);
            return Ok(out);
        }
        return Err(usage.into());
    }

    let is_reverb = matches!(head.as_str(), "reverb" | "rev");
    let is_delay = matches!(head.as_str(), "delay" | "pingpong");
    if !is_reverb && !is_delay {
        return Err(usage.into());
    }
    if rest.len() == 1 && rest[0].eq_ignore_ascii_case("off") {
        if is_reverb {
            out.reverb_enabled = Some(false);
        } else {
            out.delay_enabled = Some(false);
        }
        return Ok(out);
    }
    if rest.is_empty() || (rest.len() == 1 && rest[0].eq_ignore_ascii_case("on")) {
        if is_reverb {
            out.reverb_enabled = Some(true);
        } else {
            out.delay_enabled = Some(true);
        }
        return Ok(out);
    }

    if is_reverb {
        out.reverb_enabled = Some(true);
    } else {
        out.delay_enabled = Some(true);
    }

    let mut i = 0usize;
    while i < rest.len() {
        let tok = rest[i];
        let (name, value) = if let Some((name, value)) = tok.split_once('=') {
            (name.to_ascii_lowercase(), value)
        } else {
            let name = tok.to_ascii_lowercase();
            i += 1;
            let value = rest.get(i).ok_or(usage)?;
            (name, *value)
        };
        let change = ValueChange::parse(value, usage)?;
        match name.as_str() {
            "mix" if is_reverb => out.reverb_mix = Some(change),
            "decay" | "room" | "feedback" if is_reverb => out.reverb_decay = Some(change),
            "time" | "t" | "secs" | "seconds" if is_delay => out.delay_time_secs = Some(change),
            "feedback" | "fb" if is_delay => out.delay_feedback = Some(change),
            "mix" if is_delay => out.delay_mix = Some(change),
            _ => return Err(format!("unknown fx parameter '{name}'")),
        }
        i += 1;
    }
    Ok(out)
}

pub fn apply_fx_change(current: FxSettings, change: FxChange) -> Result<FxSettings, String> {
    let mut next = current;
    if let Some(enabled) = change.reverb_enabled {
        next.reverb_enabled = enabled;
    }
    if let Some(enabled) = change.delay_enabled {
        next.delay_enabled = enabled;
    }
    apply_fx_value(
        &mut next.reverb_mix,
        &mut next.reverb_mix_step_per_tick,
        change.reverb_mix,
    )?;
    apply_fx_value(
        &mut next.reverb_decay,
        &mut next.reverb_decay_step_per_tick,
        change.reverb_decay,
    )?;
    apply_fx_value(
        &mut next.delay_time_secs,
        &mut next.delay_time_step_per_tick,
        change.delay_time_secs,
    )?;
    apply_fx_value(
        &mut next.delay_feedback,
        &mut next.delay_feedback_step_per_tick,
        change.delay_feedback,
    )?;
    apply_fx_value(
        &mut next.delay_mix,
        &mut next.delay_mix_step_per_tick,
        change.delay_mix,
    )?;
    validate_fx(next)
}

fn apply_fx_value(
    value: &mut f32,
    step: &mut f32,
    change: Option<ValueChange>,
) -> Result<(), String> {
    match change {
        Some(ValueChange::Tick(n)) => *step = n as f32,
        Some(ValueChange::Add(0.0)) => *step = 0.0,
        Some(change) => *value = change.apply(*value as f64)? as f32,
        None => {}
    }
    Ok(())
}

fn validate_fx(next: FxSettings) -> Result<FxSettings, String> {
    if !(0.0..=1.0).contains(&next.reverb_mix) {
        return Err(format!("reverb mix {} out of range 0..1", next.reverb_mix));
    }
    if !(0.0..=0.98).contains(&next.reverb_decay) {
        return Err(format!(
            "reverb decay {} out of range 0..0.98",
            next.reverb_decay
        ));
    }
    if !(0.01..=2.0).contains(&next.delay_time_secs) {
        return Err(format!(
            "delay time {}s out of range 0.01..2",
            next.delay_time_secs
        ));
    }
    if !(0.0..=0.95).contains(&next.delay_feedback) {
        return Err(format!(
            "delay feedback {} out of range 0..0.95",
            next.delay_feedback
        ));
    }
    if !(0.0..=1.0).contains(&next.delay_mix) {
        return Err(format!("delay mix {} out of range 0..1", next.delay_mix));
    }
    Ok(next)
}
