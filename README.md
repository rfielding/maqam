# maqam-live

A live-coding terminal sequencer for Arabic maqam music, built on just intonation.

Type a phrase, hear it immediately. Add more phrases while the first loops. The
melody evolves slowly on its own — staying in key, drifting toward and away from
the dominant, accenting the turnaround before each cycle reset. Every pitch is a
pure JI ratio. The beating between simultaneous voices is the sound of the
instrument.

```
┌ maqam-live ─────────────────────────────────────────────────────────────────┐
│  0: d bay, a nah 332    X..X..X.  ×4  Bayati+Nahawand                      │
│  1: c rast              X..X..X.  ×2  Rast                                 │
├─ cmd ───────────────────────────────────────────────────────────────────────┤
│ > _                                                                         │
│  BPM:120 sus:1.2s phrases:2  [?] help                                      │
│  ◉ ~/maqam-1234567890.mp4                                                   │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## Install

**Dependencies:**

```bash
sudo apt install libasound2-dev pkg-config ffmpeg   # Ubuntu/Debian
```

**Build:**

```bash
tar -xzf maqam-live.tar.gz
cd maqam-live
cargo build --release
cargo run --release
```

The `rust-toolchain.toml` pins to Rust 1.85.0. `rustup` will download it
automatically on first build.

---

## Language

### Add a phrase

```
<root> <maqam> [<groups>] [, <root> <maqam> [<groups>]] ...  [r<N>]
```

- **root** — pitch name: `c d e f g a b`, with `+` for sharp, `-` for flat.
  `d+` = D♯, `b-` = B♭.
- **maqam** — prefix-matched: `nah` → Nahawand, `bay` → Bayati, `hij` → Hijaz,
  `ras` → Rast, `kur` → Kurd, `sab` → Saba, `aja` → Ajam.
- **groups** — additive rhythm (8th notes): `332` = 3+3+2 = 8/8,
  `44` = 4+4 = 8/8, `3234` = 3+2+3+4 = 12/8. Omit to inherit from the
  previous phrase.
- **r\<N\>** — repeat the phrase N times before advancing. No space. `r4` not
  `r 4`.

**Comma = stacked ajnas, not sequential.** Commas build one combined JI scale
from multiple smaller scales connected at their coincidence points:

```
d nah, a nah 332 r4     D Nahawand lower + A Nahawand upper, one scale, 4×
d bay, a nah 332        Bayati lower + Nahawand upper (common in Arabic music)
c ajam, g nah 44 r2     Ajam lower + Nahawand upper, square rhythm
```

Each subsequent jins root is snapped to the nearest JI frequency already in the
scale, preserving exact intonation across the combined scale.

### Other commands

| Command | Effect |
|---|---|
| `bpm <n>` | Set tempo (20–400). Default 120. |
| `s <n>` | Set melody sustain in seconds (0.05–10). Default 1.25. |
| `x<N>` | Delete phrase N (e.g. `x0`). |
| `rot` | Move last phrase to front, shift everything else down. |
| `m` | Record one full cycle to `~/maqam-<timestamp>.mp4`. |
| `m<N>` | Record N full cycles (e.g. `m4`). |
| `clear` | Remove all phrases. |
| `?` | Show help in status bar. |
| `q` | Quit. |

**Up/Down arrows** scroll through command history.

---

## How it sounds

### Just intonation

All pitches are exact rational ratios — `root_hz × p/q`. The maqam scales are
tuned to historical JI practice:

| Maqam | Characteristic interval |
|---|---|
| Nahawand | Minor scale, 6/5 third |
| Bayati | Quarter-tone 2nd (12/11), gives the "Arabic" sound |
| Hijaz | Augmented 2nd (75/64), flamenco-adjacent |
| Rast | Neutral 3rd (27/22), classical Arabic |
| Kurd | Low minor, like Phrygian |
| Saba | Diminished 4th (11/8), very distinctive |
| Ajam | JI major scale |

When two voices play simultaneously, their overtones either lock (JI intervals)
or beat slowly (near-JI intervals). This beating is the shimmer you hear — it is
not a bug. At a perfect 5th (3/2), the beating rate is zero. At a minor third
(6/5), it is slow and warm. The combined effect across a full phrase is a slowly
evolving bell chord.

### Three-level rhythm/melody hierarchy

```
Subdivision  →  melody note (degree walk, snares interpolate between kicks)
Group start  →  structural degree + JI chord (root, 5th, octave)
Phrase start →  long root voice + sub-bass two octaves below (the "1")
Phrase end   →  turnaround accent (marks the return to the beginning)
```

The melody walk ascends from tonic to dominant and back across the full phrase,
so the musical arc spans the whole cycle, not just one bar.

### Evolution

After every complete phrase play, the melody degrees drift slightly:
- **70%** — gentle drift: interior notes shift ±1 scale degree at 15% probability
- **20%** — fill: a brief excursion to a higher degree and back (turnaround feel)
- **8%** — reset: fresh arc with a new random peak

First and last degrees always return to tonic. The penultimate group before each
phrase boundary rises to the dominant (degree 4) 50% of the time — the classical
maqam cadential motion.

### Sub-bass

A pure sine wave two octaves below the phrase root plays for the full phrase
duration. It changes only when the phrase changes. This is the pedal tone that
holds the harmonic center while the melody evolves above it.

---

## Recording

`m` records exactly one full cycle (all phrases in sequence, each playing their
full repeat count) plus a sustain tail. The melody evolves during recording
exactly as it does live.

Output: `~/maqam-<timestamp>.mp4` — 1280×720, 30fps, teal waveform on dark
background, phrase list overlay showing which phrase is executing.

```
m       one cycle
m4      four cycles (good for letting evolution accumulate)
m8      eight cycles
```

Requires `ffmpeg` in PATH.

---

## Example session

```
d bay, a nah 332 r4     Start: Bayati+Nahawand, 8/8, loops 4×
c ajam 4                Add Ajam on C, inherits rhythm 332, loops 4×
bpm 100                 Slow down
s 2.0                   Longer sustain — more bell overlap
m4                      Record 4 cycles to ~/maqam-*.mp4
rot                     Bring Ajam to front, Bayati+Nahawand follows
d hij, a nah 2323 r2    New phrase: Hijaz+Nahawand, asymmetric rhythm
x1                      Delete the Ajam phrase
m8                      Record 8 cycles of evolved state
```

---

## Architecture

```
src/
  main.rs       — thread wiring (cpal audio thread ↔ crossbeam ← ratatui TUI)
  command.rs    — mini-language parser
  app.rs        — application state, command dispatch, history
  sequencer.rs  — Phrase/Bar structs, JI scale builder, coincidence-point snap
  synth.rs      — voices (additive JI synthesis), evolution, PRNG
  audio.rs      — cpal stream, real-time sequencer tick, phrase playback
  record.rs     — offline synthesis, WAV writer, ffmpeg MP4 + ASS subtitles
  tuning.rs     — JI ratios for each maqam, Pitch parsing (+/- accidentals)
  ui.rs         — ratatui TUI, dark theme, history navigation
```

**Threading:** cpal runs its callback on a dedicated audio thread. The TUI runs
on the main thread. They communicate via a `crossbeam` bounded channel carrying
`AudioCmd` values. No shared mutable state between threads.

**JI scale construction:** Each phrase builds a single combined frequency set.
The first jins root is taken as stated. Each subsequent jins root is snapped to
the nearest frequency already computed in JI (searching across octaves), then
that jins's intervals are computed from the snapped root. Deduplication uses a
4-cent threshold — tighter than the syntonic comma (21.5¢) — so genuine
comma-separated pitches are preserved as distinct scale degrees.

---

## Version history

**v1.0.0** — Initial release.
- JI maqam scales: Nahawand, Bayati, Hijaz, Rast, Kurd, Saba, Ajam
- Live REPL with history navigation
- Comma syntax for stacked ajnas with JI coincidence-point snapping
- Three-level rhythm/melody hierarchy
- Phrase-level evolution (drift, fill, reset)
- Sub-bass pedal tone
- MP4 export with phrase overlay
- Rotation, repeat, sustain, BPM control
