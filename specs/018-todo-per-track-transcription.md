Status: todo
Branch: —

# 018 — Transcribe the mic and call tracks separately

## Context

fo-foca records two tracks per meeting, `mic.wav` (you) and `system.wav` (the call), but it
**transcribes them mixed**. `Session::transcribe_chunk` (`session.rs`) sends `audio::mix(mic, sys)`
to Whisper. Each returned segment is then labelled You or Call by `audio::attribute`: the louder
track over the segment's time range wins, and the mic has to be ≥ 1.2× the call.

Whisper cuts segments at sentences, not at speaker changes, so one segment often holds both people.
The longer and louder speaker then takes the whole line. Measured on a real 2-person, 10:38 pt-BR
call (2026-11-20; the numbers come from per-second loudness of the two WAVs, and no audio was
copied):

- **78 lines were labelled Call and 19 You**, although the two people were taking turns.
- 00:00 "E aí Vini, tudo bem?" has the call at 0–2 s and you at 2–3 s, and was labelled Call.
- 00:29 "Sim, você tinha falado mesmo. Cara, eu tinha esquecido." has you at 29–30 s and the call
  at 31–32 s. The mic was 0.84× the call (it needed 1.2×), so the line was labelled Call.
- The audio itself is clean. In the seconds where you speak, the mic is 30–800× louder than the
  call, and your voice is not in `system.wav`. **The channels are already separate. The
  transcription throws that away.**

Two related defects show up on the same call:

- **Speaker over-count.** Speaker separation (003) runs on the call track only, yet it found 3
  "Persons" on a call with one remote voice (s1: 56 lines, s3: 21, s2: 1).
  `max_speakers = (attendees + 1).max(4)` (`pipeline.rs`) allowed 4 for a 2-attendee invite.
- **Track length mismatch.** `mic.wav` is 636.0 s and `system.wav` is 638.2 s. Either one capture
  started later or one dropped samples; it isn't known which. Lines merged from two tracks can only
  be ordered correctly if the tracks share one timeline.

## Decision

1. **Each track is transcribed on its own, live and after the call.** A line's track (`you` /
   `call`) comes from the file it was transcribed from, so there's no loudness guess. The mixed
   transcription and `audio::attribute` are **removed**, not kept as a setting. Imports have a
   single track and stay `call`, as today.
2. **Live:** for every chunk, send each track that has speech to Whisper separately. Merge the two
   result lists by start time before emitting `Segment` events. A track with no speech in the
   chunk costs nothing.
3. **After the call: rebuild the transcript per track.** This is a new after-call step,
   `transcript_after`, on by default and toggled in Settings → Processing next to the 015 steps. It
   re-transcribes each whole track with the GPU free and no chunk boundaries, then merges both by
   time and replaces the live transcript. The live transcript is kept as `transcript.live.jsonl`,
   which already exists for this. Speakers and notes then run on the rebuilt transcript.
   Record-only mode (015) already transcribes after the call; it becomes per-track by the same code.
4. **Speaker cap from the invite:** when the meeting has ≥ 2 calendar attendees, speaker separation
   on the call track gets `max_speakers = attendees − 1`, assuming the user is one of them. With 0–1
   attendees it stays as today.
5. **One timeline:** record when each capture's first sample arrived, and pad the later track's
   start with silence so both start at t = 0 together. Measure first which cause explains the
   2.2 s gap (see Implementation §5).
6. **No echo transcribed twice:** without headphones the mic hears the call. A mic line that repeats
   an overlapping call line is dropped (Implementation §3).

## Implementation

### 1. Speech per track (core, `audio.rs`)

- New `fn has_speech(track: &[i16]) -> bool` with the same relative threshold `pause_cut` already
  uses: noise floor = 10th percentile of 50 ms window levels, and quiet = `(floor × 2).max(0.006)`.
  It returns true when ≥ 300 ms in total of 50 ms windows lie above that threshold.
  - An absolute level doesn't work: mic noise (0.003–0.015 RMS) overlaps quiet speech, as `pause_cut`'s
    comment notes.
  - Whisper's Silero VAD (`--vad`) still runs on the server. This check only avoids sending silent
    tracks at all.
- Unit tests: tone vs silence vs noise-only buffers; a buffer of noise with 200 ms of speech returns
  false, and one with 400 ms returns true.

### 2. Live chunks (core, `session.rs`)

- `transcribe_chunk(mic, sys)` becomes:
  ```text
  for (track, samples) in [(You, mic), (Call, sys)] where has_speech(samples):
      raw = whisper.transcribe(wav(samples), language, prompt)
      segments += raw mapped to Segment { track, start/end/words offset by chunk start }
  sort segments by start; drop echoes (§3); append + emit in that order
  ```
- The existing near-silence skip (`rms(mic).max(rms(sys)) < 0.003`) is replaced by `has_speech`
  per track.
- **Prompt:** both calls get the same prompt as today: the last 200 characters of the merged
  transcript. Whisper uses the prompt for vocabulary and continuity. Using each track's own history
  instead is an option to measure, not a requirement.
- Order on the GPU: mic first, then call. Both run in the same Whisper slot as today, with no
  parallel requests. 015's layouts and swap logic don't change: it is still one "transcribe" phase
  per chunk.
