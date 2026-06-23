# CRITIQUE.md

This is a pragmatic critique of the current codebase as it stands. The app is
already musically useful, but the code is carrying some real complexity now.

## What Is Working Well

The central idea is strong: the timeline is a live-editable score made of
musical phrases plus control entries. Stable IDs are the right choice for that
model, because jumps and edits survive reordering.

The VCF design is also a good fit for the instrument. The `VcfBank` distinction
between `all` and per-instrument filters maps well to live performance, and the
command parser preserves relative changes instead of prematurely flattening
them to absolute values. That makes looping automation possible.

The session format is moving in the right direction. `MAQAM_SESSION_V3` is
explicit about entry IDs and stores VCF/FX commands as text payloads, which is
more robust than trying to infer meaning from anonymous lines.

The tests are useful and are aimed at actual behavior: VCF semantics, FX
relative changes, session compatibility, and completion behavior.

## Main Risks

### `app.rs` Is Too Large

`src/app.rs` owns command dispatch, session loading, session migration, path
completion, recording launch, MIDI clock command handling, VCF/FX display
helpers, and many tests. It is the center of the program, but it has become the
place where unrelated concerns accumulate.

The next clean split would be:

- `session.rs` for V1/V2/V3 load/save and migration
- `completion.rs` for `.mq` path completion
- `timeline.rs` or `timeline_edit.rs` for insert/edit/delete/move operations
- keep `app.rs` as orchestration and UI-facing state

This would reduce regression risk when changing commands.

### Live And Offline Sequencing Are Not Unified

The live engine and recorder try to match behavior, but they do not share one
sequencer engine. That invites drift. Any future change to jump handling,
control entries, tick automation, phrase evolution, or warning accents must be
made in more than one path or carefully checked.

A shared non-audio sequencer core would help:

- input: timeline, current transport state, sample rate/BPM
- output: events, milestones, control changes
- users: live audio and offline render

That is a larger refactor, but it is the one that would most improve confidence.

### The Audio Callback Still Does Some Work It Should Avoid

The real-time callback mostly avoids blocking, but it still does avoidable work:

- cloning the scale vector while spawning voices
- rebuilding/copying FX settings on tick automation
- retaining voices in the hot path
- using a `Vec<Voice>` that can grow and compact dynamically

This is probably acceptable for current sessions, but delay and reverb already
exposed CPU pressure. A future hardening pass should preallocate voice storage,
avoid per-event scale clones, and update cached FX lengths only when automated
parameters actually changed. The current recorder now cooperatively yields and
uses lower-priority single-thread ffmpeg encoding, which reduces skips but does
not make recording truly real-time-safe.

### Parser And Source Formatting Are Entangled

The parser returns structured changes, but `app.rs` reconstructs display/session
source strings with helpers such as `vcf_change_src` and `fx_change_src`.
Because source text is saved in V3 records, those helpers are now part of the
session format in practice.

It would be cleaner if each command/change type had one canonical formatter
near the parser. That would make round-tripping easier to reason about.

### Some Names Hide The True Model

`Phrase` is no longer always a phrase. It can be a jump or a settings entry.
That was a reasonable early simplification, but it now obscures intent.

The cleaner model is probably:

```rust
struct TimelineEntry {
    id: usize,
    src: String,
    kind: TimelineKind,
}

enum TimelineKind {
    Phrase { bar: Bar, repeat: usize },
    Jump(JumpSpec),
    Control(ControlSpec),
}
```

That would remove many `jump.is_none()` / `control.is_some()` checks and make
invalid states impossible.

### `record.rs` Is A Wrapper Around `record_old.rs`

This is understandable during fast development, but the name now sends the
wrong signal. If `record_old.rs` is the current implementation, it should be
renamed back to something current or split into smaller modules.

## Correctness Concerns To Watch

- Control entries at the beginning of the sequence define start settings, but
  control entries elsewhere execute as the sequencer reaches them. This is
  powerful, but easy to misunderstand if a session starts mid-sequence.
- `all` VCF and per-instrument VCF are mutually exclusive. That is musically
  intentional, but commands that switch modes clear other enabled slots.
- Tick automation is advanced on sequencer events, not per audio sample. That
  matches the command language, but should stay explicit in docs and tests.
- The path completion recursion skips hidden dirs and `target`; more exclusions
  may be needed if users keep large generated folders in the repo.

## Suggested Refactor Order

1. Rename `Phrase` to `TimelineEntry` with an enum kind.
2. Move session loading/saving out of `app.rs`.
3. Move completion out of `app.rs`.
4. Give VCF/FX changes canonical parser-side formatters.
5. Extract a shared sequencer core for live and offline rendering.
6. Do a focused real-time allocation pass in `audio.rs`.

That order keeps behavior stable while peeling off the highest-risk complexity.
