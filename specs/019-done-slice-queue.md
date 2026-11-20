Status: done
Branch: moacyrricardo/spec-019-slice-queue-build

# 019 — Slice queue: each track cut at its own pauses, one Whisper worker

> Builds on **018** (PR #23) and **replaces its §2 (live chunks)**. Everything else in 018 stays:
> `has_speech`, echo removal, the after-call rebuild, the speaker cap, aligning the track starts.
> The after-call rebuild switches to this queue too (§6).

## Context

018 transcribes the mic and the call separately, which fixed the labels: on a real 10:38 call,
31 You / 42 Call lines, against 19 / 78 with the mixed path. But it cuts **both tracks at the same
points**, and that makes live too slow (018 Implementation Notes, MX230, large-v3-turbo + VAD,
Balanced):

| | requests | Whisper busy | median delay | max | text done after the audio |
|---|---|---|---|---|---|
| mixed (before 018) | 45 | 0.93× real time | 22.0 s | 86 s | 12.5 s |
| per track, shared cuts (018) | 73 | **1.20×** | 33.4 s | 147 s | **142 s** |

Why: people take turns about **4.5 times a minute**. So almost every shared chunk holds a bit of
both tracks, even if only a "uhum", and that means two requests. A Whisper request costs about the
same however short its audio, about **10.5 s** on the MX230 (767 s / 73 requests), because the
encoder always runs over a 30 s window.

On the same call each track speaks for well under the call's length: mic 162 s (25 %), call
424 s (67 %), both at once 38 s (6 %). If each track is cut **at its own pauses** and silence is
never sent, it comes to roughly the same number of requests as the mixed path, and the labels stay
exact.

018's fallback (both tracks in one request with a 1 s gap) was tested and fails: Whisper's VAD
removes the gap. It is not part of this spec.

## Decision

1. **One slicer per track.** Each slicer turns its track into **slices**:
   `{ track, start_s, end_s, samples }`.
   - A slice **opens** when the track starts speaking. Silence is never sliced.
   - It **closes** at that track's next pause (≥ 350 ms), but only after `min_s`. It is never longer
     than `max_s`: at `max_s` it is cut at the quietest point in the last 3 s, never mid-word.
2. **Short speech waits.** A closed slice shorter than `min_s` (a "uhum") is held, not queued. It
   joins the track's next slice if one starts within `hold_s`. Otherwise it is queued on its own
   once `hold_s` has passed since it closed.
3. **One queue, one worker.** Ready slices from both tracks go into one queue. The single Whisper
   worker always takes **the ready slice with the earliest `start_s`**. A slice is ready once it
   has closed.
4. **Merge when behind.** When the worker takes a slice and the queue holds more than `backlog_s`
   of audio, it joins that slice with the next queued slices **of the same track**. Those must be
   contiguous in time with gaps ≤ `merge_gap_s`, and the total stays ≤ `merge_max_s`. All of it
   goes in one request. With no backlog, small slices go out alone for low latency.
5. **Lines are placed by start time.** Lines arrive out of order (a long call turn can finish after
   your later reply). The transcript file, the live view and the summary input keep lines **sorted
   by `start`**, inserting rather than appending.
6. **Echo is held back.** A mic slice that overlaps a queued or open call slice in time while the
   mic is much quieter than the call (§4) is not sent until that call slice has been transcribed.
   018's `is_echo` then compares them with both texts known. This fixes the case 018 found, where
   one of your real turns was glued to an echo line and dropped with it.
7. **Context per slice.** Each request's prompt is the last 200 characters of the transcript
   **before that slice's `start_s`**, from both tracks.
8. **The after-call rebuild uses the same queue** (018 §4) with `max_s = merge_max_s` and no
   latency limits. It's the same code in a different mode.
9. **Acceptance.** On the same 10:38 call and the same benchmark as 018, live must have **Whisper
   busy < 1.0× real time** and a **bounded backlog**, with a median delay no worse than the mixed
   path (22 s). Labels must be at least as good as 018 (31 You / 42 Call, ±10 %).

## Implementation

### 1. Slicer (core, new `slicer.rs`)

```rust
pub struct Slice { pub track: Track, pub start_s: f64, pub end_s: f64, pub samples: Vec<i16> }
pub struct SlicerConfig { pub min_s: f64, pub max_s: f64, pub hold_s: f64, pub pause_ms: u32 }
pub struct Slicer { /* per-track state: open slice, held short slice, noise floor */ }
impl Slicer {
    pub fn new(track: Track, cfg: SlicerConfig) -> Self;
    /// Feed samples captured since the last call (t = track time of the first one).
    /// Returns slices that became ready.
    pub fn push(&mut self, t_s: f64, samples: &[i16]) -> Vec<Slice>;
    /// End of recording: close and return whatever is open or held.
    pub fn finish(&mut self) -> Vec<Slice>;
}
```
- Speech and pause detection use the **same relative threshold as `audio::has_speech`** (018):
  50 ms windows, noise floor = 10th percentile, quiet = `(floor × 2).max(0.006)`. The floor is
  tracked over a rolling ~30 s of that track, not per chunk.
- Cut points reuse `audio::pause_cut` and `audio::quiet_cut` on a **single track**. Pass the same
  buffer twice, or add single-track variants if that is cleaner.
- A slice keeps ~200 ms of audio before its first speech window and after its last one, so word
  edges aren't clipped.
- Defaults, mapped from 015's "Live updates" setting (`Updates` in `config.rs`):

  | Updates | min_s | max_s | hold_s |
  |---|---|---|---|
  | Fast | 3 | 12 | 8 |
  | Balanced | 4 | 20 | 15 |
  | Light | 6 | 28 | 25 |
  | rebuild (after the call) | 6 | 28 | 0 (no hold: merge short slices into neighbours) |

  `Custom` (config.toml) keeps its raw values: `summary_every_s` stays, and the chunk length maps to
  `max_s`.
- Unit tests (synthetic tones and noise): a 10 s turn → 1 slice; 3 turns separated by 1 s pauses
  → 3 slices; a 40 s monologue at `max_s = 20` → 2–3 slices cut at quiet points; a 0.5 s "uhum"
  followed by a turn 5 s later → merged into that turn; a lone "uhum" → queued after `hold_s`;
  silence only → no slices; `finish()` flushes open and held slices.

### 2. Queue and worker (core, new `queue.rs`; `session.rs`)

```rust
pub struct SliceQueue { /* ready slices from both tracks, plus "waiting on call slice" mic slices */ }
impl SliceQueue {
    pub fn push(&mut self, s: Slice);
    /// The next request: the earliest-start ready slice, merged with same-track neighbours when
    /// the queued audio exceeds backlog_s (Decision 4). None if nothing is ready.
    pub fn next(&mut self, backlog_s: f64, merge_gap_s: f64, merge_max_s: f64) -> Option<Request>;
    pub fn queued_s(&self) -> f64;
}
pub struct Request { pub track: Track, pub start_s: f64, pub samples: Vec<i16> /* gaps filled with silence */ }
```
- Defaults: `backlog_s = 20`, `merge_gap_s = 3`, `merge_max_s = 28`.
- `session.rs`: the capture ticker feeds both slicers. The worker loop (today's
  `transcribe_chunk` caller) asks `next()`, sends **one** Whisper request, and turns the result into
  lines with 018's `tracks::segment(raw, track, start_s)`. Then the echo check and the insert by
  start (§3).
- 018's `tracks::with_speech` / `transcribe_parts` for live are replaced by this. 018's per-chunk
  `pause_cut` on both tracks goes away for live.
- **015 layouts and swapping don't change:** the worker is still the only Whisper user. The
  automatic summary (Balanced, every 5 min) and a manual "Summarize now" are scheduled as today. In
  the swap layout the worker pauses while the LLM has the GPU, and slices keep queuing.
- `Status` (the ticker event) reports `backlog_s = queue.queued_s()` plus the open slices' length.
  It's the same field the UI already shows.
- Unit tests (fake Whisper as a closure, like 018's `transcribe_parts`):
  - earliest-start-first;
  - a later-finishing earlier-starting call slice is still emitted at its place;
  - merging triggers only above `backlog_s` and only for the same track;
  - merged gaps ≤ `merge_gap_s`, total ≤ `merge_max_s`;
  - the merged request's line times map back correctly.

### 3. Lines placed by start (core `store.rs`, UI)

- `Store`: live lines are still appended to `transcript.jsonl` as they arrive (crash-safe). Every
  reader sorts by `start`: `load`, `live_transcript`, `base_transcript` and the summary input.
  Sorting is stable, so equal starts keep arrival order.
- `SessionEvent::Segment` is unchanged. The UI (`lib/live.svelte.ts`) inserts each incoming line at
  its sorted position. Add a pure helper in `app/src/lib/` with a test (`insertByStart`). The live
  view's auto-scroll stays pinned to the bottom unless the user scrolled up, as today.
- `fake.ts`: the fake live session emits an out-of-order pair (the call line after your later
  reply) so the browser preview shows the insert.

### 4. Echo held back (core `queue.rs`)

- A mic slice is **held** when an open or queued call slice overlaps it by ≥ 50 % of its length
  **and** the mic's RMS over the overlap is < 0.25× the call's RMS. From 018's measurement, your
  voice is 30–800× louder than the call when you speak, and an echo is quieter than the call.
- A held mic slice becomes ready once every overlapping call slice has produced its lines. Then
  018's `is_echo` runs per line with the call lines known.
- **Don't drop whole slices:** `is_echo` drops lines, not slices. That is how a real turn glued to
  an echo survives. If Whisper returns one line that holds both, split it at word timings where
  possible. If it can't be split, keep it: losing your words is worse than showing an echo.
- Tests: an echo slice is held, then dropped; a real reply overlapping a quiet call is not held;
  a turn glued to an echo keeps the turn.

### 5. Context (core `session.rs`)

- `prompt_before(start_s)`: the last 200 characters of the sorted transcript with `start < start_s`.
  It replaces 018's tail-of-everything for live.

### 6. After-call rebuild (core `pipeline.rs`)

- `retranscribe` runs both tracks' whole WAVs through the slicer in rebuild mode (§1 table) and the
  same queue. Without a time limit, `next()` merges up to `merge_max_s` whenever possible (it treats
  the backlog as always exceeded). Results feed 018's `store.rebuild_transcript`.
- Imports (one track) go through the same path with one slicer.

### 7. Measurement (CLI)

- Extend 018's `fofoca-cli bench-latency <call.wav> --mic <mic.wav>` with `--queue`, which uses the
  slicers and queue under the same simulated real time. Report the same columns as 018 plus queue
  stats: slices per track, merged requests, held echo slices, and the max backlog over time.
- Run on the **same 10:38 call (read in place, never copied)**, Balanced. Report mixed / 018 /
  019 in one table in the Implementation Notes. Also run Fast and Light once each.
- Report counts and timings only. **Never put transcript text from a real meeting in commits, PR
  text or specs**; use invented lines in tests and examples.

### 8. Checks

`cargo test -p fofoca-core`, `cargo test -p fofoca`, `cargo clippy --workspace --all-targets`
(no warnings), and in `app/`: `npx svelte-check`, `npm test`, `npm run build`. The new TS helper
has a test in `src/lib/*.test.ts`.

## Known Gaps

- **Short turns still cost a full request.** With ~10.5 s per request and 4.5 turns a minute, the
  margin under 1.0× depends on holding and merging. If the benchmark lands above 1.0×, the next
  lever is Whisper's `--audio-ctx` (encoder sized to the slice) for short slices. It trades
  accuracy and isn't in this spec.
- **Lines appear out of order** for a few seconds before settling. That's intended.
- **Overlapping speech** (both talking, 6 % of the measured call) still gives two lines with close
  starts.
- **Clock drift** between tracks: 018 measured none over 5 minutes (a constant 85 ms start lag, now
  padded). The 2.2 s gap on the real call is still unexplained.
- **No acoustic echo cancellation** (PipeWire `echo-cancel`). The rule is level and text based.
- The thresholds (`min_s`, `hold_s`, `backlog_s`, the 0.25× echo ratio) are first guesses tuned on
  one call. Hand tests on more calls may move them.

## Implementation Notes

Built on `moacyrricardo/spec-019-slice-queue-build` (PR #25, stacked on the spec PR #24). The
Decision is in place for live, the after-call rebuild and imports. **Decision 9's headline number
is not met:** Whisper stays at 1.02–1.06× real time on the MX230 (measured below).

### Where it landed

- `slicer.rs` (§1): `Slice`, `SlicerConfig::{live, rebuild, for_config}`, `Slicer::{push, finish,
  pending_from, pending_s}`. `audio::quiet_level` is `has_speech`'s threshold, now shared.
- `queue.rs` (§2, §4): `SliceQueue::{push, next, done, call_audio, set_call_open, queued_s}`,
  `Request`, `QueueStats`, `BACKLOG_S` / `MERGE_GAP_S` / `MERGE_MAX_S` / `ALWAYS_MERGE`, and
  `queue::transcribe(req, lines, whisper)`, which takes Whisper as a closure.
- `tracks.rs` (§3–§5): `insert_by_start`, `prompt_before`, `drop_echoes` (with `strip_echo` for a
  turn glued to an echo). 018's `with_speech` / `transcribe_parts` are gone from the core.
- `session.rs`: the capture loop feeds both slicers (call first) and the queue, and the worker
  sends one request at a time. `Status.backlog_s` = audio not yet taken + queued + open + held.
- `store.rs`: every transcript reader sorts by start (stable). Live lines are still appended.
- `pipeline.rs` (§6): `transcribe_tracks` → `slice_and_transcribe` (Whisper as a closure) for the
  rebuild and imports.
- UI: `lib/transcript.ts` `insertByStart` (+ test), used by `App.svelte`; `fake.ts` sends the
  00:32 call line after the 00:40 reply.
- `fofoca-cli bench-latency <call.wav> --mic <mic.wav> --queue [--updates fast|balanced|light]
  [--backlog S] [--merge-gap S] [--min-s S]`. 018's per-track mode stays in the CLI for
  comparison.

### Differences from the spec

- **`backlog_s` = 10 s, not 20 s.** One informed adjustment, measured (table below): about the same
  Whisper time, much lower delay.
- **Pause detection** is the slicer's own scan of 50 ms windows against a rolling ~30 s floor
  (`audio::quiet_level`). It doesn't call `audio::pause_cut`, whose floor is per buffer; the spec
  asks for a rolling one. `audio::quiet_cut` is reused for the `max_s` cut on a single track.
- **`prompt_before` is in `tracks.rs`**, not `session.rs`: the queue, the rebuild and the bench
  share it.
- **Custom (config.toml):** the chunk length is `max_s` as specified; `min_s` and `hold_s` are
  Balanced's 4 s / 15 s, capped at half and three quarters of `max_s`.
- **A pause in the recording** closes all open and held slices, so nothing is joined across it.
- **Echo split** uses word times interpolated over the line (`approx_words`, as the transcript has
  no measured word times). Only a run of ≥ 3 repeated words at the start or end is cut, with 1 s
  of slack around the call line; otherwise the line stays whole.
- **Summary scheduling (eval fix):** a due summary (automatic or "Summarize now") runs before the
  next request. Before the fix it waited for an empty queue, which with Whisper near real time
  can mean the end of the call.
- **Auto-scroll** in the live view is unchanged (it always follows the bottom). The spec's
  "unless the user scrolled up, as today" describes behaviour that doesn't exist today, and adding
  it wasn't part of this spec.
- **The stop drain stays:** after Stop, the live queue is worked through before the rebuild, which
  then transcribes everything again (as 018 did).

### Measured (§7, Decision 9)

Real 10:38 call, read in place; MX230, large-v3-turbo + VAD; simulated real time, no summary
swaps. Mixed and 018 were re-run on the same day and machine with the 018 code (018's own run
had measured 73 requests / 1.20×; this machine ran faster today, so only these rows compare):

| | requests | Whisper busy | median delay | p90 | max | done after audio | You / Call lines |
|---|---|---|---|---|---|---|---|
| mixed (before 018) | 56 | 547 s = **0.86×** | 10.8 s | 37.0 s | 62.6 s | 9.3 s | — (one track) |
| 018 per track, shared cuts | 80 | 659 s = 1.03× | 31.1 s | 60.0 s | 84.0 s | 30.8 s | 31 / 42 (018 notes) |
| **019 Balanced (shipped: backlog 10 s)** | 79 | 672 s = **1.05×** | **17.5 s** | 36.0 s | 44.0 s | 39.6 s | 33 / 52 |
| 019 Balanced, spec defaults (backlog 20 s) | 78 | 678 s = 1.06× | 23.1 s | 45.0 s | 77.8 s | 44.9 s | 35 / 48 |
| 019 Balanced, backlog 10, merge gap 6 s | 72 | 733 s = 1.15× | 20.6 s | 111.4 s | 132.0 s | 100.4 s | 29 / 46 |
| 019 Balanced, backlog 10, `min_s` 6 | 76 | 661 s = 1.04× | 17.7 s | 35.7 s | 46.9 s | 30.4 s | 35 / 49 |
| 019 Balanced, backlog 10, `min_s` 8 | 73 | 646 s = 1.01× | 18.2 s | 32.7 s | 38.4 s | 25.1 s | 28 / 63 |
| 019 Fast (backlog 10) | 80 | 669 s = 1.05× | 16.0 s | 31.7 s | 41.9 s | 36.6 s | 37 / 55 |
| 019 Light (backlog 10) | 79 | 654 s = 1.02× | 15.1 s | 32.2 s | 51.9 s | 30.3 s | 32 / 59 |

Queue stats for the shipped Balanced run: 53 mic / 71 call slices, 31 merged requests, 5 requests
returned no lines (all mic), 19 mic slices held as likely echo, 2 mic lines dropped or cut as echo.

- **Whisper busy < 1.0×: not met** (1.02–1.06×, the same as 018 today).
- **Bounded backlog: met.** Merging holds it at ~10–25 s through the call (per-minute samples
  9–44 s, peak 56 s), with no upward trend.
- **Median delay ≤ mixed (22 s in 018's run): met** at 17.5 s, but **not** against today's mixed
  run (10.8 s).
- **Labels (31 / 42 ± 10 %):** You 33 (+6 %) is in range; Call 52 (+24 %) is not. These are more
  lines: separate call turns are no longer glued into one line. The labels themselves come from
  the track, as in 018.
- **Why not < 1.0×:** a request does not cost the same however short its audio. Fitting mixed
  against 018, each request has about 4.7 s of fixed cost and the rest grows with the text, so
  long merged requests (gap 6 s) cost more. 019 sends about as many requests as 018 (79 vs 80):
  the call track's long turns are cut at every pause after `min_s`, and slices are merged only
  when they are already queued. Tuning stopped after these adjustments, as asked.
- Echo: unit-tested with a synthetic −20 dB copy. On the real call (which has no bleed) the hold
  fired 19 times and dropped 2 lines, so on this call it mostly delays mic slices rather than
  catching echoes.

### Not verified yet

- **Any real session in `cargo tauri dev`:** live slices and the backlog on a real call *with*
  summary swaps (the benchmark has none); the summary now running while slices queue; out-of-order
  lines in the real live view (only the browser preview with `fake.ts` was checked); pause/resume
  closing slices; the rebuild and an import through the queue on real audio (the ignored real-model
  test `per_track_rebuild_on_a_real_recording` was not re-run).
- **Echo hold on a call without headphones** (only synthetic echo was tested).
- **Speaker separation after the rebuild:** call lines are now shorter (one turn each). That
  should help `speakers::assign`, but it wasn't measured.
- Run-to-run variance of the benchmark: each configuration was run once.

### Follow-ups (not created here: new specs go on `main`, and this build did not touch `main`)

- **(a) Whisper time under 1.0×.** Options measured or left open: `--audio-ctx` sized to short
  slices (Known Gaps), a longer Balanced `min_s` (8 s gave 1.01× but more Call lines), or mixed
  requests while behind. Measure with `bench-latency --queue --min-s … --backlog …`.
- **(b) Echo hold threshold.** It fires on mic slices that aren't textual echoes; tune it on a call
  without headphones.
- **(c) Stop drain vs rebuild.** When `transcript_after` is on, finishing the live queue after Stop
  is mostly thrown away by the rebuild; skipping it would shorten Processing.

### Change division

There is no `CONTRIBUTING.md`; this follows the handoff's conventions. The todo→doing rename is its
own commit, then there is one commit per concern: slicer, queue/worker, place by start (core + UI
helper + `fake.ts`), echo hold-back, `prompt_before`, rebuild/imports via the queue, bench
`--queue`, then the measured `backlog_s` change and the two eval fixes (summary scheduling, the
rebuild `max_s` constant). Every commit passed all checks. There's no pre-PR: the only enabling
refactor (`audio::quiet_level`, 10 lines) rides with the slicer that needs it. The project has no
version-bump policy; none was made.
