# Claude handoff: generated woven carpet background

## Situation

The current Rust renderer already generates the carpet at runtime. The README
image remains a visual target only; it is not loaded directly.

The runtime path is:

```text
user presses m
-> maqam-live reads the current score/session
-> renderer generates a fresh still background from that score
-> HUD/subtitles are layered on top
```

## Goal

The active renderer now already includes:

- a dark woven field
- outer and inner borders
- maqam-colored inner border ticks
- maqam legend in the lower band
- jump arrows and counters inside the inner border
- readable HUD/subtitles layered on top

## Notes

The preserved design references in `docs/carpet-target.md` still matter as
visual guidance, but they are not runtime inputs.

Do not break HUD/audio while changing visuals.

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
