# Code Guide — maqam-live

This document explains what every file does, why it exists, and how the
pieces fit together. It is written for someone who does not know Rust.

---

## The big picture

maqam-live is a program that does three things simultaneously:

1. **Listens** to what you type in the terminal
2. **Plays audio** in real time
3. **Draws** the TUI (text interface) on screen

These three activities run in separate "threads" — think of them as three
workers running at the same time, passing notes to each other.

```
┌──────────────┐  commands  ┌──────────────┐
│  TUI thread  │ ─────────► │ Audio thread │
│  (main.rs /  │            │  (audio.rs)  │
│   ui.rs /    │            │              │
│   app.rs)    │ ◄───────── │              │
└──────────────┘  atomics   └──────────────┘
                 (CUR_PHRASE etc.)
```

The TUI thread sends commands ("add this phrase", "change BPM") through a
**channel** — a one-way pipe. The audio thread publishes its current position
back via **atomics** — shared numbers that can be read and written safely
without locking.

---

## File by file

### `main.rs` — The entry point (44 lines)

This is where the program starts. It does three things and nothing else:

1. Creates the **channel** that carries commands from TUI to audio
2. Starts the audio thread (`audio::start_audio`)
3. Starts the TUI (`ui::run`)

It also declares four **global atomics** — numbers visible to every part of
the program without passing them around explicitly:

```
CUR_PHRASE  — which phrase is currently playing (list index)
CUR_SUBDIV  — which subdivision within that phrase
CUR_PLAYS   — how many times the current phrase has looped
CUR_JUMP_REM — remaining jump count (legacy; superseded by JUMP_COUNTERS)
```

And `JUMP_COUNTERS` — a global map from phrase id to remaining jump count,
written by the audio thread and read by the TUI and video renderer so they
can display `[2/4]` counters.

**Why globals?** The audio thread runs inside a callback that the audio
hardware calls thousands of times per second. You cannot pass arguments into
that callback, and you cannot afford to lock a mutex in it (locking can
stall). Atomics are the only safe, zero-cost option.

---

### `tuning.rs` — Just intonation pitch system (255 lines)

This is the mathematical foundation of the whole instrument.

#### The problem it solves

Equal temperament (ET) — the tuning used by pianos and guitars — divides the
octave into 12 equal semitones. Each interval is slightly off from a pure
ratio. For example, a fifth in 12-ET is 2^(7/12) = 1.4983…, not exactly 3/2
= 1.5. The difference is small but audible, especially when multiple notes
ring together. Arabic maqam music uses **just intonation** (JI) — every
interval is an exact ratio, so beating between strings is eliminated.

#### The oud lattice

All pitches are expressed as rational multiples of D:

```
D = 293.6648 Hz  (matches an electronic tuner's D note)
G = D × 4/3      (perfect fourth — open G string)
A = D × 3/2      (perfect fifth — open A string)
C = D × 16/9     (= (4/3)² — two fourths up — open C string)
```

Every note is a ratio from D — no equal temperament anywhere in the system.

The table `PITCH_TABLE` lists 18 named ratios. `snap_to_oud_lattice()` takes
any frequency and finds the closest table entry. This is the **only** entry
point — any Hz value that enters the system gets snapped to the lattice
first.

#### Maqam scales

Seven maqamat are defined, each as eight (numerator, denominator) pairs for
scale degrees 0–7. These are the intervals from the root, not cumulative:

```rust
Maqam::Bayati => [(1,1),(12,11),(27,22),(4,3),(3,2),(18,11),(16,9),(2,1)]
```

So Bayati on D gives: D, D×12/11, D×27/22, D×4/3 (=G), D×3/2 (=A), …

The neutral second (12/11 ≈ 151 cents) is the characteristic colour of
Bayati and Rast. The semitone in Kurd (256/243 ≈ 90 cents) is narrower than
ET's 100-cent semitone. These are not approximations — they are the exact
ratios.

#### Pitch parsing

`Pitch::parse("d+")` returns a Pitch struct with letter='d', accidental=+1.
`pitch.to_hz()` converts it to an exact Hz via the pitch table.

---

### `sequencer.rs` — Data structures (292 lines)

This file defines the core data types and the phrase-building logic. It
contains no audio output and no UI — just data.

