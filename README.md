# maqam-live

A live-coding terminal sequencer for Arabic maqam music, built on just intonation.

Type a phrase, hear it immediately. Add more phrases while the first loops. Stack
ajnas (tetrachords) with commas to build scales from pieces. The melody evolves
slowly on its own — staying in key, drifting toward and away from the dominant,
accenting the turnaround before each cycle reset. Every pitch is an exact JI ratio
from D. The beating between simultaneously ringing voices is the sound of the
instrument.

```
┌ maqam-live ─────────────────────────────────────────────────────────────────┐
│  0: d bay, a nah 332    X.[X]..X.  2/4  Bayati+Nahawand                    │
│  1: >>0 x3              [1/3]                                               │
│  2: c rast 332                     X..X..X.   Rast                         │
├─ cmd ───────────────────────────────────────────────────────────────────────┤
│ > d bay, a nah 332_                                                         │
│  BPM:120 sus:1.2s vol:1.00 phrases:3  [?] help  [z] pause                  │
└─────────────────────────────────────────────────────────────────────────────┘
```

The current subdivision is highlighted (inverted) in the rhythm display. The play
counter `2/4` shows which pass you are on. Jump entries show their countdown
`[1/3]`. All IDs are permanent — they never shift when you insert or delete.

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

No extra audio dependencies needed on macOS — cpal uses CoreAudio, which is
built in. `pkg-config` is not required.

The `rust-toolchain.toml` pins to Rust 1.85.0. `rustup` will download it
automatically on first build.

---

## Language

### Add a phrase

```
<root> <maqam> [<groups>] [, <root> <maqam>] ...  [r<N>]
```

- **root** — pitch name: `c d e f g a b`, with `+` for sharp, `-` for flat.
  `d+` = D♯, `b-` = B♭.
- **maqam** — prefix-matched: `nah` → Nahawand, `bay` → Bayati, `hij` → Hijaz,
  `ras` → Rast, `kur` → Kurd, `sab` → Saba, `aja` → Ajam.
- **groups** — additive rhythm (8th notes): `332` = 3+3+2 = 8/8,
  `44` = 4+4 = 8/8, `3234` = 3+2+3+4 = 12/8. Default is `332`.
- **r\<N\>** — repeat the phrase N times before advancing (`r4` not `r 4`).

**Comma = stacked ajnas.** Commas build one combined scale from tetrachords:
each jins contributes its first 4 degrees (root + 3 characteristic intervals).
The last jins contributes all degrees up to the octave. Notes within 70 cents
of the next jins's root are dropped to prevent micro-interval clashes at
boundaries.

```
d nah, a kurd 332      D natural minor — Nahawand lower + Kurd upper
d kurd, a kurd         D Phrygian — two Kurd tetrachords
d bay, a nah           Bayati lower + Nahawand upper (classic Arabic)
d sab, g nah           Traditional Saba maqam
c rast                 Rast on C (the open C oud string)
d nah, a nah, g nah    Three-jins Nahawand span
```

**Semicolons = separate commands.** A semicolon-separated line executes each
part in order:

```
d bay,a nah 332 r4; c ajam r2; d hij 2323 r2
```

### Sequence commands

| Command | Effect |
|---|---|
| `bpm <n>` | Set tempo (20–400). Default 120. |
| `s <n>` | Set melody sustain in seconds. Default 1.25. |
| `vol <n>` | Set output volume 0–2. Default 1.0. |
| `j <id> [<times>]` | Append a jump entry: when reached, loop back to phrase `id` N times, then fall through. |
| `i <id> <phrase>` | Insert a new phrase immediately before the phrase with the given id. |
| `x <id> [id …]` | Delete phrase(s) by id. |
| `rot` | Move last phrase to front. |
| `m [<N>]` | Record N full cycles to `~/maqam-<timestamp>.mp4`. |
| `clear` | Remove all phrases. |
| `z` | Toggle pause. |
| `?` | Show help. |
| `q` | Quit. |

**Up/Down arrows** scroll through command history. **Left/Right/Home/End**
move the cursor for in-place editing. **Delete** removes the character at the
cursor.

### IDs are permanent

