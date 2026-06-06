# MQ carpet renderer prototype

Branch: `carpet-guided-background`

This branch adds a deterministic source-level carpet renderer prototype at:

```bash
src/bin/mq_carpet.rs
```

It renders a `.mq` session as a symbolic carpet, not as an expanded performance trace.

## Run

Generate a terminal-safe dark still image as PPM:

```bash
cargo run --release --bin mq_carpet -- growl.mq growl-carpet.ppm
```

Convert to PNG:

```bash
ffmpeg -y -i growl-carpet.ppm growl-carpet.png
```

Generate a brighter art-mode still:

```bash
cargo run --release --bin mq_carpet -- growl.mq growl-carpet-art.ppm --art
ffmpeg -y -i growl-carpet-art.ppm growl-carpet-art.png
```

Generate a guided-reading MP4:

```bash
cargo run --release --bin mq_carpet -- growl.mq growl-tour.mp4
```

The MP4 keeps the carpet static and centered. The current reading location is shown by a brightened re-draw of the carpet under the current position, not by a cursor ring or dot.

## Current visual rules

- Phrase commands become organic woven territories.
- Rhythm digits modulate internal bands and subdivision dots.
- JI ratios become small harmonic stitch constellations.
- 81/80 comma pairs get a brighter local stitch when present.
- Jump commands are symbolic seams/knots; loop counts are not expanded.
- Terminal mode darkens the final carpet so it can be used as a background behind light text.

## Important limitation

This is a deterministic prototype, not yet the final surreal AI-carpet style. It is meant to provide the Rust geometry pipeline and MP4 reading-cursor behavior so the style can be iterated in code.

## Design invariant

The still carpet is the source representation. Playback, pan/zoom/highlight, and the MP4 tour are just guided reading state.