#### `SubdivEvent`

Every subdivision (eighth note) produces one of two events:
- `Kick(hz)` — a strong beat (group start), with floor tom + melody note
- `Snare(hz)` — a weak beat (off-beat), with snare + melody note

The `hz` payload is the melody note's frequency for that subdivision.

#### `Bar`

A `Bar` is one complete rhythmic cycle — everything needed to play one loop
of a phrase. It holds:

- `frequencies` — the combined JI scale, sorted ascending (all the notes
  available)
- `groups` — the rhythm pattern as additive eighth notes (e.g. [3,3,2] for
  a 3+3+2 pattern)
- `group_degrees` — n_groups+1 waypoints, each an index into `frequencies`
  (these control the melodic arc)
- `degrees` — one index per subdivision, interpolated between waypoints
- `events` — one `SubdivEvent` per subdivision, with its Hz value

#### `Phrase`

A `Phrase` wraps a `Bar` with metadata:

- `id` — a permanent integer assigned at creation, never reused, never
  changed by insert/delete
- `src` — the original command string typed by the user (e.g. "d bay 332")
- `repeat` — how many times to play before advancing
- `jump` — `None` for musical phrases, `Some(JumpSpec)` for jump entries

**Why stable IDs?** If phrases were identified by list position, deleting
phrase 2 would renumber everything after it, breaking all `j` and `i`
commands. Stable IDs mean `j 5 4` will always target whatever phrase was
born as id 5, even after rotations, inserts, and deletes.

#### `JumpSpec`

A jump entry stores:
- `target_id` — the stable id of the phrase to jump back to
- `times` — how many total passes before falling through

The audio thread resolves `target_id` to a list position at runtime:
`phrases.iter().position(|p| p.phrase.id == js.target_id)`. This means
the target can move in the list and the jump still works.

#### Tetrachord stacking (`build_phrase`)

When you type `d bay, a nah`, the comma means "stack these two ajnas into
one scale." The algorithm:

1. Compute the root Hz for each jins by snapping to the oud lattice
2. For each non-last jins: take only its first 4 scale degrees
   (the traditional tetrachord)
3. For the last jins: take all degrees up to the octave
4. Drop any note within 70 cents of the next jins's root (prevents
   micro-interval clashes at boundaries)
5. Deduplicate within 50 cents (earlier jins wins)

Result: `d bay, a nah` gives D, E¾, F¾, G, A, B, C, D' — natural minor
with Bayati colour in the lower tetrachord.

#### `AudioCmd`

The enum of messages the TUI sends to the audio thread:
`AddPhrase`, `RemovePhrase`, `SetBpm`, `SetSustain`, `Clear`, `SetVol`,
`SetPaused`, `SetCurPhrase`.

---

### `command.rs` — Command parser (233 lines)

Parses a line of text into a `Cmd` value. No audio, no UI — pure parsing.

The grammar is:

```
<root> <maqam> [groups] [, <root> <maqam>]…  [r<N>]
j <id> [<times>]
i <id> <phrase-or-jump>
x <id> [<id>…]
bpm <n> / s <n> / vol <n> / rot / z / clear / m [N] / ?
```

Semicolons separate multiple commands on one line.

**Prefix matching for maqam names:** You only have to type enough letters to
be unambiguous — `bay`, `nah`, `kur`, `sab`, `ras`, `hij`, `aja` all work.
The code iterates the candidates list and returns the first name that starts
with what you typed.

**Strip-repeat:** `r4` at the end of a phrase spec is detected and stripped
before the rest is parsed. The result is a `(phrase_text, repeat_count)`
pair.

---

### `app.rs` — Application state and command dispatch (366 lines)

`App` is the main state machine. It holds:

- `phrases` — the ordered list of `Phrase` objects
- `next_phrase_id` — a counter that only ever goes up
- `audio_tx` — the channel sender (to send commands to the audio thread)
- `history` — the command history buffer (up/down arrows)
- `cursor` — position within the command line being edited
- `message` — optional status message to show in the TUI

When you press Enter, `App::dispatch()` is called. It:

1. Parses the command
2. Modifies `self.phrases` (the authoritative phrase list)
3. Sends the appropriate `AudioCmd` messages to the audio thread

