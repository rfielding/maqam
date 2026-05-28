// tuning.rs — just intonation tuning for oud-based maqam

pub const D_HZ: f64 = 293.6648_f64;

// ── Chromatic pitch table ─────────────────────────────────────────────────────
//
//  Note   Ratio    Cents
//  D      1/1        0     open string
//  D#/Eb  256/243   90     Pythagorean minor 2nd
//  E¾     12/11    151     11-limit neutral 2nd
//  E      9/8      204     Pythagorean major 2nd
//  F      32/27    294     Pythagorean minor 3rd
//  F#     81/64    408     Pythagorean major 3rd
//  G      4/3      498     open string
//  Ab     1024/729 588     Pythagorean diminished 5th
//  A      3/2      702     open string
//  Bb     128/81   792     Pythagorean minor 6th
//  B      27/16    906     Pythagorean major 6th
//  C      16/9     996     open string

const PITCH_TABLE: &[(&str, u32, u32)] = &[
    ("d",   1,   1),
    ("d+",  256, 243),
    ("e-",  256, 243),
    ("e¾",  12,  11),
    ("e",   9,   8),
    ("f",   32,  27),
    ("f+",  81,  64),
    ("g-",  81,  64),
    ("g",   4,   3),
    ("g+",  1024,729),
    ("a-",  1024,729),
    ("a",   3,   2),
    ("a+",  128, 81),
    ("b-",  128, 81),
    ("b",   27,  16),
    ("c-",  27,  16),
    ("c",   16,  9),
    ("c+",  1,   1),
];

fn pitch_ratio(letter: char, accidental: i8) -> (u32, u32) {
    let key: String = match accidental {
        1  => format!("{}+", letter),
        -1 => format!("{}-", letter),
        _  => letter.to_string(),
    };
    for (name, p, q) in PITCH_TABLE {
        if *name == key.as_str() { return (*p, *q); }
    }
    for (name, p, q) in PITCH_TABLE {
        if name.chars().next() == Some(letter) && name.len() == 1 {
            return (*p, *q);
        }
    }
    (1, 1)
}

pub fn pitch_to_hz(letter: char, accidental: i8, octave: u8) -> f64 {
    let (p, q) = pitch_ratio(letter, accidental);
    let base   = D_HZ * p as f64 / q as f64;
    base * 2f64.powi(octave as i32 - 4)
}

pub fn snap_to_oud_lattice(nominal_hz: f64) -> f64 {
    let mut hz = nominal_hz;
    while hz < D_HZ        { hz *= 2.0; }
    while hz >= D_HZ * 2.0 { hz /= 2.0; }
    let ratio = hz / D_HZ;
    let mut best_hz   = hz;
    let mut best_dist = f64::MAX;
    for &(_, p, q) in PITCH_TABLE {
        let r    = p as f64 / q as f64;
        let dist = (r / ratio).log2().abs();
        if dist < best_dist { best_dist = dist; best_hz = D_HZ * r; }
    }
    best_hz
}

// ── Maqam ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Maqam {
    Nahawand,
    Bayati,
    Hijaz,
    Rast,
    Kurd,
    Saba,
    Ajam,
    Nikriz,
    Suznak,
    Jiharkah,
}

