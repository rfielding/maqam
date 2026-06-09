# Maqam carpet renderer

Rust checkpoint for the procedural carpet/background renderer.

Run from this directory:

```sh
cargo run --release -- --all
```

Or render one score:

```sh
cargo run --release -- --mq ../../examples/magiccarpet.mq --out out.png --name magiccarpet.mq
```

The Python checkpoint is included as `mq_carpet_surface_gosper_beats_v45.py`.

Current design choices:
- text is drawn into the carpet filter
- `1/1` ratios are omitted from ratio labels
- text is horizontal, not rotated
- center Gosper curve is black and slightly thicker
- jump arrows are white
- output is 2:1 with square pixels