**Why resend everything on insert/delete?** When a phrase is inserted or
deleted, the audio thread needs to update its internal list. The simplest
correct approach is to `Clear` the audio thread's list and re-add everything
in order. This takes a few microseconds and is inaudible.

**`rot`** (rotate) moves the last phrase to the front. Internally: pop the
last element, insert it at position 0, then resend all phrases. The audio
thread picks up at the same phrase id it was playing (by searching for the
id in the new list order).

---

### `audio.rs` — Real-time audio callback (281 lines)

This is the most performance-critical file. The hardware calls the audio
callback thousands of times per second; each call must return in microseconds
or you get a glitch.

#### Structure

The callback runs a tight inner loop:

```
For each sample:
  1. Process any pending AudioCmd messages (non-blocking check)
  2. Tick the sequencer (advance position, fire events)
  3. Mix all active voices
  4. Write the sample to the output buffer
```

#### `tick_sequencer`

The sequencer operates at the **subdivision level** — it does not think in
"bars" or "beats" in the traditional sense, but in subdivisions (eighth
notes by default).

For each sample:
1. Compute which subdivision we are in: `bar_pos / subdiv_samples`
2. If the subdivision changed: fire the corresponding `SubdivEvent`
3. If the bar is complete: advance to the next play (or next phrase)

When advancing to the next phrase, the sequencer checks if the next entry is
a **jump entry**. If so, it executes the jump logic:
- Look up the `target_id` in the current phrase list to find the position
- Check the jump counter (stored in `jump_counters` HashMap)
- If counter > 0: decrement and jump back to target
- If counter = 0: remove the counter, fall through to next phrase

**Milestone events** are set during tick_sequencer and passed to
`spawn_voices`:
- `PhraseStart` — first subdivision of first play
- `PhraseChange` — first subdivision of any subsequent play
- `Turnaround` — last subdivision of last play

These trigger different drum sounds.

#### Thread safety

The audio callback can never lock a mutex (mutex operations can block
indefinitely if another thread holds the lock). All communication uses:
- `AtomicUsize` for simple counters (CUR_PHRASE etc.)
- `try_lock()` (not `lock()`) for the jump counters map — if the lock is
  busy, the update is silently skipped; the TUI will catch up on the next
  frame

---

### `synth.rs` — Voice synthesis (416 lines)

Every sound that comes out of the speaker is a `Voice`. This file defines
what voices sound like and how they evolve.

#### `Voice`

A voice is one sustained sound. It has:
- `kind` — what type of voice (melody, sub-bass, floor tom, snare, etc.)
- `hz` — its pitch
- `pan` — stereo position (-1 = full left, +1 = full right)
- `phase` — current oscillator position (0.0 to 1.0)
- `t` — elapsed time in samples
- `sustain_secs` — how long before it fades out
- `release_frames` — if set, force a fast fade regardless of sustain
- `done` — once true, the voice is removed from the list

`Voice::sample(sr)` computes one sample of audio. No buffering, no
lookahead — pure sample-by-sample synthesis.

#### Voice kinds

**Melody:** A sine wave with a mild frequency-modulation (FM) shimmer. The
modulator frequency and depth are randomized per-voice to give each note a
slightly different timbre. Gain envelope: instant attack, exponential decay.

**SubBass:** Pure sine, very low frequency, long sustain. Six octaves below
the root are spawned simultaneously at different gains (see `spawn_sub_bass`
in the file). The lowest may be below hearing range but is felt physically.

**FloorTom, Rimshot, Crash, PhraseChange:** Short percussive bursts using
filtered noise + sine blend, modeled loosely after drum physics.

**Snare:** Short noise burst with fast decay.

#### Stereo panning

Each voice gets a random pan position at spawn time, fixed for that voice's
lifetime. The pan is applied using constant-power panning:
```
left  += sample * cos((pan + 1) * π/4)
right += sample * sin((pan + 1) * π/4)
```

Kick and sub-bass are always centred (pan = 0).

#### Chord tones

When a melody note fires, two additional voices are spawned: one at 1.5×
frequency (a perfect fifth above) and one at 2.0× (an octave). Both are
snapped to the nearest note in the current scale (`snap_to_scale`), so
nothing outside the maqam ever sounds. The chord tones are quieter than the
melody (gains 0.14 and 0.08 vs 0.22).

