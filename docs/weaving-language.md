# Weaving Language

The carpet is generated from score-time, not performance-time.

## Structure

- The outer border is the physical carpet edge.
- The inner border is a closed score cycle.
- The inner border contains one segment per score tick.
- Every musical phrase receives the same total border length.
- A visible gap separates adjacent phrase ranges.
- Ticks are distributed evenly inside their phrase range.
- The first tick of each rhythm group receives a stronger knot marker.
- Kick ticks use a distinct gold/orange thread color.
- During playback, the current score tick is highlighted on the border.
- The lower border band carries the maqam legend.
- The carpet also includes small geometric motifs inside the border bands.

## Score Ticks

Only playable phrase lines contribute score ticks.

- Jumps do not contribute ticks.
- BPM and sustain controls do not contribute ticks.
- Phrase repeat counts do not contribute ticks.
- Performance traversal does not duplicate ticks.

Rhythm digits describe group lengths. `4444` contributes sixteen ticks.
Repeated non-uniform cyclic motifs are represented once: `332332` contributes
the canonical cycle `332`, or eight ticks.

Therefore this score:

```text
0: d bayati 4444
1: j 0 3
2: c rast 332332
3: j 0 4
```

has twenty-four inner-border ticks:

```text
4 + 4 + 4 + 4 + 3 + 3 + 2
```

Jumps, repeats, BPM, and sustain are represented separately from the tick
count: jump arrows, repeat counters, and the HUD overlay show them without
changing the immutable score border.

## Jump Arrows

Each jump is drawn just inside the inner score border. Its path begins at the
end of the playable phrase immediately preceding the jump line, follows the
border backward, and points to the start of the target phrase. Multiple jumps
use progressively inset lanes so their woven paths remain distinguishable.
