// tuning.rs — just intonation tuning for oud-based maqam

use std::sync::{OnceLock, RwLock};
use std::collections::HashMap;

pub const D_HZ: f64 = 293.6648_f64;

// ── Pitch table ───────────────────────────────────────────────────────────────

const PITCH_TABLE: &[(&str, u32, u32)] = &[
    ("d",   1,   1), ("d+",  256, 243), ("e-",  256, 243),
    ("e¾",  12,  11), ("e",   9,   8),  ("f",   32,  27),
    ("f+",  81,  64), ("g-",  81,  64), ("g",   4,   3),
    ("g+",  1024,729),("a-",  1024,729),("a",   3,   2),
    ("a+",  128, 81), ("b-",  128, 81), ("b",   27,  16),
    ("c-",  27,  16), ("c",   16,  9),  ("c+",  1,   1),
];

fn pitch_ratio(letter: char, accidental: i8) -> (u32, u32) {
    let key: String = match accidental {
        1 => format!("{}+", letter), -1 => format!("{}-", letter),
        _ => letter.to_string(),
    };
    for (name, p, q) in PITCH_TABLE {
        if *name == key.as_str() { return (*p, *q); }
    }
    for (name, p, q) in PITCH_TABLE {
        if name.chars().next() == Some(letter) && name.len() == 1 { return (*p, *q); }
    }
    (1, 1)
}

pub fn pitch_to_hz(letter: char, accidental: i8, octave: u8) -> f64 {
    let (p, q) = pitch_ratio(letter, accidental);
    D_HZ * p as f64 / q as f64 * 2f64.powi(octave as i32 - 4)
}

pub fn snap_to_oud_lattice(nominal_hz: f64) -> f64 {
    let mut hz = nominal_hz;
    while hz < D_HZ        { hz *= 2.0; }
    while hz >= D_HZ * 2.0 { hz /= 2.0; }
    let ratio = hz / D_HZ;
    let mut best_hz = hz;
    let mut best_dist = f64::MAX;
    for &(_, p, q) in PITCH_TABLE {
        let r = p as f64 / q as f64;
        let dist = (r / ratio).log2().abs();
        if dist < best_dist { best_dist = dist; best_hz = D_HZ * r; }
    }
    best_hz
}

// ── Jins registry ─────────────────────────────────────────────────────────────

static REGISTRY: OnceLock<RwLock<HashMap<String, Vec<(u32, u32)>>>> = OnceLock::new();

fn registry() -> &'static RwLock<HashMap<String, Vec<(u32, u32)>>> {
    REGISTRY.get_or_init(|| {
        let mut m: HashMap<String, Vec<(u32, u32)>> = HashMap::new();
        m.insert("Nahawand".into(), vec![(1,1),(9,8),(32,27),(4,3),(3,2)]);
        m.insert("Bayati".into(),   vec![(1,1),(12,11),(32,27),(4,3),(3,2)]);
        m.insert("Hijaz".into(),    vec![(1,1),(256,243),(81,64),(4,3),(3,2)]);
        m.insert("Rast".into(),     vec![(1,1),(9,8),(27,22),(4,3),(3,2)]);
        m.insert("Kurd".into(),     vec![(1,1),(256,243),(32,27),(4,3),(3,2)]);
        m.insert("Saba".into(),     vec![(1,1),(13,12),(32,27),(80,64)]);
        m.insert("Zaba".into(),     vec![(1,1),(12,11),(32,27),(11,8)]);
        m.insert("Ajam".into(),     vec![(1,1),(9,8),(5,4),(4,3),(3,2)]);
        m.insert("Nikriz".into(),   vec![(1,1),(256,243),(81,64),(4,3),(3,2)]);
        m.insert("Suznak".into(),   vec![(1,1),(9,8),(27,22),(4,3),(3,2)]);
        m.insert("Jiharkah".into(), vec![(1,1),(9,8),(5,4),(4,3),(3,2)]);
        RwLock::new(m)
    })
}

// ── Maqam ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Maqam(pub String);

impl Maqam {
    pub fn new(name: &str) -> Self { Maqam(name.to_string()) }

    /// Case-insensitive prefix match against registry names.
    pub fn parse(s: &str) -> Option<Self> {
        let s_lower = s.to_ascii_lowercase();
        if s_lower.len() < 2 { return None; }
        let reg = registry().read().unwrap();
        // Exact match first
        for name in reg.keys() {
            if name.to_ascii_lowercase() == s_lower { return Some(Maqam(name.clone())); }
        }
        // Prefix match — pick shortest to avoid ambiguity
        let mut matches: Vec<&String> = reg.keys()
            .filter(|n| n.to_ascii_lowercase().starts_with(&s_lower))
            .collect();
        matches.sort_by_key(|n| n.len());
        matches.into_iter().next().map(|n| Maqam(n.clone()))
    }

    pub fn name(&self) -> &str { &self.0 }

    /// Ratios from the live registry — reflects runtime create/delete.
    pub fn ratios(&self) -> Vec<(u32, u32)> {
        registry().read().unwrap().get(&self.0).cloned().unwrap_or_default()
    }

    /// Sorted list of all registered jins.
    pub fn list_all() -> Vec<(String, Vec<(u32, u32)>)> {
        let reg = registry().read().unwrap();
        let mut v: Vec<_> = reg.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    /// Create or overwrite a jins entry.
    pub fn create(name: &str, ratios: Vec<(u32, u32)>) {
        registry().write().unwrap().insert(name.to_string(), ratios);
    }

    /// Delete a jins entry. Returns false if it didn't exist.
    pub fn delete(name: &str) -> bool {
        registry().write().unwrap().remove(name).is_some()
    }

    #[allow(dead_code)]
    pub fn degree_hz(&self, root_hz: f64, degree: usize) -> f64 {
        let ratios = self.ratios();
        if ratios.is_empty() { return root_hz; }
        let (p, q) = ratios[degree.min(ratios.len() - 1)];
        root_hz * p as f64 / q as f64
    }
}

// ── Pitch ─────────────────────────────────────────────────────────────────────

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

    pub fn to_hz(self) -> f64 { pitch_to_hz(self.letter, self.accidental, self.octave) }

    #[allow(dead_code)]
    pub fn display(self) -> String {
        let mut s = self.letter.to_ascii_uppercase().to_string();
        match self.accidental { 1 => s.push('+'), -1 => s.push('-'), _ => {} }
        s
    }
}
