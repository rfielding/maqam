// command.rs — mini-language parser
//
// Keyword dispatch (checked before any pitch parsing):
//
//   bpm <n>              set tempo
//   s <n>                set sustain seconds
//   vol <n>              set output volume (0–2)
//   x<N> [N …]          delete phrase(s) at list positions
//   j <pos> [<times>]    append jump entry: when reached, loop to pos N times
//   j<pos> [<times>]     same with attached digit
//   i <pos> <phrase>     insert phrase before list position
//   i<pos> <phrase>      same with attached digit
//   rot                  rotate: last phrase moves to front
//   m [<N>]              record N cycles to MP4
//   z                    toggle pause
//   clear                clear all phrases
//   ?  help  q           misc
//
// ADD PHRASE (everything else):
//   <root> <maqam> [<groups>] [, …]  [r<N> | bare ≤20]
//
//   root:   c d e f g a b  with + for sharp, - for flat
//   maqam:  prefix-matched nah bay hij ras kur sab aja
//   groups: additive 8th-note rhythm e.g. 332 3234 44
//   repeat: r4 or bare number ≤20 at end

use crate::tuning::{Maqam, Pitch};

pub struct JinsSpec {
    pub src:    String,
    pub root:   Pitch,
    pub maqam:  Maqam,
    pub groups: Option<Vec<u8>>,
}

#[allow(dead_code)]
pub enum Cmd {
    AddPhrase { specs: Vec<JinsSpec>, repeat: usize },
    Jump   { to: usize, times: usize },
    Insert { before: usize, specs: Vec<JinsSpec>, repeat: usize },
    DeleteBars(Vec<usize>),
    Rotate,
    SetBpm(f64),
    SetSustain(f64),
    SetVol(f32),
    Record(usize),
    TogglePause,
    Clear,
    Help,
    Quit,
}

pub fn parse(raw: &str) -> Result<Cmd, String> {
    let input = raw.trim();
    if input.is_empty() { return Err("empty".into()); }

    // ── Exact keyword matches ─────────────────────────────────────────────
    match input {
        "q" | "quit" => return Ok(Cmd::Quit),
        "?" | "help" => return Ok(Cmd::Help),
        "clear"      => return Ok(Cmd::Clear),
        "rot"        => return Ok(Cmd::Rotate),
        "m"          => return Ok(Cmd::Record(1)),
        "z"          => return Ok(Cmd::TogglePause),
        _            => {}
    }

    // ── m<N> / m <N>  (before tokenising so strip_repeat can't eat the N) ─
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

    // All remaining commands use first-token dispatch via alpha/digits split
    let first  = input.split_whitespace().next().unwrap_or("");
    let alpha: String = first.chars().take_while( |c| c.is_ascii_alphabetic()).collect();
    let digits: String= first.chars().skip_while(|c| c.is_ascii_alphabetic()).collect();
    let al = alpha.to_ascii_lowercase();

    // ── JUMP: j <pos> [<times>]  |  j<pos> [<times>] ─────────────────────
    if al == "j" {
        let to: usize = if !digits.is_empty() {
            digits.parse().map_err(|_| "usage: j <pos> [times]")?
        } else {
            input.split_whitespace().nth(1)
                .and_then(|s| s.parse().ok())
                .ok_or("usage: j <pos> [times]")?
        };
        let times_idx = if digits.is_empty() { 2 } else { 1 };
        let times: usize = input.split_whitespace().nth(times_idx)
            .and_then(|s| s.parse().ok()).unwrap_or(1);
        return Ok(Cmd::Jump { to, times: times.max(1) });
    }

    // ── INSERT: i<pos> <cmd>  |  i <pos> <cmd> ───────────────────────────
    if al == "i" {
        let before: usize;
        let rest: &str;
        if !digits.is_empty() {
            before = digits.parse().map_err(|_| "usage: i<pos> <phrase>")?;
            rest   = input[first.len()..].trim();
        } else {
            let mut toks = input.splitn(3, char::is_whitespace);
            toks.next();
            before = toks.next().and_then(|s| s.parse().ok())
                .ok_or("usage: i <pos> <phrase>")?;
            rest   = toks.next().unwrap_or("").trim();
        }
        if rest.is_empty() { return Err("usage: i <pos> <phrase>".into()); }
        let (phrase_part, repeat) = strip_repeat(rest);
        let specs: Result<Vec<JinsSpec>, String> = phrase_part
            .split(',').map(|p| parse_jins_spec(p.trim())).collect();
        return Ok(Cmd::Insert { before, specs: specs?, repeat });
    }

    // ── BPM ───────────────────────────────────────────────────────────────
    if al == "bpm" {
        let n: f64 = input.split_whitespace().nth(1)
            .and_then(|s| s.parse().ok())
            .ok_or("usage: bpm <tempo>")?;
        if !(20.0..=400.0).contains(&n) { return Err(format!("bpm {n} out of range")); }
        return Ok(Cmd::SetBpm(n));
    }

    // ── SUSTAIN ───────────────────────────────────────────────────────────
    if (al == "s" || al == "sus") && digits.is_empty() {
        let n: f64 = input.split_whitespace().nth(1)
            .and_then(|s| s.parse().ok())
            .ok_or("usage: s <secs>")?;
        if !(0.05..=10.0).contains(&n) { return Err(format!("sustain {n}s out of range")); }
        return Ok(Cmd::SetSustain(n));
    }

    // ── VOL ───────────────────────────────────────────────────────────────
    if al == "vol" {
        let n: f32 = if !digits.is_empty() {
            digits.parse().unwrap_or(1.0)
        } else {
            input.split_whitespace().nth(1)
                .and_then(|s| s.parse().ok()).unwrap_or(1.0)
        };
        if !(0.0..=2.0).contains(&n) { return Err(format!("vol {n} out of range 0–2")); }
        return Ok(Cmd::SetVol(n));
    }

    // ── DELETE: x<N> [N …] ───────────────────────────────────────────────
    if al == "x" {
        let mut ids: Vec<usize> = Vec::new();
        if !digits.is_empty() {
            ids.push(digits.parse().map_err(|_| "usage: x<N> [N …]")?);
        }
        let mut toks = input.split_whitespace();
        toks.next();
        for tok in toks {
            ids.push(tok.parse().map_err(|_| format!("bad id '{tok}'"))?);
        }
        if ids.is_empty() { return Err("usage: x<N>".into()); }
        return Ok(Cmd::DeleteBars(ids));
    }

    // ── ADD PHRASE ────────────────────────────────────────────────────────
    let (phrase_part, repeat) = strip_repeat(input);
    if phrase_part.is_empty() { return Err("empty phrase".into()); }
    let specs: Result<Vec<JinsSpec>, String> = phrase_part
        .split(',').map(|p| parse_jins_spec(p.trim())).collect();
    Ok(Cmd::AddPhrase { specs: specs?, repeat })
}