- **Cost (measured on the 10:38 call, 12 s chunks):** 37 of 53 chunks have speech on both tracks,
  16 on the call only and 0 on the mic only. That's about **1.7× the Whisper requests** of today.
  Whisper's encoder cost is per request, not per second of speech, so expect live Whisper time to
  rise by about that much. The build must measure, on the MX230 Balanced layout, the text delay
  before and after (010 measured 12.7 s) and whether the backlog stays bounded. If it doesn't, see
  Known Gaps.
- `audio::mix` stays if anything else uses it. `audio::attribute` and its test are deleted.

### 3. Echo (no headphones)

- When the mic picks up the speakers, the mic track contains a quieter copy of the call, and Whisper
  will transcribe it as You.
- Rule: drop a mic segment when a call segment overlaps it in time by ≥ 50 % of the mic segment's
  length **and** ≥ 60 % of the mic segment's normalised words (lower-cased, accents and punctuation
  stripped) appear in that call segment.
- Applied in the live merge and in the after-call rebuild. The dropped count goes to the log, not
  the UI.
- Unit tests cover: an exact echo, a partial echo, a real overlapping reply (different words, kept),
  and "sim" vs "sim" in non-overlapping time (kept).

### 4. After-call rebuild (core, `pipeline.rs` + `config.rs` + Settings)

- `config.rs` `[workflow]`: new `transcript_after: bool`, default `true`, saved with `toml_edit` as
  the other workflow keys are (015). Settings → Processing tab: an "After the call" toggle
  "Re-transcribe each track", placed above "Separate speakers". It uses the same pure helper style
  as `lib/workflow.ts`, plus a test.
- `pipeline::retranscribe(store, cfg, id, hooks)`:
  - For each existing track, call `transcribe_samples` (the 012 import path: quiet cuts, offsets)
    with that track's `Track`.
  - Merge by start, drop echoes (§3), then `store.replace_transcript(id, &segments)`. That keeps the
    first live copy as `transcript.live.jsonl`.
  - Mark the step done (`mark_done(…, "transcript")`). It runs **before** `speakers` and `notes` in
    the after-call order.
- `transcribe_samples` gains a `track: Track` parameter instead of hard-coding `Track::Call`.
  Imports pass `Call`.
- `speakers::assign` reads the transcript to split. It must use the rebuilt transcript, so after
  this step `live_transcript()` is no longer the right source for the speakers redo. Add
  `Store::base_transcript(id)`: the rebuilt one when it exists (`transcript.base.jsonl`, written by
  `retranscribe`), otherwise the live one. The speakers redo reads it.
- If the meeting's audio was already deleted (retention 011) or a track is missing, rebuild from
  what exists, or skip the step when nothing does. Never fail the pipeline.
- Processing screen: the new step shows as "Transcribing each track" with progress, reusing
  `hooks`/`JobEvent`.

### 5. One timeline (core, `capture.rs` / `session.rs`)

- `Capture` records `first_sample_at: Instant` when its reader thread gets its first bytes.
- At session start, once both captures report it, compute `lag = |mic − sys|`. Insert `lag` worth
  of silence at the start of the later track's buffer **and** WAV, so `t = 0` matches. Log the lag.
- **Measure before building the fix:** record a 5-minute meeting (with sound on both tracks) and
  compare each track's sample count with wall-clock time. That shows whether the 2.2 s comes from
  start lag (constant) or from drift or dropped samples (growing over time).
  - Start lag: the padding above fixes it.
  - Drift: out of scope here; write it in Implementation Notes and a follow-up spec.
- Replay (014) already plays both tracks against the call track as master clock. The padding makes
  its re-sync smaller and needs no change there.

### 6. Speaker cap (core, `pipeline.rs`)

- `max_speakers = if meta.attendees.len() >= 2 { meta.attendees.len() - 1 } else { (attendees + 1).max(4) }`.
  The clustering threshold is unchanged.
- Test: 2 attendees → 1 cluster, even when two embeddings sit just under the threshold.
- Imports have no attendees and stay as today.

### 7. Tests and checks

- New unit tests: §1, §3, §6, the merge-by-time ordering, the `transcript_after` config
  round-trip, and the Settings helper.
- The existing ignored real-model test gets a per-track variant. It is also ignored and run by hand.
- Hand test (user): a real call with headphones, then one without, then one in record-only mode.
  Check the You/Call labels in the transcript and the replay.
- Checks as in the handoff: `cargo test -p fofoca-core`, `cargo test -p fofoca`, clippy
  warning-free, `npx svelte-check`, `npm test`, `npm run build`.

## Known Gaps

- **Live cost.** About 1.7× the Whisper requests. If the measured text delay or backlog on the
  MX230 Balanced layout gets clearly worse, the fallback is to put both tracks into **one** request,
  separated by 1 s of silence, and split the results at the boundary. That's one encoder pass for
  two tracks. It's left out because the boundary mapping is fragile; build it only if the
  measurement asks for it.
- **Overlapping speech** shows as two lines with close start times, one per track. There's no
  interleaving inside a line.
- **Clock drift** between the two captures (as opposed to start lag) isn't corrected here (§5).
- **Echo rule is text-based.** A mic echo that Whisper transcribes differently from the call line
  can slip through. Acoustic echo cancellation (PipeWire's `echo-cancel` module) is out of scope.
- **The speaker cap assumes the user is in the invite.** For an invite listing only the remote
  people, the cap is 1 too low. 015 Settings may later need a "people on the call" override.
- **Past meetings aren't rebuilt automatically.** A "Re-transcribe" button per meeting would reuse
  `retranscribe`. It's left for later, and only works where the audio is still kept (011).
