# maqam-live

A real-time terminal sequencer for Arabic maqam music using just intonation
synthesis. It is built for live-coding short maqam phrases, moving through a
timeline of phrases and control entries, shaping the sound with per-instrument
VCF/VCO settings, and rendering the current score to MP4.

![maqam-live screenshot](screenshot1.png)

The screenshot is a visual target for generated score backgrounds. At runtime,
recording generates a background from the current session and overlays the
terminal HUD.

## Build

Prerequisites:

- Rust, via `rustup`: <https://rustup.rs/>
- Linux audio development headers if building on Linux, for example
  `libasound2-dev pkg-config`
- `ffmpeg` on `PATH` if you want MP4 recording

```bash
cargo build --release
cargo run --release
```

The package default binary is `maqam-live`. If command-line arguments are
provided, the app runs those commands without opening the TUI:

```bash
cargo run --release -- "load default.mq" -- "m 1"
```

Use `--` between command groups when a single shell invocation should run
multiple maqam-live commands.

## Concepts

### Jins And Maqam

A jins is a short scale fragment. maqam-live treats each phrase as one or more
stacked ajnas, all tuned as exact just-intonation ratios. A single jins phrase
uses the notes available in that fragment. A comma stacks ajnas into one
combined scale:

```text
d bayati 332
d bayati, a nahawand 332
d hijaz, a kurd 44
```

Root names are `c d e f g a b`. Append `+` or `-` for sharp/flat-like oud
lattice positions, for example `d+` or `f-`.

Built-in jins names can be abbreviated to an unambiguous prefix:

```text
nah bay hij ras kur sab aja nik suz jih zab
```

### Rhythm Groups

Rhythm is written as non-zero digits. Each digit is a group size. The first
subdivision of a group is a kick (`X`), and the remaining subdivisions are
snares (`.`).

```text
332   ->  X..X..X.
44    ->  X...X...
4444  ->  X...X...X...X...
664   ->  X.....X.....X...
21    ->  X.X
```

If a phrase omits rhythm, it inherits the last rhythm used.

## Commands

### Phrases

```text
<root> <jins> [rhythm]
<root> <jins>, <root> <jins> [rhythm]
<root> <jins> [rhythm] r<N>
```

Examples:

```text
d bayati 332
a nahawand 44 r3
d bayati, a nahawand 332
```

### Timeline Control

The sequence is a timeline of musical phrases plus control entries. IDs are
stable: deleting or moving entries does not renumber existing IDs.

```text
j <id> [times]                       jump back to id, then fall through
i <id> <command>                     insert before id
edit <id> <command>                  replace an entry
x <id> [id ...]                      delete entries
up <id> / down <id>                  move an entry one slot
rot                                  move the last entry to the front
```

Examples:

```text
d bayati 332
j 0 4
c rast 332
i 2 f hijaz 332
edit 1 j 0 6
x 3
```

`edit` is blocked for the currently playing phrase.

### Playback

```text
z                  toggle pause/play; unpause restarts from phrase 0
z <id>             seek to phrase id without toggling pause
q / quit           quit
? / help           show help
;                  separate multiple commands on one line
```

### Settings Entries

These commands update current state and also append a timeline entry, so they
can be moved, edited, saved, loaded, and replayed as part of the piece.

```text
bpm <n|+n|-n|*k|/k>                  tempo, range 20..400
s <n|+n|-n|*k|/k>                    sustain seconds, range 0.05..10
sus <n|+n|-n|*k|/k>                  same as s
vol <n>                              live volume multiplier, range 0..2
```

Examples:

```text
bpm 180
bpm *2
s 1.5
s *0.8
vol 0.8
```

`vol` is saved in sessions, but unlike `bpm` and `s`, it is not a timeline
entry.

### VCF And VCO

The VCF is off by default. It is a Moog-ish resonant low-pass filter. The
filter can be applied to one master mix (`all`) or per instrument (`bass`,
`kanun`, `kick`). Enabling `all` disables the per-instrument filters. Enabling a
per-instrument filter disables `all` but leaves other per-instrument filters
alone.

```text
vcf off
vcf all off
vcf <all|bass|kanun|kick> off
vcf <target> <cutoff> [res] [drive]
vcf <target> cut=<hz|+n|-n|*k|/k|+nt> res=<n> drive=<n> wave=<shape>
cut <hz|+n|-n|+nt>
res <n|+n|-n|+nt>
drive <n|+n|-n|+nt>
```

