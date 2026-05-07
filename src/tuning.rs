// tuning.rs — just intonation tuning for oud-based maqam

// ── Oud reference pitch ───────────────────────────────────────────────────────
//
// The oud open strings are a pure-fourth chain rooted on D:
//   A = D × 3/4   (220.249 Hz — a fourth below D)
//   D = 293.6648  (matches electronic tuner D)
//   G = D × 4/3   (391.553 Hz)
//   C = D × 16/9  (522.071 Hz — two fourths above D)
//
// All pitches in the system are expressed as ratios from D_HZ.
// Rule: prefer 3-smooth (Pythagorean) ratios; use 5-smooth or 12/11 only when
// the note cannot be reached by a 4/3 chain.

pub const D_HZ: f64 = 293.6648_f64;

// ── Chromatic pitch table (all ratios from D, reduced to [1,2)) ───────────────
//
// These are the ONLY valid root frequencies. Every maqam root snaps to one
// of these values × 2^n for the appropriate octave.
//
//  Note   Ratio    Cents   Source
//  D      1/1        0     open string
//  D#/Eb  256/243   90     Pythagorean minor 2nd
//  E¾     12/11    151     11-limit neutral 2nd (Bayati, Saba characteristic)
//  E      9/8      204     Pythagorean major 2nd
//  F      32/27    294     Pythagorean minor 3rd (= C×4/3, i.e. (4/3)^3 from D)
//  F#     81/64    408     Pythagorean major 3rd
//  G      4/3      498     open string
//  Ab     1024/729 588     Pythagorean diminished 5th
//  A      3/2      702     open string (A above D; open A string is D×3/4)
//  Bb     128/81   792     Pythagorean minor 6th
//  B      27/16    906     Pythagorean major 6th
//  C      16/9     996     open string ((4/3)^2)

const PITCH_TABLE: &[(&str, u32, u32)] = &[
    ("d",   1,   1),
    ("d+",  256, 243),  // D# / Eb
    ("e-",  256, 243),  // Eb = D+
    ("e¾",  12,  11),   // neutral E (internal use for maqam intervals)
    ("e",   9,   8),
    ("f",   32,  27),
    ("f+",  81,  64),   // F#
    ("g-",  81,  64),   // Gb = F#
    ("g",   4,   3),
    ("g+",  1024,729),  // G#/Ab
    ("a-",  1024,729),
    ("a",   3,   2),
    ("a+",  128, 81),   // A# / Bb — same as Bb
    ("b-",  128, 81),   // Bb
    ("b",   27,  16),
    ("c-",  27,  16),   // Cb = B (rare)
    ("c",   16,  9),
    ("c+",  1,   1),    // C# ≈ D (rare; wraps to octave above)
];

/// Return the D-based ratio (p, q) for a pitch letter + accidental.
/// The ratio is reduced to [1, 2) so it lives in the D4 register.
fn pitch_ratio(letter: char, accidental: i8) -> (u32, u32) {
    let key: String = match accidental {
        1  => format!("{}+", letter),
        -1 => format!("{}-", letter),
        _  => letter.to_string(),
    };
    for (name, p, q) in PITCH_TABLE {
        if *name == key.as_str() { return (*p, *q); }
    }
    // Default: treat as natural
    for (name, p, q) in PITCH_TABLE {
        if name.chars().next() == Some(letter) && name.len() == 1 {
            return (*p, *q);
        }
    }
    (1, 1)
}

/// Convert a Pitch to its exact JI frequency in Hz.
///
/// The octave field specifies which octave: 4 = D4 register [D4, D5).
/// Note: C is at 16/9 from D, so C4 here = D4×16/9 = 522 Hz (C5 in concert).
/// This is intentional — we use the oud's natural register.
pub fn pitch_to_hz(letter: char, accidental: i8, octave: u8) -> f64 {
    let (p, q) = pitch_ratio(letter, accidental);
    let base   = D_HZ * p as f64 / q as f64;
    // Octave 4 = same register as D4.
    // Since ratio is in [1,2), base is already in [D4, 2×D4).
    // Octave offset: 4 = reference.
    base * 2f64.powi(octave as i32 - 4)
}

