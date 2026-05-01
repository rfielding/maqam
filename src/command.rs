// command.rs — mini-language parser
//
// PITCH NAMES:  c d e f g a b   (plus # and b accidentals)
// KEYWORDS (reserved, not valid as pitches or maqam names):
//   bpm <n>          set tempo
//   s <n>            set sustain in seconds
//   x<N>             delete bar N  (also: x <N>)
//   r<N>             repeat suffix — NO space, e.g. r4  (avoid ambiguity)
//   clear            remove all bars
//   ?  help  q       misc
//
// ADD PHRASE:
//   <root> <maqam> [<groups>] [, <root> <maqam> [<groups>]] ...  [r<N>]
//
//   Comma connects ajnas into one unified scale (multiple modal centers,
//   one shared melody walk).  Rhythm is optional per-jins; missing rhythm
//   inherits from the nearest jins to the right that has one, then falls
//   back to the last phrase's rhythm.
//
//   Repeat r<N> — NO SPACE between r and N.  Bare numbers ≤ 20 at end of
//   phrase are also accepted as repeat for fast typing.
//
// EXAMPLES:
//   d bay,a nah 332 r4    D Bayati lower + A Nahawand upper, repeat 4×
//   c ajam r4             C Ajam, inherit rhythm, repeat 4×
//   c ajam 4              same — bare ≤20 = repeat
//   bpm 90                set tempo
//   s 1.0                 set sustain
//   x0                    delete bar 0

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
    Rotate,
    Record(usize),
    TogglePause,
    SetVol(f32),
    SetSustain(f64),
    DeleteBar(usize),
    SetBpm(f64),
    Clear,
    Help,
    Quit,
}

pub fn parse(raw: &str) -> Result<Cmd, String> {
    let input = raw.trim();
    if input.is_empty() { return Err("empty".into()); }

    match input {
        "q" | "quit"    => return Ok(Cmd::Quit),
        "?" | "help"    => return Ok(Cmd::Help),
        "clear"         => return Ok(Cmd::Clear),
        "rot"           => return Ok(Cmd::Rotate),
        "m"             => return Ok(Cmd::Record(1)),
        "z"             => return Ok(Cmd::TogglePause),

        _ => {}
    }

    // ── RECORD: m<N>  |  m <N>  ──────────────────────────────────────────
    // Handled before any tokenizing so strip_repeat can't consume the number.
    {
        let mut it = input.split_whitespace();
        if let Some(tok) = it.next() {
            let tok_l = tok.to_ascii_lowercase();
            let (a, d): (&str, &str) = if tok_l.starts_with('m') {
                ("m", &tok_l[1..])
            } else { ("", "") };
            if a == "m" {
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
    let alpha:  String = first.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
    let digits: String = first.chars().skip_while(|c| c.is_ascii_alphabetic()).collect();

    // ── BPM: requires full "bpm" keyword — keeps "b" free as pitch B ────
    if alpha.eq_ignore_ascii_case("bpm") {
        let n: f64 = input.split_whitespace().nth(1)
            .and_then(|s| s.parse().ok())
            .ok_or("usage: bpm <tempo>")?;
        if !(20.0..=400.0).contains(&n) {
            return Err(format!("bpm {n} out of range 20–400"));
        }
        return Ok(Cmd::SetBpm(n));
    }

    // ── SUSTAIN: "s <n>" — 's' is not a pitch name ───────────────────────
    if alpha.eq_ignore_ascii_case("s") || alpha.eq_ignore_ascii_case("sus") {
        if digits.is_empty() {
            let n: f64 = input.split_whitespace().nth(1)
                .and_then(|s| s.parse().ok())
                .ok_or("usage: s <seconds>")?;
            if !(0.05..=10.0).contains(&n) {
                return Err(format!("sustain {n}s out of range 0.05–10"));
            }
            return Ok(Cmd::SetSustain(n));
        }
    }

    // ── DELETE: x<N>  |  x <N>  — keeps "d" free as pitch D ─────────────
    if alpha.eq_ignore_ascii_case("x") {
        let id_str = if !digits.is_empty() {
            digits.clone()
        } else {
            input.split_whitespace().nth(1).unwrap_or("").to_string()
        };
        let id: usize = id_str.parse()
            .map_err(|_| format!("usage: x<N>  (e.g. x0 x1 x2)"))?;
        return Ok(Cmd::DeleteBar(id));
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
    Ok(Cmd::AddPhrase { specs: specs?, repeat })
}

/// Strip trailing repeat: r<N> (no space) or bare number ≤ 20.
/// Returns (remaining_input, repeat_count).
fn strip_repeat(input: &str) -> (&str, usize) {
    let toks: Vec<&str> = input.split_whitespace().collect();
    if toks.is_empty() { return (input, 1); }

    let last = *toks.last().unwrap();
    let la   = last.to_ascii_lowercase();
    let la_a: String = la.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
    let la_d: String = la.chars().skip_while(|c| c.is_ascii_alphabetic()).collect();

    // r<N> (no space) or bare number ≤ 20
    let (is_repeat, num_str): (bool, &str) = if la_a == "r" && !la_d.is_empty() {
        (true, &la_d)
    } else if la_a.is_empty() && !la_d.is_empty() {
        (false, &la_d)  // bare number
    } else {
        return (input, 1);
    };

    let Ok(n) = num_str.parse::<usize>() else { return (input, 1); };
    if !is_repeat && n > 20 { return (input, 1); }  // large number = rhythm, not repeat

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

    let maq_tok = toks.next().ok_or("missing maqam")?;
    let maqam   = Maqam::parse(maq_tok)
        .ok_or_else(|| format!("unknown maqam '{maq_tok}'  (nah bay hij rast kurd saba ajam)"))?;

    let groups = match toks.next() {
        None      => None,
        Some(tok) => {
            let g: Vec<u8> = tok.chars()
                .filter(|c| c.is_ascii_digit() && *c != '0')
                .map(|c| c as u8 - b'0')
                .collect();
            if g.is_empty() {
                return Err(format!("rhythm '{tok}' must be non-zero digits (e.g. 332)"));
            }
            Some(g)
        }
    };

    Ok(JinsSpec { src: part.trim().to_string(), root, maqam, groups })
}
