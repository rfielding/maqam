// tuning.rs — just intonation pitch classes and maqam scales

pub const A4_HZ: f64 = 440.0;

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
}

impl Maqam {
    /// Parse a prefix-ambiguous token, e.g. "nah" → Nahawand, "bay" → Bayati.
    /// Minimum 2 chars required to avoid single-letter collisions.
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
        ];
        for (name, kind) in candidates {
            if name.starts_with(s.as_str()) {
                return Some(*kind);
            }
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
        }
    }

    /// JI ratios for scale degrees 0‥7 (degree 7 = octave = 2/1).
    pub fn ratios(self) -> [(u32, u32); 8] {
        match self {
            // 1  9/8  6/5  4/3  3/2  8/5  9/5  2
            // Nahawand: Pythagorean minor — matches oud open string fourths
            Maqam::Nahawand => [(1,1),(9,8),(32,27),(4,3),(3,2),(128,81),(16,9),(2,1)],
            // 1  12/11  27/22  4/3  3/2  18/11  16/9  2   (bayati quarter-tone 2nd ≈ 12/11)
            Maqam::Bayati   => [(1,1),(12,11),(27,22),(4,3),(3,2),(18,11),(16,9),(2,1)],
            // 1  16/15  5/4  4/3  3/2  8/5  15/8  2
            Maqam::Hijaz    => [(1,1),(16,15),(5,4),(4,3),(3,2),(8,5),(15,8),(2,1)],
            // 1  9/8  27/22  4/3  3/2  27/16  18/11  2
            Maqam::Rast     => [(1,1),(9,8),(27,22),(4,3),(3,2),(27,16),(16,9),(2,1)],
            // 1  12/11  32/27  4/3  3/2  128/81  16/9  2
            // Kurd: Pythagorean minor with neutral 2nd (characteristic oud interval)
            // Kurd: Pythagorean Phrygian — low minor 2nd (256/243=90¢), matches oud fourths
            Maqam::Kurd     => [(1,1),(256,243),(32,27),(4,3),(3,2),(128,81),(16,9),(2,1)],
            // 1  12/11  7/6  11/8  3/2  128/81  16/9  2
            Maqam::Saba     => [(1,1),(12,11),(7,6),(11,8),(3,2),(128,81),(16,9),(2,1)],
            // Ajam: major scale in JI — 1 9/8 5/4 4/3 3/2 5/3 15/8 2
            Maqam::Ajam     => [(1,1),(9,8),(5,4),(4,3),(3,2),(5,3),(15,8),(2,1)],
        }
    }

    /// Frequency for a scale degree (0=root, 4=dominant, 7=octave).
#[allow(dead_code)]
    pub fn degree_hz(self, root_hz: f64, degree: usize) -> f64 {
        let (p, q) = self.ratios()[degree.min(7)];
        root_hz * p as f64 / q as f64
    }
}

// ── Pitch class ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct Pitch {
    pub letter:     char, // c d e f g a b
    pub accidental: i8,   // +1=sharp, -1=flat, 0=natural
    pub octave:     u8,
}

impl Pitch {
    /// Parse a pitch token.  Default octave = 4.
    ///   Sharp: d+  d+4   (also # accepted)
    ///   Flat:  d-  d-4   (b is pitch B, not flat)
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

    /// Equal-tempered Hz (for kick-drum root etc.).  JI offsets are applied
    /// separately via Maqam::degree_hz.
    pub fn to_hz(self) -> f64 {
        let st: i32 = match self.letter {
            'c' => -9, 'd' => -7, 'e' => -5, 'f' => -4,
            'g' => -2, 'a' =>  0, 'b' =>  2, _   =>  0,
        } + self.accidental as i32
          + (self.octave as i32 - 4) * 12;
        A4_HZ * 2f64.powf(st as f64 / 12.0)
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
