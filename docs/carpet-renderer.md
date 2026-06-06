# MQ carpet reference pipeline

Branch: `carpet-guided-background`

This branch now preserves the **working visual direction** instead of pretending the failed Rust prototype was useful.

## Canonical reference artifacts

These are the visual and motion targets:

```text
embroidered_map_of_musical_territories.png
    still carpet target

embroidered_map_guided_180bpm_bright_redraw_fixed.mp4
    guided-reading motion target
```

The still image is the target for the carpet language:

- continuous woven territories
- no obvious boxes
- dense ornament everywhere
- seams as geography
- dark enough to work as a terminal/background texture

The MP4 behavior is the target for reading/playback:

- carpet stays fixed and centered
- no pan or zoom
- no ring cursor
- no dot cursor
- current position is shown by a **brighter re-draw of the carpet itself**
- background remains dark enough for text legibility

## Reference script

```bash
python3 scripts/make_guided_redraw_mp4.py \
  reference/carpet/embroidered_map_of_musical_territories.png \
  carpet_guided_redraw.mp4
```

Optional flags:

```bash
--bpm 180
--seconds 20
--fps 30
--width 1280
--height 720
```

Dependencies:

```bash
pip install pillow imageio imageio-ffmpeg numpy
```

`ffmpeg` and `ffprobe` should also be on `PATH`.

## Rust status

`src/bin/mq_carpet.rs` is intentionally disabled.

The first Rust attempt produced sparse color cells and disconnected squiggle lines. That is the wrong aesthetic and should not be merged as a renderer.

The Rust port should only resume after matching the preserved reference behavior:

1. Use the reference PNG as the still carpet target.
2. Use `scripts/make_guided_redraw_mp4.py` as the motion target.
3. Port the bright-redraw behavior first.
4. Only then attempt deterministic source-derived carpet generation.

## Design invariant

The still carpet is the score-like source representation.

Playback state should not create new geometry. It should only guide the eye through the existing carpet by locally brightening/re-drawing the manuscript.
