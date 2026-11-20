Status: done
Branch: moacyrricardo/spec-018-per-track-transcription

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
- The first line (a greeting and its answer) has the call at 0–2 s and you at 2–3 s, and was
  labelled Call.
- The line at 00:29 (your reply and theirs) has you at 29–30 s and the call at 31–32 s. The mic
  was 0.84× the call (it needed 1.2×), so the line was labelled Call.
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

## Implementation Notes

Built on `moacyrricardo/spec-018-per-track-transcription` (PR #23). The Decision is in place:
each track is transcribed on its own, live and after the call, and `audio::attribute` and
`audio::mix` are gone.

### Where it landed

- `audio::has_speech` (§1) and a new `tracks.rs`: `with_speech` (tracks of a chunk worth a
  request, mic first), `transcribe_parts` (one request per track, Whisper passed in as a closure,
  so the original bug has a model-free regression test), `merge` + `is_echo` (§3), `segment`.
- `session.rs`: live chunks go through `tracks`. After the call, `rebuild_transcript` runs when
  `transcript_after` is on or in record-only, reusing the live Whisper if it's loaded. Both tracks
  are aligned on the first sample (`align_tracks`).
- `pipeline.rs`: `transcribe_samples(cfg, samples, track, hooks)`, `retranscribe` /
  `retranscribe_with` → `Rebuild::{Done, Skipped}`, `STEP_TRANSCRIPT = "transcript"`,
  `max_speakers(attendees)`. The speakers step reads `Store::base_transcript`.
- `store.rs`: `rebuild_transcript` writes `transcript.base.jsonl` and replaces `transcript.jsonl`;
  the live copy stays in `transcript.live.jsonl`. `base_transcript` falls back to the live one.
- `capture.rs`: `TrackBuffer::first_sample_at`, `pad_front`, `start_lag`.
- UI: Settings → Processing → "Re-transcribe each track" above "Separate speakers" (record-only
  shows a forced "Transcribe each track" row). `workflow.ts` gets `transcriptDevice`, and
  `afterSteps` adds "Re-transcribe". The Processing screen shows "Transcribing each track · N %".
  `fake.ts` mirrors all of it.
- `fofoca-cli bench-latency <call.wav> --mic <mic.wav> [--mixed]` measures live delay per track,
  or mixed as before.

### Differences from the spec

- **Progress event.** The session reports rebuild progress with a new
  `SessionEvent::Progress { step, progress }`. `Hooks` are reused, but `JobEvent` belongs to
  import jobs, so it wasn't used.
- **Silence skip in the shared path.** `transcribe_samples` now skips silent stretches with
  `has_speech` instead of `rms ≥ 0.003`, and loads Whisper only once a chunk has speech. That
  changes imports too: a quiet stretch with only noise is no longer sent.
- **Rebuild prompt.** The after-call rebuild carries each track's own previous line as the prompt
  (the import path). The spec only fixed the prompt for live chunks (tail of the merged
  transcript), and that is what live does.
- **Empty rebuild.** If the rebuild yields no lines while a live transcript exists, the live one is
  kept (`Rebuild::Skipped`) rather than replaced with nothing.
- **Failure handling.** A failed rebuild keeps the live transcript and clears `processed`, so
  "delete after processing" keeps the audio for a retry.
- **`has_speech` threshold.** Exactly the spec's rule (`(p10 × 2).max(0.006)`, ≥ 300 ms), without
  `pause_cut`'s median cap: with the cap, mic noise would count as speech. A perfectly steady
  signal (a test tone, a hum) counts as no speech.
- **Logging.** "The log" is stderr (`eprintln!`): the core has no logger.

### Measured

- **Live cost, §2 (real 10:38 call, read in place; MX230, large-v3-turbo + VAD, Balanced 8–30 s
  chunks, simulated real time, no summary swaps):**

  | | requests | Whisper busy | median delay | p90 | max | finished after the audio |
  |---|---|---|---|---|---|---|
  | mixed (before) | 45 | 595 s = 0.93× real time | 22.0 s | 57.9 s | 86 s | 12.5 s |
  | per track (now) | 73 | 767 s = **1.20×** | **33.4 s** | 71.0 s | 147 s | **142 s** |

  **The backlog is not bounded:** it grows by about 0.2 s per second of call. Chunks:
  33 both tracks, 5 call only, 2 mic only, 1 silent (the backlog pushed most chunks to 30 s). On
  this call even "before" was far from 010's 12.7 s, which came from a 75 s clip. Line counts per
  track: 31 You / 42 Call, against 19 / 78 in the live transcript the user recorded.
- **Known-Gap fallback not built.** The fallback is one request with both tracks joined by 1 s of
  silence. The measurement calls for it, but the next finding makes its boundary unreliable: VAD
  removes the 1 s gap like any other silence. Deferred (follow-up (a)).
- **New finding: VAD glues one speaker's turns into one line.** Each track is silent while the
  other person talks; whisper-server's VAD removes that silence, and Whisper writes several
  separate turns as one line. On synthetic pt-BR turn-taking audio (libespeak-ng, 10 turns,
  38.7 s), start times map back correctly and every label was right. But the after-call rebuild
  made a Call line from 1.0 to 26.9 s holding 4 call turns, and live made a You line holding 3
  turns. Interleaving inside such a line is lost. It is worse after the call (chunks up to 30 s)
  than live. Deferred (follow-up (b)).
- **Echo rule (synthetic, call at −20 dB in the mic):** all 6 echoes dropped, and every later You
  turn kept. But your first turn was glued to an echo of the greeting, and the whole line was
  dropped. Part of follow-up (b).
- **Timeline, §5:** two `pw-record` captures started with `capture.rs`'s arguments on an idle
  machine, audio discarded. The call's first sample came 84 ms after the mic's, and the gap stayed
  at 85 ms (±40 ms read jitter) for 5 minutes; both tracks kept pace with the wall clock. So: start
  lag, no drift. The padding fixes that. The earlier short meetings differ by 0–0.13 s, which fits.
- **Speaker cap:** unit-tested only (2 attendees → 1 voice even for embeddings at cosine 0.45 < 0.5).

### Not verified yet

- **Cause of the 2.2 s on the 10:38 call.** Not reproduced. The call's tracks share no sound
  (envelopes anti-correlated: the two people took turns, with no bleed), so start lag vs. dropped
  mic samples can't be told from that recording. If it shows up again, compare the logged
  "started N ms after" line with the final length difference: a match means start lag; a
  difference means drift or drops, which is out of scope here.
- **Any real session in `cargo tauri dev`:** live per-track lines and the backlog on a real call
  with summary swaps (which make the measured cost worse); the rebuild replacing the live text;
  record-only; a call without headphones; the speaker cap on a real 2-person invite; replay after
  padding. The spec's hand tests (with and without headphones, record-only) are still to do.
- **Real echo:** only synthetic echo (a −20 dB copy) was tested, not speakers into a room mic.

### Follow-ups (not created here: new specs go on `main`, and this build did not push to `main`)

- **(a) Live cost of per-track transcription.** Options: the joined request (checked against the
  gluing finding), per-track only when Whisper is idle and mixed under backlog, a smaller live
  model, or splitting by VAD segments. Measure with `bench-latency --mic … [--mixed]`.
  **Merge decision for this PR:** until (a) lands, the live transcript falls further behind on long
  calls on the MX230 Balanced layout. The after-call rebuild still gives the corrected final
  transcript.
- **(b) Per-track lines spanning several turns.** Cut each track at its own pauses (at least in
  the after-call rebuild, which has no real-time budget), or split lines at gaps on their own
  track. That would also stop real words from being dropped with a glued echo.

### Change division

There is no `CONTRIBUTING.md`, so this follows the handoff's conventions: the todo→doing rename
is its own commit, then one commit per concern (speech detection, merge/echo, live per-track,
after-call rebuild, Settings UI, speaker cap, timeline, bench tool, real-model test, eval fix),
green at every commit, and this close-out. No pre-PR or stack: nothing was a behaviour-neutral
enabling refactor big enough to split out (the `transcribe_samples` track parameter landed with
the rebuild that needs it). The project has no version-bump policy; none was made.