#### `evolve_bar`

Called after every complete phrase play. Randomly adjusts the melodic arc
(group_degrees waypoints) within the current scale:
- **70% of the time:** Gentle drift — nudge one peak ±1 scale degree
- **20%:** Fill — push peaks slightly higher (more energy)
- **8%:** Reset — new random zigzag arc
- Always: force first and last waypoints to 0 (tonic)

Then forces the penultimate group to aim at the dominant (≈ scale degree 5)
50% of the time — the classical maqam cadential motion.

#### PRNG

Uses a simple linear congruential generator stored in a global atomic. This
means evolution is deterministic given a starting state, and thread-safe
without locking.

---

### `ui.rs` — Terminal interface (264 lines)

Draws the TUI using the **ratatui** library. Called 25 times per second from
a loop in `run()`.

#### Layout

```
┌ maqam-live ─────────────────────────────────────────────────────────┐
│ phrase list (fills available space)                                 │
├─ cmd ───────────────────────────────────────────────────────────────┤
│ command input + history                                             │
├─────────────────────────────────────────────────────────────────────┤
│ BPM / sustain / vol / phrase count                                  │
│ status / recording message                                          │
└─────────────────────────────────────────────────────────────────────┘
```

#### Colors

All colors are pure green on pure black (`rgb(0,255,0)` on `rgb(0,0,0)`).
Every `Span` (a colored text segment) carries an explicit background color
to prevent the terminal's own background from bleeding through.

Active phrase (currently playing) uses full-brightness green. Inactive
phrases use dimmer green. Borders and decorations also use green so the
whole interface has the same character.

#### Key handling

`crossterm` delivers key events. The main handling is:
- **Enter:** `app.dispatch(input)` — process the command
- **Up/Down:** navigate history
- **Left/Right/Home/End:** move cursor within command line
- **Delete/Backspace:** edit at cursor
- **Ctrl-C / q:** quit

#### Reading shared state

The TUI reads `CUR_PHRASE` and `CUR_SUBDIV` atomics every frame to know
which phrase is playing and which subdivision to highlight. It reads
`JUMP_COUNTERS` to show live `[2/4]` counters on jump entries.

---

### `record.rs` — Offline rendering to MP4 (360 lines)

When you type `m` or `m4`, this file runs the entire sequence offline (not
in real time) to produce a video file. The result should be indistinguishable
from what you hear live.

#### Why offline?

Real-time audio happens in tiny chunks determined by the hardware. Offline
rendering happens all at once, as fast as the CPU allows, into a buffer.
This makes it possible to produce a file without gaps or glitches.

#### Matching live behavior

The key design constraint: the render must produce exactly the same audio as
the live audio thread. This means:

- Same `Voice` struct and `Voice::sample()` function
- Same `spawn_voices`, `spawn_phrase_start`, `spawn_sub_bass`
- Same `evolve_bar` called after each play
- Same jump counter logic (via `expand_one_cycle`)
- **No** voice cutting at phrase boundaries (the live thread never cuts
  voices either — they decay naturally)

#### `expand_one_cycle`

Simulates the jump logic to produce an ordered list of phrase indices —
exactly the sequence that will play in one complete cycle. This is the same
logic as `tick_sequencer` in audio.rs, but run forward in time all at once
rather than sample by sample.

Given the phrase list and `j` entries, this returns e.g. `[0, 0, 0, 2]` for
"phrase 0 three times, then phrase 2 once" — following all jump counters.

#### Normalization

The raw synth output has peaks around 0.1–0.4 full scale. The render finds
the peak amplitude and scales everything to 90% of full scale. This is what
makes the waveform visible in the video — without normalization, it would be
a nearly flat line.

#### WAV format

16-bit signed PCM stereo, 44100 Hz. This is the format that `ffmpeg`'s
`showwaves` filter handles most reliably across versions and platforms.

#### Video generation

ffmpeg is called directly (no shell script, no intermediate steps):

```
ffmpeg -i maqam-live.wav
  -filter_complex "showwaves + subtitles"
  -c:v libx264 -crf 18
  -c:a aac -b:a 256k
  output.mp4
```

