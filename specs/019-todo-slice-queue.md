# 019 — Slice queue: each track cut at its own pauses, one Whisper worker

> **Reworked 2026-11-25.** This decides **how** live per-track audio (spec **018**) is cut and
> scheduled. 018 decides **what** live produces; the two are built together. The after-call
> transcript belongs to **020** (`pipeline/tracks.rs`), so this spec no longer touches it, and the
> original §6 ("rebuild through the queue") is gone. A first build (PR #25, branch
> `moacyrricardo/spec-019-slice-queue-build`, not merged) was dropped. Its measurements are kept
> below and shape the Decision.

## Context

Per-track live transcription needs one Whisper request per track that has speech. A request costs
about **8–10.5 s on the MX230, of which about 4.7 s is fixed**, so cost follows the **number of
requests**, not the minutes of audio. People take turns about 4.5 times a minute, and a short
"uhum" still costs a full request.

Measured on the real 10:38 two-person call (read in place, MX230, large-v3-turbo + VAD, simulated
real time, **no summary swaps**; all rows from one sitting except where noted):

| Live variant | Requests | Whisper busy | Median delay | Worst | Text done after audio | You / Call |
|---|---|---|---|---|---|---|
| Mixed (today) | 56 | **0.86×** | 10.8 s | 62.6 s | 9.3 s | guessed |
| Per track, both tracks cut at the same points (018 first build) | 80 | 1.03× | 31.1 s | 84.0 s | 30.8 s | 31 / 42 |
| Slice queue, Balanced, backlog merge at 20 s (spec defaults) | 78 | 1.06× | 23.1 s | 77.8 s | 44.9 s | 35 / 48 |
| **Slice queue, Balanced, backlog merge at 10 s** | 79–81 | **1.05–1.06×** | **17.2–17.5 s** | 44–48 s | 40–44 s | 33–34 / 47–52 |
| … merge gap 6 s | 72 | 1.15× | 20.6 s | 132 s | 100 s | 29 / 46 |
| … `min_s` 6 s | 76 | 1.04× | 17.7 s | 46.9 s | 30.4 s | 35 / 49 |
| … `min_s` 8 s | 73 | 1.01× | 18.2 s | 38.4 s | 25.1 s | 28 / 63 |
| Fast | 80 | 1.05× | 16.0 s | 41.9 s | 36.6 s | 37 / 55 |
| Light | 79 | 1.02× | 15.1 s | 51.9 s | 30.3 s | 32 / 59 |

The 10 s Balanced row was reproduced by an independent second run (1.06×, median 17.2 s).

What this showed:
- **The queue works.** Each track cut at its own pauses, earliest-ready-first, merging when behind:
  the **backlog stays bounded** (10–33 s, empties at the end), unlike shared cuts, whose backlog
  grew without bound. The median delay roughly halves against shared cuts.
- **It doesn't fit the MX230: 1.01–1.06×, never under 1.0×.** The queue sent about as many requests
  as shared cuts (79 vs 80), for two reasons:
  - the **call track's long turns are cut at every pause** once `min_s` is reached, so one long turn
    becomes several requests;
  - slices are merged **only once they're already queued** behind a backlog.
- **Longer merges cost more, not less**: a 6 s merge gap gave 1.15×, because the per-request cost
  also grows with the text.
- **`min_s` trades cost for label detail**: 8 s gave 1.01×, but more Call lines and fewer You lines,
  because short replies glue to the next slice.
- **The benchmark had no summary swaps.** In Balanced the live summary stops Whisper every 5 minutes,
  so the real cost is higher than these numbers.
- **The "hold back likely echo" rule misfired**: 14–21 mic slices held on a headphone call with no
  echo at all, and 2 lines dropped. Echo is out of scope (018, 020), so it isn't part of this spec.
- **Stop drain is wasted work**: after Stop the queue was worked through (30–44 s), then 020's
  after-call step transcribed everything again.

## Decision

1. **One slicer per track.** A slice opens when its track starts speaking and closes at that track's
   own pause (≥ 350 ms), between `min_s` and `max_s`, never mid-word (cut at the quietest point in
   the last 3 s at `max_s`). Silence is never sliced. Short speech under `min_s` is held for up to
   `hold_s` and joins the track's next slice.
2. **Speech detection reuses 020's.** The slicer uses the same span rules as
   `audio::speech::speech_spans`, as a streaming variant with a rolling ~30 s noise floor. It lives in
   `audio/speech.rs`, not as a second detector.
3. **Per-track `min_s`.** The call track's turns are longer, and cutting them at every pause was the
   main source of extra requests. The call track gets a higher `min_s` than the mic (starting point:
   call = 2 × mic). That keeps short "You" replies separate, which is what labels need, while sending
   fewer call requests.
4. **One queue, one Whisper worker, earliest-ready-first.** A slice is ready once it has closed.
   **Merge when behind**: when more than `backlog_s` = **10 s** of audio is queued, the worker joins
   the next slice with following queued slices **of the same track**, contiguous with gaps ≤ 3 s and
   ≤ 28 s in total. (10 s was measured better than 20 s: same Whisper time, much lower delay.)
5. **Packing like 020.** A merged request carries only the slices' speech, joined the way 020's
   `pack` joins spans (short joints, not the real gaps), mapped back with 020's `Window`/`Piece`
   logic. Long silent gaps don't count toward the 28 s.