Every phrase and jump entry is assigned a sequential ID at creation. IDs never
change when you insert or delete other entries. The number displayed on each
line is that permanent ID — not its current list position. `x`, `j`, and `i`
all operate by ID.

```
 0: d bay, a nah 332    ← always id 0
 1: >>0 x3              ← always id 1, jumps to wherever id 0 currently is
 2: c rast 332          ← always id 2
```

If you delete id 1 and add a new phrase, the new phrase gets id 3. Id 1 is
gone forever. `j 0 4` still correctly targets the `d bay` phrase even after
rearrangement.

---

## Tuning

### The oud lattice

All pitches are exact rational multiples of D:

```
A = D × 3/4   = 220.249 Hz  (open A string — a perfect fourth below D)
D = 293.6648  Hz             (matches electronic tuner D)
G = D × 4/3   = 391.553 Hz  (open G string)
C = D × 16/9  = 522.071 Hz  (open C string — two fourths above D)
```

Every other note is a 5-limit ratio from D, with Pythagorean (3-smooth) ratios
preferred because they resonate with the open strings:

| Note | From D | Cents |
|---|---|---|
| D♭/E♭ | 256/243 | 90¢ Pythagorean semitone |
| E | 9/8 | 204¢ |
| F | 32/27 | 294¢ = C × 4/3 (Pythagorean) |
| G | 4/3 | 498¢ open string |
| A♭ | 1024/729 | 588¢ |
| A | 3/2 | 702¢ open string |
| B♭ | 128/81 | 792¢ Pythagorean |
| B | 27/16 | 906¢ |
| C | 16/9 | 996¢ open string |

Neutral intervals (12/11 = 151¢, 27/22 = 355¢) are kept at their 11-limit
values — they are not rounded. These are the characteristic colour of Bayati,
Rast, and Saba.

### Maqam scales

| Maqam | Degrees (cents) | Character |
|---|---|---|
| Nahawand | 0, 204, 294, 498, 702, 792, 996, 1200 | Pythagorean natural minor |
| Bayati | 0, 151, 355, 498, 702, 853, 996, 1200 | Neutral 2nd (12/11) |
| Hijaz | 0, 90, 408, 498, 702, 792, 1110, 1200 | Augmented 2nd |
| Rast | 0, 204, 355, 498, 702, 906, 996, 1200 | Neutral 3rd (27/22) |
| Kurd | 0, 90, 294, 498, 702, 792, 996, 1200 | Pythagorean Phrygian |
| Saba | 0, 151, 267, 551, 702, 792, 996, 1200 | Diminished 4th (11/8) |
| Ajam | 0, 204, 386, 498, 702, 884, 1088, 1200 | 5-limit major |

---

## How it sounds

### Three-level rhythm/melody hierarchy

```
Subdivision  →  melody note (zigzag walk through combined scale)
Group start  →  floor tom + chord tones snapped to scale
Phrase start →  crash cymbal + long root voice + sub-bass pedal
Phrase change → high tom added on top of floor tom
Turnaround   → rimshot added on top of floor tom (last beat before repeat)
```

Chord tones (5th and octave above the melody note) are snapped to the nearest
note in the current scale. Nothing outside the maqam ever plays.

### Sub-bass

A chain of pure sine octaves (six, from root down to ~8 Hz) plays for the full
phrase duration. The lowest partials add physical pressure even below the
audible range. Gains peak two octaves below the root and taper toward both
extremes.

### Stereo

Each melody and snare voice is assigned a random fixed pan position (±90%) at
spawn. The pan does not move as the voice decays — each note lands in one
position in the stereo field and rings there. Floor tom and sub-bass stay
centered. The two ears hear different mixtures of simultaneously decaying
voices.

### Evolution

After every complete phrase play, the melody degrees drift slightly:
- **70%** — gentle drift: interior notes shift ±1 scale degree
- **20%** — fill: excursion toward a higher degree and back
- **8%** — reset: fresh arc with a new random peak

The penultimate group before each phrase boundary rises to the dominant 50% of
the time — the classical maqam cadential motion.

### Phrase transitions

