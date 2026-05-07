# maqam-live

A live-coding terminal sequencer for Arabic maqam music, built on just
intonation. Type a phrase, hear it immediately. Stack ajnas (tetrachords)
with commas to build scales from pieces. Insert, edit, delete, and rotate
phrases while the sequence loops. Render to MP4 when it sounds right.

Every pitch is an exact rational multiple of D — no equal temperament
anywhere in the signal chain. The beating between simultaneously ringing
voices is the sound of the instrument.

Music theory reference: **[maqamworld.com](https://maqamworld.com)** —
the most comprehensive English-language resource on Arabic maqam theory,
ajnas, and modulation practice.

```
┌ maqam-live ─────────────────────────────────────────────────────────────────┐
│ >  0: d bay, a nah 332    X..X..X.  Bayati+Nahawand       [2/4]            │
│    1: j 0 3                                                [1/3]            │
│    2: c rast 332           X..X..X.  Rast                                  │
├─ cmd ───────────────────────────────────────────────────────────────────────┤
│ > d bay, a nah 332_                                                         │
│  BPM:120 sus:1.2s vol:1.00 phrases:3  [?] help  [z] pause                  │
└─────────────────────────────────────────────────────────────────────────────┘
```

`>` marks the currently playing phrase. Jump entries show live pass counters.
IDs are permanent — they never shift when you insert, delete, or rotate.

---

## Install

### Linux (Ubuntu / Debian)

```bash
sudo apt install libasound2-dev pkg-config ffmpeg
tar -xzf maqam-live.tar.gz
cd maqam-live
cargo build --release
cargo run --release
```

### macOS

```bash
brew install ffmpeg
tar -xzf maqam-live.tar.gz
cd maqam-live
cargo build --release
cargo run --release
```

No extra audio dependencies — cpal uses CoreAudio, which is built in.

### Windows

1. Install [ffmpeg](https://ffmpeg.org/download.html) and add it to PATH
   (`winget install ffmpeg` works on recent Windows)
2. Install [Rust](https://rustup.rs/)
3. Open a terminal (Windows Terminal recommended):

```powershell
tar -xzf maqam-live.tar.gz
cd maqam-live
cargo build --release
cargo run --release
```

cpal uses WASAPI on Windows — no extra audio dependencies needed.
Recordings are saved to `%USERPROFILE%\maqam-<timestamp>.mp4`.

`rust-toolchain.toml` pins Rust 1.85.0. `rustup` downloads it automatically.

---

## Language

### Add a phrase

```
<root> <maqam> [<groups>] [, <root> <maqam>] ...  [r<N>]
```

**root** — pitch name: `c d e f g a b`, with `+` for sharp, `-` for flat.
`d+` = D♯, `b-` = B♭.

**maqam** — prefix-matched (type as few letters as needed):

| Name | Type | Character | Degrees (cents) |
|---|---|---|---|
| `nah` Nahawand | Pythagorean minor | Minor, resonant | 0 204 294 498 702 792 996 1200 |
| `bay` Bayati | 11-limit neutral | Arabic, expressive | 0 151 355 498 702 853 996 1200 |
| `hij` Hijaz | Augmented 2nd | Dramatic, Middle Eastern | 0 90 408 498 702 792 1110 1200 |
| `ras` Rast | Neutral 3rd | Balanced, central | 0 204 355 498 702 906 996 1200 |
| `kur` Kurd | Pythagorean Phrygian | Dark, Spanish | 0 90 294 498 702 792 996 1200 |
| `sab` Saba | Diminished 4th (11/8) | Melancholic, characteristic | 0 151 267 551 702 792 996 1200 |
| `aja` Ajam | 5-limit major | Bright, major | 0 204 386 498 702 884 1088 1200 |
| `nik` Nikriz | Hijaz lower + natural upper | Dramatic with bright upper | 0 90 408 498 702 906 996 1200 |
| `suz` Suznak | Rast lower + Hijaz upper | Neutral colour + augmented 2nd | 0 204 355 498 702 792 1110 1200 |
| `jih` Jiharkah | Ajam lower + flat 7th | Major with Mixolydian ending | 0 204 386 498 702 884 996 1200 |

See [maqamworld.com](https://maqamworld.com) for full maqam theory, modulation
paths, and performance practice.

**groups** — additive rhythm in eighth notes: `332` = 3+3+2 = one bar of 8,
`44` = 4+4, `3234` = 12/8. Default is `332`.

**r\<N\>** — repeat the phrase N times before advancing (`r4`, not `r 4`).

### Comma = stacked ajnas

Commas build one combined scale from tetrachords. Each jins contributes its
first 4 scale degrees; the last jins fills out to the octave. Notes within
70 cents of a jins boundary are dropped to prevent micro-interval clashes.

```
d nah, a kurd 332      D natural minor
d kurd, a kurd         D Phrygian
d bay, a nah           Bayati + Nahawand (classic)
d sab, g nah           Traditional Saba maqam
d hij, a nah           Hijaz lower + natural upper
d nah, a bay, c nik    Three-jins combination
```

### Commands

**Sequence control:**

| Command | Effect |
|---|---|
| `j <id> [<n>]` | Append jump: loop back to phrase `id` a total of `n` times, then fall through. Default n=1. |
| `i <id> <phrase>` | Insert a new phrase immediately before the phrase with id. |
| `i <id> j <target> [<n>]` | Insert a jump entry before the phrase with id. |
| `x <id> [id …]` | Delete phrase(s) by id. Cannot delete the currently playing phrase. |
| `edit <id> <phrase>` | Replace the content of phrase id in-place. Cannot edit the currently playing phrase. |
| `rot` | Move the last phrase to the front. Playback continues uninterrupted. |

**Settings:**

| Command | Effect |
|---|---|
| `bpm <n>` | Set tempo (20–400). Default 120. |
| `s <n>` | Set melody sustain in seconds. Default 1.25. |
| `vol <n>` | Set output volume 0–2. Default 1.0. |

**Other:**

| Command | Effect |
|---|---|
| `m [<n>]` | Record n full cycles to `~/maqam-<timestamp>.mp4`. Default 1. |
| `clear` | Remove all phrases. |
| `z` | Toggle pause. |
| `?` | Show help. |
| `q` | Quit. |

Semicolons separate multiple commands: `d bay r4; j 0 3; c rast`

**Up/Down arrows** scroll command history. **Left/Right/Home/End** move the
cursor. **Delete** removes the character at the cursor.

### IDs are permanent

Every phrase and jump entry is assigned a sequential ID at creation. IDs
never change when you insert, delete, or rotate. The number shown on each
line is that permanent ID — not its list position.

```
 0: d bay, a nah 332    ← always id 0
 1: j 0 3               ← always id 1, jumps to wherever id 0 currently is
 2: c rast 332          ← always id 2
```

If you delete id 1 and add a new phrase, the new phrase gets id 3. Id 1 is
gone forever. All `j`, `i`, `x`, and `edit` commands operate by id.

---

## Tuning

### The oud lattice

All pitches are exact rational multiples of D:

```
D = 293.6648 Hz    matches electronic tuner D
G = D × 4/3        open G string (perfect fourth)
A = D × 3/2        open A string (perfect fifth)
C = D × 16/9       open C string (two fourths above D)
```

Every note is a 5-limit or 11-limit ratio from D. No equal temperament
anywhere in the system. The table of valid pitches:

| Note | Ratio from D | Cents | Character |
|---|---|---|---|
| D | 1/1 | 0 | open string |
| E♭ | 256/243 | 90 | Pythagorean semitone |
| E¾ | 12/11 | 151 | 11-limit neutral 2nd |
| E | 9/8 | 204 | Pythagorean whole tone |
| F | 32/27 | 294 | Pythagorean minor 3rd |
| F# | 81/64 | 408 | Pythagorean major 3rd |
| G | 4/3 | 498 | open string, perfect fourth |
| A♭ | 1024/729 | 588 | Pythagorean diminished 5th |
| A | 3/2 | 702 | open string, perfect fifth |
| B♭ | 128/81 | 792 | Pythagorean minor 6th |
| B | 27/16 | 906 | Pythagorean major 6th |
| C | 16/9 | 996 | open string |

Neutral intervals (12/11 = 151¢, 27/22 = 355¢) are kept at exact 11-limit
values — never rounded. These are the characteristic colour of Bayati, Rast,
and Saba.

---

## How it sounds

### Rhythmic hierarchy

```
Subdivision  →  melody note (zigzag walk through scale)
Group start  →  floor tom + chord tones snapped to scale
Phrase start →  crash cymbal + long root voice + sub-bass pedal
Phrase change → high tom on top
Turnaround   → rimshot on top (last beat before loop)
```

Chord tones (5th and octave above melody) are snapped to the nearest scale
note. Nothing outside the maqam ever plays.

### Sub-bass

A chain of six pure octaves below the root plays for the full phrase
duration. Gains peak two octaves below the root and taper toward both
extremes. The lowest partials add physical weight even below the audible
range.

### Stereo

Each note voice is assigned a random fixed pan (±90%) at spawn. The pan does
not move as the voice decays. Floor tom and sub-bass stay centred.

### Evolution

After every phrase play, the melodic arc drifts slightly: interior notes
shift ±1 scale degree 70% of the time, with occasional fills (20%) and full
resets (8%). The penultimate group before each boundary rises toward the
dominant 50% of the time — the classical maqam cadential motion.

---

## Example session

```
d bay, a nah 332 r4     Bayati+Nahawand, 4 loops
c rast                  Add Rast on C
bpm 96                  Slow down
s 2.0                   Longer sustain
j 0 3                   Append jump: loop to id 0 three times
m 4                     Record 4 cycles to ~/maqam-*.mp4
edit 2 c jih r2         Edit id 2 to Jiharkah, 2 loops
i 2 d hij 2323 r2       Insert Hijaz before id 2
x 2                     Delete id 2 (if not playing)
vol 0.8                 Back off volume
```

---

## Recording

`m` records one full cycle (following all jumps and repeats). `m4` records
four cycles. Output is `~/maqam-<timestamp>.mp4` — 1280×720, teal waveform
on black, phrase list overlay showing which phrase is active with pass
counters. Repo URL pinned to bottom-left corner.

Requires `ffmpeg` in PATH.

---

## Architecture

```
src/
  main.rs       thread wiring, global atomics
  command.rs    mini-language parser
  app.rs        application state, command dispatch, history
  sequencer.rs  Phrase/JumpSpec/Bar structs, tetrachord stacking
  tuning.rs     oud lattice, 5-limit ratios, maqam interval tables
  synth.rs      additive synthesis, evolution, stereo, drums
  audio.rs      cpal stream, per-sample sequencer tick, jump logic
  record.rs     offline synthesis, normalization, ffmpeg MP4 + ASS subtitles
  ui.rs         ratatui TUI, cursor editing, history navigation
```

See `CODE.md` for a detailed explanation of every file, the reasoning behind
the design, and a complete walkthrough of how a keypress becomes sound.

---

## Music theory

This software implements the Arabic maqam system as described at
**[maqamworld.com](https://maqamworld.com)**, the authoritative English-language
reference for Arabic maqam theory.

Key concepts used:
- **Jins** (pl. ajnas): a 3–5 note melodic cell, the building block of maqamat
- **Tetrachord stacking**: combining ajnas to build full scales
- **Just intonation**: exact rational frequency ratios, not equal temperament
- **Neutral intervals**: 12/11 (151¢) and 27/22 (355¢), between the Western
  semitone and whole tone — characteristic of Bayati, Rast, and Sikah families
- **Maqam families**: each maqam belongs to a family sharing a lower jins
  (Bayati family, Rast family, Hijaz family, etc.)

For modulation paths, characteristic phrases (sayr), regional variations, and
performance practice, see maqamworld.com directly.

---

## Version history

**v1.0.0** — Initial release.

**v1.1.0** — Tuning rewrite.
- Oud lattice tuning (D-based JI, no equal temperament)
- Tetrachord stacking with 70-cent boundary guard
- Permanent phrase IDs (survive insert/delete/rotate)
- Jump entries with live countdown display
- Cursor editing, command history
- Stereo panning, sub-bass octave chain
- Four drum timbres by milestone
- `vol`, `z`, `rot`, `edit` commands
- Non-disruptive insert/rotate (playback continues uninterrupted)
- Delete/edit blocked on currently playing phrase
- Added maqamat: Nikriz, Suznak, Jiharkah
- MP4 recording with phrase overlay, repo URL, pass counters
- Cross-platform: Linux (ALSA), macOS (CoreAudio), Windows (WASAPI)

---

Source: https://github.com/rfielding/maqam
Music theory reference: https://maqamworld.com