6. **Lines are placed by start time**: in the live file readers, the live view (insert, don't append)
   and the live summary's input.
7. **Context**: each request's prompt is the last 200 characters of the live transcript before that
   slice's start, from both tracks.
8. **The live summary runs on schedule.** A due summary (automatic or "Summarize now") runs before
   the next request, not when the queue is empty. With Whisper near real time, "empty" can mean the
   end of the call.
9. **Recording pause** closes all open and held slices. Nothing joins across a pause.
10. **No stop drain when 020's step will run.** On Stop, if `transcript_after` is on (or the meeting
    is record-only), queued and open slices are discarded. 020's step transcribes the whole tracks
    anyway. With the step off, the queue is worked through as today.
11. **Acceptance (shared with 018 Decision 6):** on the 10:38 call at Balanced **with a summary swap
    every 5 min**:
    - **Whisper busy ≤ 1.0×**, and a bounded backlog that returns below 30 s at least every 5 min;
    - text done ≤ 30 s after the audio;
    - a median delay at most 10 s worse than mixed in the same sitting;
    - You lines ≥ mixed.

    Each configuration runs twice. **If it doesn't fit, 018 + 019 don't ship**; the result goes
    back to the user as a decision (see Known Gaps).

## Implementation

### 1. Slicer (`slicer.rs`, streaming spans in `audio/speech.rs`)

```rust
pub struct Slice { pub track: Track, pub start_s: f64, pub end_s: f64, pub samples: Vec<i16> }
pub struct SlicerConfig { pub min_s: f64, pub max_s: f64, pub hold_s: f64, pub pause_ms: u32 }
impl Slicer {
    pub fn new(track: Track, cfg: SlicerConfig) -> Self;
    pub fn push(&mut self, t_s: f64, samples: &[i16]) -> Vec<Slice>; // slices that became ready
    pub fn finish(&mut self) -> Vec<Slice>;                          // close open + held
}
```
- Profiles (015 "Live updates"; mic values; call `min_s` = 2 × mic, capped at `max_s / 2`):

  | Updates | mic `min_s` | `max_s` | `hold_s` |
  |---|---|---|---|
  | Fast | 3 | 12 | 8 |
  | Balanced | 4 | 20 | 15 |
  | Light | 6 | 28 | 25 |

  `Custom` maps its chunk length to `max_s`, with Balanced's `min_s` / `hold_s` capped at half and
  three quarters of it.
- Test that the streaming detector and `speech_spans` agree on the same complete buffer.
- Unit tests (synthetic): one 10 s turn → 1 slice; 3 turns with 1 s pauses → 3; a 40 s monologue at
  `max_s` 20 → 2–3 slices cut at quiet points; a 0.5 s "uhum" before a turn 5 s later → merged; a
  lone "uhum" → queued after `hold_s`; silence → none; `finish()` flushes; a pause closes everything.

### 2. Queue and worker (`queue.rs`, `session.rs`)

```rust
impl SliceQueue {
    pub fn push(&mut self, s: Slice);
    pub fn next(&mut self) -> Option<Request>; // earliest-start ready slice, merged per Decision 4–5
    pub fn queued_s(&self) -> f64;
}
```
- The worker sends one request at a time and maps lines back with 020's window mapping.
- Summary scheduling per Decision 8. Stop behaviour per Decision 10.
- `Status.backlog_s` = audio not yet sliced + queued + open + held (the field the UI already shows).
- Tests (Whisper as a closure): earliest-start-first; a later-finishing, earlier-starting call slice
  still lands in place; merging only above `backlog_s` and only within one track; merged gaps and
  total respected; mapping back across joints; a due summary runs before the next request.

### 3. Order (`store.rs`, UI)

- Every reader of the live transcript sorts by start (stable). Live lines are still appended
  (crash-safe).
- UI: a pure `insertByStart` helper in `app/src/lib/` with a test, used by the live view.
  `fake.ts` emits one out-of-order pair.

### 4. Benchmark (`fofoca-cli bench-latency`)

- `bench-latency <call.wav> --mic <mic.wav> --queue [--updates …] [--backlog S] [--min-s S]
  [--call-min-s S] [--mixed]`, **with a simulated summary swap every 5 min at Balanced** (the
  summary model's measured load and unload time on the MX230).
- Report the Decision 11 columns plus queue stats: slices per track, merged requests, and the
  backlog every minute. Two runs per configuration, mixed in the same sitting.
- Counts and timings only. No transcript text from real meetings anywhere.

### 5. Checks

`cargo test -p fofoca-core`, `cargo test -p fofoca`, `cargo clippy --workspace --all-targets` (no
warnings), and in `app/`: `npx svelte-check && npm test && npm run build`.

## Known Gaps

- **≤ 1.0× is not proven.** The best measured variant was 1.01× without summary swaps. Levers, in
  the order to try:
  1. per-track `min_s` (Decision 3, new and unmeasured);
  2. Whisper's `--audio-ctx` sized to short slices, which trades accuracy for a smaller encoder pass;
  3. a longer Balanced `min_s` (8 s gave 1.01× but fewer You lines).

  If none fit, the remaining choices are a smaller live model, or per-track only while Whisper is
  idle with mixed under backlog. Each is a decision for the user, not something to slip in.
- **Turns glued into one line** within a track (VAD drops the other person's turn) remain. The cuts
  reduce it; splitting lines at word-time gaps is a follow-up.
- **Lines appear out of order** for a few seconds before settling. That's intended.
- **Overlapping speech** gives two lines with close starts.
- **Echo**, **track alignment (021)** and **live speaker separation** are out of scope.
- The thresholds were tuned on one call; more calls may move them.
