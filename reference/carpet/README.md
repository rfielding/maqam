# Carpet reference assets

The reference PNG/MP4 are intentionally not committed here yet because they are binary generated artifacts.

Put the preserved reference image here before running the guided-redraw script:

```text
reference/carpet/embroidered_map_of_musical_territories.png
```

From the snapshot zip, copy it with:

```bash
mkdir -p reference/carpet
cp mq_carpet_reference/assets/embroidered_map_of_musical_territories.png \
  reference/carpet/embroidered_map_of_musical_territories.png
```

Then run:

```bash
python3 scripts/make_guided_redraw_mp4.py \
  reference/carpet/embroidered_map_of_musical_territories.png \
  carpet_guided_redraw.mp4
```
