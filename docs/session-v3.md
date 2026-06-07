# MAQAM_SESSION_V3

V3 exists because the renderer needs stable line identities.  The carpet cannot infer
structure from anonymous text lines.  Every timeline entry must carry its own id.

Header:

```text
MAQAM_SESSION_V3
```

Non-timeline setup lines remain anonymous because they do not become carpet
territories:

```text
create <name> <ratio>...
vol <float>
```

Timeline records are pipe-delimited and always include an id in field 2.

```text
B|id|bpm
S|id|seconds
P|id|repeat|source
J|id|target_id|times
```

Meanings:

```text
B|0|180              bpm control line, id 0, bpm 180
S|1|1.2              sustain control line, id 1, 1.2 seconds
P|2|1|g hijaz 4444   playable phrase, id 2, repeat 1, source command
J|3|2|3              jump line, id 3, target id 2, jump count 3
```

The renderer source of truth is the loaded `Vec<Phrase>`, not the raw `.mq` text.
The ids in this format become stable visual coordinates:

- phrase id -> carpet territory
- jump id -> knot / seam
- jump target id -> destination territory
- phrase bar events -> rhythm movement inside the territory

Compatibility plan:

- Save should emit V3.
- Load should accept V3.
- Load should keep accepting old anonymous V2 files long enough to migrate them.
- The old V2 save format should not be used for new files because ids can drift.