impl Maqam {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.to_ascii_lowercase();
        if s.len() < 2 { return None; }
        let candidates: &[(&str, Maqam)] = &[
            ("nahawand", Maqam::Nahawand),
            ("bayati",   Maqam::Bayati),
            ("hijaz",    Maqam::Hijaz),
            ("rast",     Maqam::Rast),
            ("kurd",     Maqam::Kurd),
            ("saba",     Maqam::Saba),
            ("ajam",     Maqam::Ajam),
            ("nikriz",   Maqam::Nikriz),
            ("suznak",   Maqam::Suznak),
            ("jiharkah", Maqam::Jiharkah),
            ("jiharka",  Maqam::Jiharkah),
            ("jaharkah", Maqam::Jiharkah),
        ];
        for (name, kind) in candidates {
            if name.starts_with(s.as_str()) { return Some(*kind); }
        }
        None
    }

    pub fn name(self) -> &'static str {
        match self {
            Maqam::Nahawand => "Nahawand",
            Maqam::Bayati   => "Bayati",
            Maqam::Hijaz    => "Hijaz",
            Maqam::Rast     => "Rast",
            Maqam::Kurd     => "Kurd",
            Maqam::Saba     => "Saba",
            Maqam::Ajam     => "Ajam",
            Maqam::Nikriz   => "Nikriz",
            Maqam::Suznak   => "Suznak",
            Maqam::Jiharkah => "Jiharkah",
        }
    }

    /// JI ratios for this jins.  The array length IS the jins size:
    ///   5 notes  — root through 5th — for jins with a perfect 4th (4/3).
    ///              The 5th grounds the harmonic field; 2nd and 4th define colour.
    ///   4 notes  — root through the characteristic note — when the 4th is
    ///              altered (Saba's tritone 11/8).  Adding the 5th after a
    ///              tritone creates an augmented-2nd step that breaks the jins.
    /// A chromatic jins would simply have 12 entries.
    pub fn ratios(self) -> &'static [(u32, u32)] {
        match self {
            // ── 5-note jins (root, 2nd, 3rd, 4th, 5th) ────────────────────
            Maqam::Nahawand => &[(1,1),(9,8),(32,27),(4,3),(3,2)],
            Maqam::Bayati   => &[(1,1),(12,11),(32,27),(4,3),(3,2)],
            Maqam::Hijaz    => &[(1,1),(256,243),(81,64),(4,3),(3,2)],
            Maqam::Rast     => &[(1,1),(9,8),(27,22),(4,3),(3,2)],
            Maqam::Kurd     => &[(1,1),(256,243),(32,27),(4,3),(3,2)],
            Maqam::Ajam     => &[(1,1),(9,8),(5,4),(4,3),(3,2)],
            // Nikriz/Suznak/Jiharkah lower = Hijaz/Rast/Ajam;
            // the upper jins is supplied by the second comma-spec.
            Maqam::Nikriz   => &[(1,1),(256,243),(81,64),(4,3),(3,2)],
            Maqam::Suznak   => &[(1,1),(9,8),(27,22),(4,3),(3,2)],
            Maqam::Jiharkah => &[(1,1),(9,8),(5,4),(4,3),(3,2)],
            // ── 4-note jins (tritone/flat-4th endpoint) ───────────────────
            Maqam::Saba     => &[(1,1),(12,11),(32,27),(11,8)],
        }
    }

    #[allow(dead_code)]
    pub fn degree_hz(self, root_hz: f64, degree: usize) -> f64 {
        let ratios  = self.ratios();
        let (p, q)  = ratios[degree.min(ratios.len().saturating_sub(1))];
        root_hz * p as f64 / q as f64
    }
}

// ── Pitch ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct Pitch {
    pub letter:     char,
    pub accidental: i8,
    pub octave:     u8,
}

impl Pitch {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.to_ascii_lowercase();
        let mut it = s.chars().peekable();
        let letter = it.next()?;
        if !"cdefgab".contains(letter) { return None; }
        let mut accidental = 0i8;
        match it.peek().copied() {
            Some('+') | Some('#') => { accidental =  1; it.next(); }
            Some('-')             => { accidental = -1; it.next(); }
            _ => {}
        }
        let octave = match it.next() {
            Some(d) if d.is_ascii_digit() => d as u8 - b'0',
            None => 4,
            _ => return None,
        };
        Some(Pitch { letter, accidental, octave })
    }

    pub fn to_hz(self) -> f64 {
        pitch_to_hz(self.letter, self.accidental, self.octave)
    }

    #[allow(dead_code)]
    pub fn display(self) -> String {
        let mut s = self.letter.to_ascii_uppercase().to_string();
        match self.accidental {
            1  => s.push('+'),
            -1 => s.push('-'),
            _  => {}
        }
        s
    }
}
