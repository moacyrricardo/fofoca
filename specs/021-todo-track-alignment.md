Status: todo
Branch: —

# 021 — Keep the mic and call tracks on one timeline

## Context

Each meeting records two tracks with two separate `pw-record` processes (`capture.rs`, started one
after the other in `session.rs`): `mic.wav` and `system.wav`. Everything that joins the two by time
assumes they share t = 0 and run at the same rate:
- the live You/Call guess;
- replay's re-sync (014);
- the after-call per-track transcript, **shipped as 020**: `pipeline/tracks.rs` `merge_tracks` joins
  each track's lines by their own recorded times, and 020's Known Gaps point here for alignment;
- live per-track transcription (**018 + 019**, todo), which places lines from both tracks by start
  time as they arrive.

Two measurements disagree on how far apart the tracks are:

- **Start lag: 85 ms, constant.** Two captures started with `capture.rs`'s own arguments (audio
  discarded) showed the call track starting 84–85 ms after the mic. It stayed constant over 5
  minutes, with no drift. That's harmless for joining lines, but it's a systematic offset.
- **A 2.2 s length difference, cause unknown.** On a real 10:38 call, `mic.wav` is 636.0 s and
  `system.wav` 638.2 s. That recording can't show the cause: the two people took turns with no
  bleed between tracks, so there is no shared sound to line them up on. The possible causes look
  different:
  - **Late start** of one capture (constant offset): lines near turn changes are misordered by up to
    2.2 s.
  - **Drift or dropped samples** (an offset growing over time): late in a long call the tracks are
    far apart.
  - **Different stop time** (one capture ends later; offset ≈ 0 throughout): harmless.

Spec 020 deliberately doesn't align the tracks. Its Known Gaps say a large start lag can misorder
lines near turn changes until this spec lands (decision D3, 2026-11-22).

## Decision

1. **Measure first, then fix only what's measured.** This spec has two parts, in order: a
   measurement (§1–§2) that decides which of the three causes is real, then the matching fix (§3).
   The fix for a cause that isn't observed is not built.
2. **Record each capture's timing** in every meeting: when its first sample arrived and how many
   samples it delivered, against wall-clock time. That's cheap, permanent, and makes every future
   recording diagnosable.
3. **Fix by cause:**
   - **Start lag:** pad the later track's start with silence so both begin at the same wall-clock
     moment.
   - **Drift or dropped samples:** record it and correct it in time mapping (when joining lines and in
     replay), not by resampling the WAVs.
   - **Stop difference:** nothing to fix.

## Implementation

### 1. Timing record (core: `capture.rs`, `session.rs`, `store.rs`)

- `TrackBuffer` records `first_sample_at` (wall clock, `SystemTime`) when its reader thread gets its
  first bytes, plus a running sample count. Every ~10 s it appends a checkpoint
  `(wall_clock, samples_so_far)`.
- On stop, write `timing.json` next to the WAVs:
  `{ mic: { first_sample_at, samples, checkpoints: [...] }, call: { … }, sample_rate }`. It's small
  (one checkpoint per 10 s) and has no audio and no text.
- Pausing (010) is recorded as gaps in the checkpoints, so it isn't mistaken for drift.

### 2. Measurement (CLI)

- `fofoca-cli timing <meeting-id>`: from `timing.json`, print the start lag, each track's rate
  (samples per wall-clock second) and its deviation from 16 000, any gaps, and the end difference.
  Give a verdict: start lag / drift / dropped samples / stop difference.
- `fofoca-cli timing-probe --minutes N`: start both captures as a session does (audio discarded),
  then print the same report. That's the existing 5-minute experiment, made repeatable.
- **Run:** `timing-probe` 3× for 10 minutes, plus **3 real meetings** recorded with this build (the
  user records them; the report has numbers only). Write the results into Implementation Notes and
  pick the fix from them.

### 3. Fix (only the measured cause)

- **Start lag:** in `session.rs`, once both tracks have `first_sample_at`, compute
  `lag = |mic − call|`. Insert `lag` of silence at the start of the later track's buffer **and** WAV.
  Log it. Replay needs no change.
- **Drift or dropped samples:** store a per-track time map in `timing.json` (from the checkpoints).
  Readers that join tracks by time (020's join, replay's re-sync) convert each track's time to wall
  time through it. The WAVs stay untouched.
- **Stop difference:** nothing beyond the report.

### 4. Tests

- Unit tests: the timing report on synthetic checkpoints shows a constant lag, linear drift, a
  dropped block and a stop difference, each identified correctly. Padding inserts exactly `lag`
  samples. The time map round-trips.
- Checks: `cargo test -p fofoca-core`, `cargo clippy --workspace --all-targets` (no warnings), and in
  `app/`: `npx svelte-check`, `npm test`, `npm run build`.

## Known Gaps

- **Past meetings** have no `timing.json`, so they can't be diagnosed or corrected. Only new
  recordings are covered.
- **Imports** (012) have one track. Nothing to align.
- The cause of the **original 2.2 s** may never be known if it doesn't recur. The recorded timing
  makes the next occurrence diagnosable.
- **Acoustic alignment** (cross-correlating the tracks on shared sound) is out of scope. It needs
  bleed between tracks, which a headphone call doesn't have.