/// Strip trailing repeat: r<N> (no space) or bare number ≤20.
fn strip_repeat(input: &str) -> (&str, usize) {
    let toks: Vec<&str> = input.split_whitespace().collect();
    if toks.is_empty() { return (input, 1); }
    let last = *toks.last().unwrap();
    let la   = last.to_ascii_lowercase();
    let la_a: String = la.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
    let la_d: String = la.chars().skip_while(|c| c.is_ascii_alphabetic()).collect();
    let (is_r, num_s): (bool, &str) = if la_a == "r" && !la_d.is_empty() {
        (true, &la_d)
    } else if la_a.is_empty() && !la_d.is_empty() {
        (false, &la_d)
    } else {
        return (input, 1);
    };
    let Ok(n) = num_s.parse::<usize>() else { return (input, 1); };
    if !is_r && n > 20 { return (input, 1); }
    let trimmed = input.trim_end();
    let pos = trimmed.rfind(last).unwrap_or(trimmed.len());
    let remaining = trimmed[..pos].trim_end();
    (remaining, n.max(1))
}

fn parse_jins_spec(part: &str) -> Result<JinsSpec, String> {
    let mut toks = part.split_whitespace();
    let root_tok = toks.next().ok_or("missing pitch")?;
    let root     = Pitch::parse(root_tok)
        .ok_or_else(|| format!("unknown pitch '{root_tok}'"))?;
    let maq_tok  = toks.next().ok_or("missing maqam")?;
    let maqam    = Maqam::parse(maq_tok)
        .ok_or_else(|| format!("unknown maqam '{maq_tok}'  (nah bay hij rast kurd saba ajam)"))?;
    let groups = match toks.next() {
        None      => None,
        Some(tok) => {
            let g: Vec<u8> = tok.chars()
                .filter(|c| c.is_ascii_digit() && *c != '0')
                .map(|c| c as u8 - b'0').collect();
            if g.is_empty() { return Err(format!("rhythm '{tok}' must be non-zero digits")); }
            Some(g)
        }
    };
    Ok(JinsSpec { src: part.trim().to_string(), root, maqam, groups })
}