Wave names must be named parameters:

```text
wave=sin
wave=tri
wave=squ
wave=saw
```

Examples:

```text
vcf bass 900 0.65 3.5 wave=saw
vcf kanun cut=2400 res=0.35 drive=2.0 wave=tri
vcf kick cut=700 res=0.25 drive=2.5 wave=squ
vcf bass cut=+2t
vcf bass cut=+0
vcf all off
```

Relative changes only affect the parameter named. Tick changes such as
`cut=+2t` add that amount on each sequencer tick. `cut=+0`, `res=+0`, and
`drive=+0` stop movement for that parameter.

### FX

Reverb and ping-pong delay use the same named-parameter and relative-change
rules as VCF. They are off by default.

```text
reverb on
reverb off
reverb mix=<0..1> decay=<0..0.98>
delay on
delay off
delay time=<0.01..2> feedback=<0..0.95> mix=<0..1>
pingpong time=<0.01..2> feedback=<0..0.95> mix=<0..1>
fx off
```

Examples:

```text
reverb mix=0.25 decay=0.7
pingpong time=0.33 feedback=0.45 mix=0.2
delay mix=+0.1
delay feedback=+0.01t
delay feedback=+0
fx off
```

Delay and reverb are more expensive than the VCF in the real-time callback.
For heavy sessions, prefer `cargo run --release` or a built release binary.

### Sessions

```text
save <file>
save                 reuse last loaded/saved path
load <file>
clear
```

Saved sessions use `MAQAM_SESSION_V3`, with explicit stable IDs. Records include:

```text
P|id|repeat|phrase
J|id|target_id|times
B|id|bpm
S|id|sustain
V|id|vcf command
F|id|fx command
vol <n>
create <Name> <ratio> ...
```

The loader accepts V3 plus older V1/V2 session formats.

Tab completion works for `save` and `load` paths. For `load`, completion looks
for `.mq` files recursively when the partial path has no slash, lists ambiguous
matches, and completes a unique match or common prefix.

### Recording

```text
m
m <n>
m<n>
```

Records one or more cycles to `./maqam-<timestamp>.mp4`. Recording runs on a
worker thread and reports progress in the TUI. Offline rendering uses the same
synth, VCF bank, and FX settings as live playback. The recorder yields during
CPU-heavy synthesis and runs ffmpeg at lower priority with single-threaded
x264 encoding so live audio has scheduling room.

### Jins Registry

The jins registry is editable at runtime.

```text
ls
audition <Name>
audition <root> <Name>[, <root> <Name> ...]
create <Name> <p/q> <p/q> ...
delete <Name>
```

Examples:

```text
audition Hijaz
audition d bayati, f hijaz
create Zaba 1/1 12/11 32/27 11/8
delete Zaba
ls
```

Custom jins are saved as `create` lines and restored before phrases are loaded.

### MIDI Clock

These commands are handled by the app before the main parser:

```text
clockin <device>       receive MIDI clock and sync BPM
clockout <device>      send MIDI clock at current BPM
```

`clockout` receives later BPM updates from the app.

## Built-In Jins

| Name | Ratios | Character |
|---|---|---|
| Nahawand | `1/1 9/8 32/27 4/3 3/2` | Natural minor |
| Bayati | `1/1 12/11 32/27 4/3 3/2` | Neutral second |
| Hijaz | `1/1 256/243 81/64 4/3 3/2` | Augmented second |
| Rast | `1/1 9/8 27/22 4/3 3/2` | Neutral third |
| Kurd | `1/1 256/243 32/27 4/3 3/2` | Phrygian |
| Saba | `1/1 13/12 32/27 80/64` | Major-third endpoint |
| Zaba | `1/1 12/11 32/27 11/8` | Tritone endpoint |
| Ajam | `1/1 9/8 5/4 4/3 3/2` | Major |
| Nikriz | `1/1 256/243 81/64 4/3 3/2` | Hijaz lower |
| Suznak | `1/1 9/8 27/22 4/3 3/2` | Rast lower |
| Jiharkah | `1/1 9/8 5/4 4/3 3/2` | Ajam lower |

## Example

```text
bpm 140
s 1.5
vcf bass 900 0.65 3.5 wave=saw
reverb mix=0.18 decay=0.7

d bayati 332
j 3 3
a nahawand 332
pingpong time=0.33 feedback=0.42 mix=0.2
d bayati, a nahawand 664

m 2
```

## Source

https://github.com/rfielding/maqam
