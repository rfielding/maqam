# Code Guide - maqam-live

This guide describes the current code structure and runtime data flow. It is
intended as a map for changing the code, not as a Rust tutorial.

## Runtime Model

maqam-live has three main runtime paths:

1. The TUI/main thread reads keys, edits app state, and sends audio commands.
2. The CPAL audio callback renders real-time stereo audio.
3. The recording worker renders the same sequence offline and invokes ffmpeg.

The TUI talks to audio through a bounded `crossbeam-channel` of `AudioCmd`
values. Audio exposes live playback position through atomics:

```text
CUR_PHRASE
CUR_SUBDIV
CUR_PLAYS
CUR_JUMP_REM
REC_SAMPLES_DONE
REC_SAMPLES_TOTAL
REC_ACTIVE
```

Jump counters are shared through a `OnceLock<Mutex<HashMap<usize, usize>>>`.
The audio callback only uses `try_lock()` when publishing jump-counter state.

## Timeline Model

The app keeps one authoritative `Vec<Phrase>`. Despite the name, `Phrase` is a
timeline entry. It can be:

- a musical phrase, with a `Bar`
- a jump entry, with `JumpSpec`
- a settings/control entry, with `ControlSpec`

Every entry has a stable `id`. Commands such as `j`, `i`, `edit`, `x`, `up`,
and `down` resolve IDs through `App::resolve_id_ref`. IDs do not change when
the timeline is reordered.

`ControlSpec` currently supports:

```rust
SetBpm(f64)
SetSustain(f64)
SetVcf(VcfChange)
SetFx(FxChange)
```

VCF and FX control entries store the original relative command as a parsed
change, not as a frozen absolute setting. That is what makes commands like
`vcf bass cut=+2t` meaningful when replayed in a loop.

## File Guide

### `src/main.rs`

Declares modules, global atomics, and the jump-counter map. It has two entry
modes:

- no CLI args: start audio and run the TUI
- CLI args present: run each command group through an `App`, wait for any
  recording job, then print the result

`cli_commands` splits argument groups on literal `--`.

### `src/app.rs`

Owns interactive state:

- timeline entries
- command input, cursor, history, help/jins overlays
- current BPM, sustain, volume, VCF bank, and FX settings
- session path
- recording result receiver
- MIDI clockout sender

`App::handle_command` splits semicolon-separated commands. It handles
`clockin` and `clockout` directly, then sends the rest through
`command::parse`.

`App::execute` mutates the authoritative timeline and sends the matching
`AudioCmd` updates. Reordering uses `resync_audio_sequence`, which clears the
audio thread and rebuilds its phrase list from app state while preserving a
reasonable playback focus.

Session loading is also here. V3, V2, and V1 loaders all rebuild app state and
then resync audio.

Path completion is implemented in this file. `save` and `load` complete `.mq`
paths; `load` recursively finds `.mq` files when the partial path has no slash
and lists ambiguous matches in `App::message`.

### `src/command.rs`

Pure parser and value-change logic. It parses:

- musical phrases
- jumps, inserts, edits, deletes, moves, rotate
- `bpm`, `s`/`sus`, `vol`
- VCF/VCO commands
- reverb, delay, pingpong, and `fx off`
- session commands
- jins registry commands
- recording and playback commands

`ValueChange` represents absolute, additive, multiplicative, divisive, and
per-tick movement:

```rust
Set(f64)
Add(f64)
Mul(f64)
Div(f64)
Tick(f64)
```

`apply_vcf_change` applies a `VcfChange` against a `VcfBank` and returns a
single `VcfSettings` for the target slot. `apply_fx_change` applies `FxChange`
against `FxSettings`.

### `src/sequencer.rs`

Defines the core timeline and bar data structures:

- `SubdivEvent`
- `Bar`
- `JumpSpec`
- `ControlSpec`
- `Phrase`
- `AudioCmd`

`build_phrase` turns one or more parsed jins specs into a playable `Bar`.
Comma-separated ajnas are stacked into one combined scale. For stacked ajnas,
each non-final jins contributes its lower fragment while the final jins fills
out the remaining scale up to the octave. Nearby duplicate or boundary notes
are filtered by cents thresholds.

The resulting bar stores rhythm groups, available frequencies, melodic
waypoints, expanded per-subdivision degrees, and per-subdivision events.

### `src/audio.rs`

The real-time audio engine. `start_audio` opens the default CPAL output device
and builds a callback.

Each callback drains pending `AudioCmd` messages with `try_recv`, then processes
each output frame:

1. advance the sequencer
2. apply any pending control entry
3. spawn phrase-start, sub-bass, melody, and percussion voices
4. advance VCF/FX tick automation on sequencer ticks
5. mix active voices into dry/per-target buses
6. process enabled VCF slots
7. process FX
8. clamp, apply volume, and write output samples

The audio callback avoids blocking operations. It does allocate in a few places
that are worth watching, most notably cloning the scale when spawning voices.

### `src/synth.rs`

Defines `Voice`, `VoiceKind`, melodic evolution, and voice spawning.

