# Claude handoff: generated woven carpet background

## Situation

The current `carpet-guided-background` branch can render video/HUD/audio, but the generated background is still a test scaffold. The README image is a visual target only. It is not loaded at runtime.

The runtime requirement is:

```text
user presses m
-> maqam-live reads the current score/session
-> renderer generates a fresh still/animated background from that score
-> HUD/subtitles are layered on top
-> later, playback ticks brighten the current stitch/path position
```

## Goal

Replace the rectangular/blocky Hilbert debug scaffold with a procedural woven carpet-like score background generated from the current `.mq` session, especially `magiccarpet.mq`.

The desired look is documented in:

```text
docs/carpet-target.md
assets/carpet-target.svg   # README visual target only, not a runtime asset
```

The target should read as a dark woven maqam score artifact:

- dark Persian-rug/manuscript field
- ornate border
- 3-4 adjacent organic territories, not rectangular cells
- embroidered shared seams
- dense stitch ornament hiding the scaffold
- ratio/rhythm data represented as beads, hatch marks, dots, knots, and constellations
- bright terminal HUD remains readable on top

## Important correction

The visual target image was produced by an image generator. There is no checked-in model/code that generated that exact image.

However, there are real Python reference files in the user's File Library from earlier procedural attempts. Use them as source/design references:

```text
reconstruct_embroidered_carpet_no_flowers.py
reconstruct_embroidered_carpet.py
hilbert_interval_weaver_v43.py
hilbert_interval_weaver_v35.py
```

Key ideas from those Python files:

- source-only procedural carpet reconstruction, no external input image
- Hilbert should provide hidden locality, not visible square grid
- segment-driven Hilbert path: each tick/beat appends or visits a segment `p0 -> p1`
- high-order Hilbert sampling preserves inherited local orientation and avoids quadrant jumps
- draw stitches/marks along local Hilbert segment direction
- ratio/rhythm values modulate color, hatch angle, bead/knot placement, motif density

## Current repo state to inspect

Start with:

```bash
git fetch origin
git checkout carpet-guided-background
git pull
cargo build --release
cargo run --release -- load magiccarpet.mq -- z -- m
```

The result currently has usable HUD/text but the background is a blocky test scaffold.

Primary files likely involved:

```text
src/record.rs              # wrapper around old recorder / current bridge
src/record_old.rs          # existing HUD/subtitle/audio rendering path
src/source_background.rs   # experimental generated background renderer
src/renderer.rs            # older/background renderer utilities may exist
src/sequencer.rs           # Phrase/session structure
README.md
docs/carpet-target.md
```

Do not break HUD/audio while changing visuals.

## Architectural rule

The carpet image is generated from score data. It is not a static asset.

```text
static asset in README: visual target only
runtime background: generated from current score/session when m is pressed
```

## Renderer strategy

Use Hilbert internally, but do not show Hilbert cells directly.

Bad visual:

```text
phrase ownership -> rectangles / visible square cells / debug bins
```

Good visual:

```text
score/event stream -> Hilbert locality -> stitch direction / territory adjacency -> woven image
```

Suggested passes:

1. Base field
   - very dark woven textile texture
   - warp/weft threads
   - subtle vignette
   - dark enough for green HUD text

2. Organic region underpaint
   - map phrase/event ranges to Hilbert intervals
   - blur/smooth/dilate ownership into soft organic territories
   - use jewel-tone dark fills
   - do not display raw square boundaries

3. Embroidered seams
   - derive contours where smoothed phrase territories meet
   - draw dark/gold shared seams
   - add bead/knot accents at rhythm boundaries and jump points

4. Dense stitch overlay
   - sample many Hilbert segments `d -> d+1`
   - compute `p0`, `p1`, local direction and perpendicular
   - draw short thread strokes along segment direction
   - draw perpendicular hatches based on ratio/rhythm token energy
   - place small knots/dots/constellations from ratio hashes
   - make this dense enough to bury the underlying partition

5. Border and final unification
   - ornate frame/border
   - final weave/noise overlay across all regions
   - keep contrast low enough for readable HUD

## Deterministic playback map for later animation

While generating the still, also keep or be able to recompute a deterministic mapping:

```text
score tick / subdivision -> event index -> Hilbert interval/segment -> canvas point
```

Future animation should brighten/redraw the current stitch or bead at that point rather than moving an unrelated marker.

## Immediate task

Make `cargo run --release -- load magiccarpet.mq -- z -- m` produce a dark woven procedural still background that is clearly not the current blocky scaffold, while preserving HUD/audio.

Success criteria:

- HUD/text visible
- video renders from `m`
- background is generated, not static target asset
- no bright white/pink background
- no obvious square Hilbert grid
- looks more like a woven rug than a debug map
- `cargo build --release` passes

## Suggested commit message

```text
Generate woven carpet background from score
```