All voices fade out over 10ms when the phrase advances. This prevents chromatic
clashes between the sustain tails of different scales. Like a qanun player
lifting fingers between phrases.

---

## Example session

```
d bay, a nah 332 r4     Bayati+Nahawand, loops 4×
c rast                  Add Rast on C, inherits rhythm 332
bpm 96                  Slow down
s 2.0                   Longer sustain
j 0 3                   Append: loop back to id 0 three times, then continue
m4                      Record 4 full cycles to ~/maqam-*.mp4
x 2                     Delete id 2 (the Rast phrase)
i 2 d hij 2323 r2       Insert Hijaz before id 2 (wherever it now lives)
vol 0.8                 Back off volume
```

---

## Jump and insert mechanics

`j <id> <times>` appends an entry to the sequence that, when reached, jumps
back to the phrase whose permanent id is `<id>` and plays it `<times>` times
total before falling through. The counter is shown in the TUI and resets each
time the outer loop reaches the jump entry again.

`i <id> <phrase>` inserts a new phrase immediately before the phrase with id
`<id>`. If that phrase has moved due to earlier inserts or deletes, the insert
still lands in the right place.

```
d nah          ← id 0
j 0 3          ← id 1: play id 0 three times, then fall through
c rast         ← id 2
j 0 4          ← id 3: play entire sequence from id 0 four times
```

---

## Recording

`m` records one full cycle offline (all phrases, all repeats, all jumps) plus a
decay tail. The final "1" is struck at the end — the tonic, ringing out to
close the arc.

```
m       one cycle
m4      four cycles
m8      eight cycles — good for letting evolution accumulate
```

Output: `~/maqam-<timestamp>.mp4` — 1280×720, 30fps, teal stereo waveform on
dark background, phrase list overlay showing which phrase is playing.

Requires `ffmpeg` in PATH.

---

## Architecture

```
src/
  main.rs       — thread wiring (cpal audio ↔ crossbeam ← ratatui TUI)
  command.rs    — mini-language parser
  app.rs        — application state, command dispatch, history
  sequencer.rs  — Phrase/JumpSpec/Bar structs, tetrachord stacking, JI lattice
  synth.rs      — voices (additive synthesis), evolution, stereo pan, drums
  audio.rs      — cpal stream, per-sample sequencer tick, jump counter logic
  record.rs     — offline synthesis, WAV writer, ffmpeg MP4 + subtitles
  tuning.rs     — oud lattice, pitch table, maqam interval tables
  ui.rs         — ratatui TUI, dark theme, cursor editing, history navigation
```

**Threading:** cpal callback runs on a dedicated audio thread. TUI on main.
Communication via `crossbeam` bounded channel (`AudioCmd` values). Shared
playback state (current phrase, subdivision, play count, jump counter) via
atomic integers — no locking.

**Tuning:** All pitches are exact D-based ratios. `snap_to_oud_lattice` reduces
any Hz value to the D4 register `[D, 2D)` and finds the nearest table entry.
Pythagorean ratios (3-smooth) are preferred; 5-smooth is the fallback; 11-limit
(12/11, 27/22) is kept exact for neutral intervals.

**Tetrachord stacking:** each jins contributes exactly its first 4 scale
degrees. Notes within 70 cents of the next jins's root are dropped to prevent
micro-interval clashes. The last jins contributes all degrees up to the octave.
Dedup threshold is 50 cents for multi-jins, 23 cents for single.

---

## Version history

**v1.0.0** — Initial release.

**v1.1.0** — Tuning rewrite.
- All pitches from exact D-based oud lattice (no equal temperament)
- Pythagorean preference for notes reachable by 4/3 chain from D
- Tetrachord stacking: each jins contributes 4 degrees, boundary guard
- Permanent phrase IDs — survive insert/delete, used by j/i/x
- Jump entries appear as sequence items with live countdown display
- Cursor editing in command line (←→ Home End Delete)
- Stereo panning: each voice random fixed position ±90%
- Sub-bass octave chain (6 octaves, tapered gains)
- Four drum timbres stacked by milestone (floor tom, rimshot, crash, high tom)
- `vol` command for live output scaling
- `z` to pause/resume