The subtitle overlay uses **ASS format** — a plain text file describing timed
text events with styles. One `Dialogue` event per phrase line per bar play,
each with its own `MarginV` offset to stack them vertically. All text uses a
single `Line` style (green, Courier New 20pt, 2px black outline) so every
line has identical font metrics and columns stay aligned.

The repo URL `https://github.com/rfielding/maqam` appears in the bottom-left
corner (ASS alignment=1, MarginV=100) throughout the entire video.

#### Cross-platform paths

- Temp files: `std::env::temp_dir()` → `/tmp/` on Linux/macOS,
  `%TEMP%` on Windows. Backslashes are converted to forward slashes because
  ffmpeg requires forward slashes even on Windows.
- Output directory: `$HOME` on Linux/macOS, `$USERPROFILE` on Windows.

---

## Data flow: from keypress to sound

Here is what happens when you type `d nah 332` and press Enter:

```
1. ui.rs: captures Enter keypress
2. app.rs dispatch():
   - command.rs parses "d nah 332" into Cmd::Phrase{ specs=[{root=D,maqam=Nahawand,groups=[3,3,2]}], repeat=1 }
   - tuning.rs: snap D to oud lattice → 293.6648 Hz
   - tuning.rs: Nahawand.ratios() → 8 JI intervals
   - sequencer.rs build_phrase(): computes combined scale, zigzag walk, event list
   - app.phrases.push(new Phrase { id=N, src="d nah 332", bar=..., repeat=1 })
   - audio_tx.send(AudioCmd::AddPhrase(phrase.clone()))

3. audio.rs (next time the audio callback checks for commands):
   - receives AddPhrase
   - phrases.push(PlayingPhrase::new(phrase, sr, bpm))

4. audio.rs (sample by sample, ~44100× per second):
   - tick_sequencer() advances bar_pos
   - when subdivision changes: lookup events[subdiv] → SubdivEvent::Kick(293.66)
   - synth.rs spawn_voices(): spawns FloorTom + melody voice at 293.66 Hz
   - Voice::sample() computes sine + FM for each active voice
   - all voice samples mixed together → written to audio output buffer

5. ui.rs (25× per second):
   - reads CUR_PHRASE atomic → knows which phrase is playing
   - reads CUR_SUBDIV atomic → highlights the current beat
   - redraws the phrase list with the active phrase in bright green
```

---

## Threading model

```
Main thread (TUI):
  - Runs the terminal event loop
  - Reads key events, updates App state
  - Sends AudioCmd to audio thread via channel
  - Reads CUR_PHRASE/CUR_SUBDIV atomics to update display
  - Reads JUMP_COUNTERS mutex (non-blocking) for jump displays

Audio thread (cpal callback):
  - Runs at ~44100 Hz (called by audio hardware)
  - CANNOT lock mutexes (would cause audio glitches)
  - Reads channel (try_recv, non-blocking) for new commands
  - Writes CUR_PHRASE/CUR_SUBDIV atomics after advancing
  - Writes JUMP_COUNTERS with try_lock (skips if busy)
```

**Why the channel is one-way:** Commands go TUI → Audio. The audio thread
never needs to request information from the TUI — it has everything it needs
in its local state. The atomics provide the reverse flow: status information
Audio → TUI, at zero cost and zero blocking.

---

## Dependencies

| Crate | Purpose |
|---|---|
| `cpal` | Cross-platform audio output (ALSA/CoreAudio/WASAPI) |
| `ratatui` | Terminal UI widgets and layout |
| `crossterm` | Terminal key events, raw mode, cursor control |
| `crossbeam-channel` | Fast bounded MPSC channel for AudioCmd |
| `anyhow` | Error handling with descriptive messages |

No unsafe code. No C FFI. All platform differences (Linux ALSA vs macOS
CoreAudio vs Windows WASAPI) are handled by cpal.

---

## Building

Requires Rust 1.85.0 (pinned in `rust-toolchain.toml`). `rustup` downloads
it automatically.

```bash
# Linux
sudo apt install libasound2-dev pkg-config ffmpeg
cargo build --release

# macOS
brew install ffmpeg
cargo build --release

# Windows
winget install ffmpeg    # or download from ffmpeg.org, add to PATH
cargo build --release
```