Voices are sample-by-sample oscillators/envelopes. Melody voices are additive by
default, but `sample_with_wave` lets VCF-targeted voices use `sin`, `tri`,
`squ`, or `saw` oscillator shapes. Sub-bass, drums, phrase-start accents, and
turnaround markers are also generated here.

`evolve_bar` mutates melodic waypoints after each phrase play so repeated
phrases drift instead of repeating exactly.

### `src/vcf.rs`

Defines VCF/VCO data and the filter implementation:

- `VcfSettings`
- `VcfBank`
- `VcfTarget`
- `VcoWave`
- `MoogLadder`

`VcfBank` has four slots: `all`, `bass`, `kanun`, and `kick`. `all` is a master
filter. Per-instrument slots filter only matching `VoiceKind` groups. `all` and
per-instrument modes are mutually exclusive by design.

`advance_tick` applies per-tick automation to enabled slots.

### `src/fx.rs`

Defines `FxSettings` and `FxProcessor`.

FX are global post-mix processors:

- ping-pong delay
- compact feedback reverb

Both are off by default. `advance_tick` applies per-tick automation. The
processor has a fast bypass path when neither effect is enabled.

### `src/ui.rs`

Renders the ratatui interface and handles keyboard input. It reads app state
plus playback atomics each frame. It handles:

- command input and cursor movement
- history navigation
- Tab completion
- help and jins overlays
- status line formatting for BPM, sustain, VCF, FX, volume, phrase count, and
  recording progress

### `src/session_v3.rs`

Serialization helpers for `MAQAM_SESSION_V3`.

V3 writes explicit stable IDs:

```text
P|id|repeat|phrase
J|id|target|times
B|id|bpm
S|id|sustain
V|id|vcf command
F|id|fx command
```

Fields are escaped for `|`, backslash, and newline. Custom jins are written as
plain `create` lines before the timeline, and volume is written as `vol <n>`.

### `src/record.rs` And `src/record_old.rs`

`record.rs` is a thin wrapper over `record_old.rs`.

The recorder expands the current timeline, renders audio offline, applies VCF
and FX, generates the visual score/background, writes temporary media files,
and invokes ffmpeg to produce an MP4. While live playback is still running, the
offline synth loop periodically yields, and ffmpeg is launched at lower priority
on Unix with x264 capped to one thread.

Recording is intentionally outside the real-time audio callback. It can spend
more CPU and allocate freely without causing live audio skips.

### `src/tuning.rs`

Defines the oud-lattice pitch system, pitch parsing, built-in jins/maqam ratios,
custom jins registry, and frequency snapping.

Every parsed pitch is converted to Hz and snapped to the oud lattice before it
is used to build phrase frequencies.

### `src/midi_clock.rs` And `src/midi_clockout.rs`

MIDI clock integration. `clockin <device>` starts a receiver that updates audio
BPM from external MIDI clock. `clockout <device>` starts a sender and receives
later BPM updates from the app.

### `src/carpet.rs`, `src/renderer.rs`, `src/source_background.rs`

Visual rendering support for MP4 output. These files generate score/background
imagery used by offline recording.

### `src/osc.rs` And `src/midi.rs`

Support modules for OSC/MIDI-adjacent work. They are currently not central to
the main app loop.

## Session Loading Flow

`App::load_session` reads the first line:

- `MAQAM_SESSION_V3` -> explicit-ID loader
- `MAQAM_SESSION_V2` -> legacy V2 loader
- `MAQAM_SESSION_V1` -> legacy V1 loader

V3 loading resets custom jins to defaults, applies any `create` lines, loads
volume, then rebuilds timeline entries. Plain control lines under a V3 header
are accepted for convenience. Older numeric VCF records are still accepted.

After loading, the app computes the sequence-start settings by walking leading
control entries before the first musical phrase, then sends `Clear`,
`SetBpm`, `SetSustain`, `SetVcfBank`, `SetFxSettings`, `SetVol`, and every
loaded entry to audio.

## Real-Time Audio Notes

The audio callback must stay non-blocking. The current code mostly follows that
rule:

- command channel is drained with `try_recv`
- jump-counter mutex uses `try_lock`
- FX has a fast bypass when inactive
- VCF processing is per-bus and only active for enabled slots

Known pressure points:

- active FX is more expensive than VCF
- `fx_processor.set_settings(fx)` currently runs on sequencer ticks when FX
  automation advances
- scale data is cloned while spawning voices
- recorder and live engine share behavior but not one unified sequencer engine

## Tests

The test suite lives mostly in module-local `#[cfg(test)]` blocks, especially
in `app.rs`. Current tests cover session loading/saving, VCF command behavior,
FX parameter rules, load completion, bundled session loading, and an ignored
recording smoke test.

Run:

```bash
cargo test
```

## Dependencies

| Crate | Purpose |
|---|---|
| `cpal` | real-time audio output |
| `ratatui` | terminal UI rendering |
| `crossterm` | terminal input/raw mode |
| `crossbeam-channel` | app-to-audio command channel |
| `anyhow` | top-level error handling |

Rust edition is 2021. The toolchain is pinned by `rust-toolchain.toml`.
