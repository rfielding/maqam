# MQ carpet renderer

This document describes the active carpet/background renderer used by the Rust
code in `src/carpet.rs` and `src/record_old.rs`.

## Current behavior

- The carpet background is generated from the loaded score/session when `m` is pressed.
- The base field is a dark woven texture, with geometric motifs inside the border bands.
- The inner score border is color-matched to the maqam ratios used by each phrase.
- The lower border band carries a maqam legend with glyph, name, and pitch ratios.
- Jump arrows are drawn inside the inner border; repeat counters and BPM/sustain are shown in the HUD/subtitles.
- The video overlay remains terminal-friendly and readable against the dark background.

## Notable files

- `src/carpet.rs` — carpet geometry, borders, motifs, legend, and jump arcs
- `src/record_old.rs` — MP4 assembly, HUD text, subtitles, and ffmpeg filter chain
- `src/bin/mq_carpet.rs` — disabled placeholder binary kept as an explicit stop sign for the old prototype path

## Design invariant

The carpet is derived from score data, not from a static asset. Visual layers
should remain legible and should not introduce unrelated debug geometry.