/// Snap any Hz value to the nearest entry in our pitch table,
/// returning the result in the SAME register as D_HZ (i.e. [D_HZ, D_HZ×2)).
///
/// This is the ONLY way notes enter the system — always expressed as
/// an exact D-based ratio.
pub fn snap_to_oud_lattice(nominal_hz: f64) -> f64 {
    // Reduce nominal to same register as D4: [D_HZ, D_HZ×2)
    let mut hz = nominal_hz;
    while hz < D_HZ       { hz *= 2.0; }
    while hz >= D_HZ * 2.0 { hz /= 2.0; }

    let ratio = hz / D_HZ; // in [1, 2)

    let mut best_hz   = hz;
    let mut best_dist = f64::MAX;

    for &(_, p, q) in PITCH_TABLE {
        let r = p as f64 / q as f64;
        // Each entry is already in [1,2) by construction
        let dist = (r / ratio).log2().abs();
        if dist < best_dist {
            best_dist = dist;
            best_hz   = D_HZ * r;
        }
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
    Nikriz,     // Hijaz lower + natural upper (B not Bb)
    Suznak,     // Rast lower + Hijaz upper (Bb + C#)
    Jiharkah,   // Ajam lower + flat 7th (C not C#)
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

    /// JI ratios for scale degrees 0..7.
    /// All intervals are exact — computed against the jins root.
    /// Combined scales use tetrachord stacking (each jins contributes
    /// notes only within its range), so interval clashes are impossible.
    pub fn ratios(self) -> [(u32, u32); 8] {
        match self {
            // Nahawand: Pythagorean natural minor
            // D E F G A Bb C D  →  0,204,294,498,702,792,996,1200¢
            Maqam::Nahawand => [(1,1),(9,8),(32,27),(4,3),(3,2),(128,81),(16,9),(2,1)],

            // Bayati: neutral 2nd (12/11), characteristic of Arabic music
            // D E¾ F G A Bb C D  →  0,151,355,498,702,853,996,1200¢
            // Note: 27/22 = 9/8 × 3/11... but more naturally expressed as
            // two 12/11 stacked: (12/11)^2 = 144/121 ≈ 299¢, close to 32/27=294¢
            // We use 27/22 for the authentic sound.
            Maqam::Bayati   => [(1,1),(12,11),(27,22),(4,3),(3,2),(18,11),(16,9),(2,1)],

            // Hijaz: augmented 2nd between degrees 2 and 3
            // D Eb F# G A Bb B D  →  0,90,408,498,702,792,1088,1200¢
            // Note: using Pythagorean Eb (256/243) and F# (81/64) for oud resonance
            Maqam::Hijaz    => [(1,1),(256,243),(81,64),(4,3),(3,2),(128,81),(243,128),(2,1)],

            // Rast: neutral 3rd (27/22), whole tone 2nd
            // D E F# G A B Bb D  →  0,204,355,498,702,906,996,1200¢
            // (Pythagorean 6th: 27/16)
            Maqam::Rast     => [(1,1),(9,8),(27,22),(4,3),(3,2),(27,16),(16,9),(2,1)],

            // Kurd: Pythagorean Phrygian — flat 2nd (256/243)
            // D Eb F G A Bb C D  →  0,90,294,498,702,792,996,1200¢
            Maqam::Kurd     => [(1,1),(256,243),(32,27),(4,3),(3,2),(128,81),(16,9),(2,1)],

            // Saba: distinctive diminished 4th (11/8)
            // D E¾ F- (11/8) A Bb C D  →  0,151,267,551,702,792,996,1200¢
            Maqam::Saba     => [(1,1),(12,11),(7,6),(11,8),(3,2),(128,81),(16,9),(2,1)],

            // Ajam: 5-limit major scale
            // D E F# G A B C# D  →  0,204,386,498,702,884,1088,1200¢
            Maqam::Ajam     => [(1,1),(9,8),(5,4),(4,3),(3,2),(5,3),(15,8),(2,1)],

            // Nikriz: Hijaz lower tetrachord + natural upper (B♮ instead of B♭)
            // D E♭ F# G A B C D  →  0,90,408,498,702,906,996,1200¢
            // Lower = Hijaz; upper = Pythagorean (same as Nahawand upper)
            // Reference: maqamworld.com/en/nikriz
            Maqam::Nikriz   => [(1,1),(256,243),(81,64),(4,3),(3,2),(27,16),(16,9),(2,1)],

            // Suznak: Rast lower tetrachord + Hijaz upper tetrachord
            // D E F¾ G A B♭ C# D  →  0,204,355,498,702,792,1110,1200¢
            // Lower = Rast (neutral 3rd); upper = Hijaz (augmented 2nd)
            // Reference: maqamworld.com/en/suznak
            Maqam::Suznak   => [(1,1),(9,8),(27,22),(4,3),(3,2),(128,81),(243,128),(2,1)],

            // Jiharkah (Jaharkah): Ajam lower + flat 7th (C♮ not C#)
            // D E F# G A B(5/3) C D  →  0,204,386,498,702,884,996,1200¢
            // The flat 7th gives it a major-pentatonic feel with Mixolydian flavour.
            // Reference: maqamworld.com/en/jiharkah
            Maqam::Jiharkah => [(1,1),(9,8),(5,4),(4,3),(3,2),(5,3),(16,9),(2,1)],
        }
    }

    #[allow(dead_code)]
    pub fn degree_hz(self, root_hz: f64, degree: usize) -> f64 {
        let (p, q) = self.ratios()[degree.min(7)];
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

    /// Exact JI Hz — always from the oud lattice.
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
