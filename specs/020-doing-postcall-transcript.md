Status: doing
Branch: moacyrricardo/spec-020-postcall-transcript-build

# 020 — After-call per-track transcript

## Context

The live path (001/010) mixes the mic and call tracks into one stream and labels each Whisper
segment by whichever track is louder over that segment (`audio::attribute`). Whisper's segments
often cross a change of speaker, so a whole segment goes to one side. On a real two-person pt-BR
call (10:38), 78 lines were labelled Call and 19 You while the two people were taking turns.

The audio itself is clean. When the user speaks, the mic is 30–800× louder than the call, and the
user's voice isn't in `system.wav`. Each track contains only its own side's speech (headphones
assumed, see Known Gaps). So transcribing the two tracks **separately** makes You/Call correct by
construction: every line comes from the track it was spoken on.

This spec changes only the **saved** transcript, which is produced once the call ends. Live
transcription stays exactly as it is on `main`. On that call there were 162 s of speech on the mic
track, 424 s on the call track, and 38 s where both overlapped. A Whisper request costs about
8–10.5 s on the MX230, roughly the same however short its audio. So the step sends only the speech,
packed into full windows, instead of every second of both tracks.

Related: 003 (speakers run on the call track and split Call lines; its `max_speakers` from the
calendar invite is fixed separately), 005 (notes cite transcript times, ±2 s), 011 (audio
retention), 012 (imports), 015 (after-call steps, layouts, record-only), 018 (todo, per-track
transcription; this spec leaves the live path alone).

## Decision

1. **New after-call step: "Transcript from each track".** When a recording ends, `mic.wav` (→ You)
   and `system.wav` (→ Call) are transcribed separately. Only each track's speech is sent, packed
   into windows of up to 30 s. The resulting lines are mapped back to recording time and merged by
   start time. That merged result becomes the saved transcript, and speakers (003) and notes (005)
   run after it.
2. **Order:** after the live backlog drains: transcript per track → speakers (call track only, 003)
   → checked notes (005) → storage policy (011).
3. **Echo (no headphones): not handled.** Without headphones, the call's words can also appear as
   You lines. Removing them is left to a later spec.
4. **Speaker cap from the calendar invite: not in scope.** It's 003's `max_speakers`, fixed
   separately. Mentioned here only because it's related.
5. **Aligning the two tracks' start: not in scope.** The tracks are joined by their own timelines
   as recorded. A separate spec measures and fixes capture alignment.
6. **On by default, toggleable.** It's listed in Settings → Processing → *After the call* with
   speakers and notes (`[workflow] transcript_after = true`). Turning it off keeps the live
   transcript as the saved one. Record-only (015) always runs it, shown as a fixed, checked row,
   because it's that mode's only transcription.
7. **No time budget.** This spec is a step toward the live work. It has no speed gate and no faster
   path. The step is timed once on the real 10:38 call, and the result goes in Implementation Notes.
   (This replaces the 0.5× budget in the original brief.)
8. **The live transcript is never lost, but never shown either (D1 · File + CLI only).** The
   per-track result replaces the saved transcript (`transcript.jsonl`). The live transcript is kept
   in `transcript.live.jsonl`, never overwritten and never shown in the app.
   `fofoca-cli transcript <id> show|use live|tracks` is the only way to see it or restore it. If the
   step fails, produces nothing, or can't run (audio gone, a track missing on a recording), the live
   transcript stays the saved one.
9. **Re-run from the meeting page (D2 · Button, re-runs whole chain).** *Transcribe again from the
   audio* on the meeting page queues `Job::Retranscribe`. It re-runs the per-track transcript, then
   speakers and notes as enabled in Settings, then retention. Existing notes are replaced, so they
   always cite the transcript they were written from. It's allowed for any meeting whose audio
   still exists, including meetings this step never processed: the step was off, the meeting was
   recorded before 020, or the step failed. The button is disabled, with the reason, once the audio
   is deleted. A CLI entry runs the same chain.
10. **Imports use the same path (D3 · Imports use speech packing).** An import is one Call track
    through the same function. `pipeline::transcribe_samples` is removed. Imports always run it
    (it's their only transcription), whatever the toggle says.

## Implementation

### Files on disk (per meeting)

| File | Holds | Written by |
|---|---|---|
| `transcript.jsonl` | the **saved** transcript: shown, read by notes, split by speakers | live session, this step, speakers, `transcript use` |
| `transcript.live.jsonl` | the live transcript, **write-once**: copied from `transcript.jsonl` before it's first replaced, and never overwritten afterwards | `Store::keep_live` |
| `transcript.tracks.jsonl` **(new)** | the latest per-track result, before speaker splitting; overwritten on re-run | this step |

- `Store::keep_live(id)` (new): if `transcript.jsonl` exists and neither `transcript.live.jsonl`
  nor `transcript.tracks.jsonl` exists, it copies `transcript.jsonl` to `transcript.live.jsonl`.
  `replace_transcript` calls it instead of doing its own copy. With this rule, a record-only
  meeting, whose first `transcript.jsonl` is already the per-track result, never gets a fake "live"
  file.
- `Store::save_tracks(id, &[Segment])` (new): writes `transcript.tracks.jsonl` atomically
  (temp + rename, like `replace_transcript`).
- `MeetingMeta.transcript_source: Option<TranscriptSource>` (new, `live` | `tracks`, omitted =
  "the newest one"). `Store::base_transcript(id)` (new) is what speakers start from: if the source
  is `live`, the live transcript. Otherwise `transcript.tracks.jsonl` when it exists. Otherwise
  today's `live_transcript(id)`. `pipeline::separate_speakers` switches from `live_transcript` to
  `base_transcript`. `live_transcript` stays, unchanged, for the CLI.

### Speech detection and packing (`audio.rs`, pure, unit-tested)

- `speech_spans(samples) -> Vec<Span { start, end }>` (sample indices). Frames are 100 ms (the
  `FRAME` already used by `quiet_cut`). The noise floor is the 10th percentile of frame RMS. A frame
  counts as speech when its RMS is ≥ max(4 × floor, 0.003), and 0.003 is the existing "near-silent"
  threshold. Spans are padded by 0.3 s on each side, gaps < 0.6 s are merged, and spans < 0.2 s are
  dropped. All constants live in one `SpeechOpts` with `Default`, so tests and later tuning don't
  touch call sites.
- `pack(spans, samples, max_s = 30.0, joint_s = 1.0) -> Vec<Window>`. Spans are added greedily, in
  order, separated by `joint_s` of zeros, while the window stays ≤ `max_s`. A span longer than
  `max_s - 1` is first split with `quiet_cut` (search 3 s), as the live path does.
  `Window { audio: Vec<i16>, pieces: Vec<Piece { src: usize, dst: usize, len: usize }> }`.
- `Window::to_source(t_window_s) -> f64` maps a time in the window back to recording time. Inside a
  piece, it adds `src - dst`. Inside a joint, it snaps to the nearer piece edge.

### Transcribing one track (`pipeline.rs`)

`transcribe_track(client: &WhisperClient, track: Track, samples: &[i16], cfg, hooks) ->
Result<Vec<Segment>>`:

- For each window: call `hooks.yield_gpu` (the caller stops and restarts the server, as
  `transcribe_samples` did), then `progress("tracks", windows_done / windows)`, then
  `client.transcribe(wav_bytes(window.audio), language, prompt)`. Requests keep the `vtt` format,
  so word timings are interpolated by `approx_words`, as today. The prompt is the last 200
  characters of **this track's** text so far.
- Each returned segment's words are mapped with `to_source`. If consecutive words land in
  different pieces, the segment is **split there** (text re-joined from words, as
  `speakers::assign` does), so a line never spans skipped silence. `start`/`end` are the first and
  last word's mapped times (segments without words map `start`/`end` directly). Every segment gets
  `track` and `speaker: None`.
- `merge_tracks(you, call) -> Vec<Segment>` (pure) sorts by `start`, then `end`, then You before
  Call. Overlapping lines are both kept.

### The step (`pipeline::transcribe_tracks`)

`transcribe_tracks(store, cfg, id, asr: Option<&WhisperClient>, hooks) -> Result<StepResult>`.
`StepResult::Done` gains `you`/`call` line counts and `took_s`. Speakers keep their own fields,
which can be done as a new `Done` variant per step or a struct; the builder picks one.

1. Read `mic.wav` and `system.wav`, each if it exists.
   - Neither exists → `Skipped("the audio is no longer stored — the live transcript is kept")`.
   - **Import** (`meta.imported_from` set): only `system.wav` is expected, giving one Call track.
   - **Recording** with one track missing → `Skipped("only one track is stored — the live
     transcript is kept")`. Exception: record-only has no live transcript, so it transcribes the
     track it has.
2. Use `asr` when given (the session's live Whisper server, so there's no reload). Otherwise start
   `ModelServer::start(Kind::Whisper, cfg, cfg.live_devices().0)`, and stop it at the end. Run
   `transcribe_track` for each present track, then `merge_tracks`.
3. If the result is empty and the current `transcript.jsonl` isn't →
   `Skipped("transcribing each track found no speech — the live transcript is kept")`. Nothing is
   written.
4. Otherwise: `keep_live`, `save_tracks`, `replace_transcript` (it calls `keep_live` too), set
   `meta.transcript_source = None`, then `mark_done("tracks")`.
5. Any error before step 4 leaves every file untouched. The caller reports it, and the audio is
   kept because `processed` stays false.

### Live session (`session.rs`)

- After the backlog drains and the WAVs are finished, if `!live || cfg.workflow.transcript_after`:
  emit `Step { step: "tracks" }` and call `transcribe_tracks`, passing the live client when one is
  loaded. On `Done`, reload `self.segments` from the store. On `Skipped`, show a notice and set
  `post_ok = false`. On `Err`, show an error ("the live transcript is kept"), set
  `post_ok = false`, and continue to speakers/notes on the live transcript. After this, Whisper is
  freed, as today.
- Record-only no longer transcribes a mix: `transcribe_recording` is removed and this step replaces
  it. If it fails, the meeting has no transcript, notes are skipped (too few words), and the audio
  is kept for a re-run.
- `processed` (011) = every step that ran succeeded, now including `tracks` when it ran.

### Imports (`pipeline::run_import`, `jobs.rs`)

- `run_import` converts the file, then calls `transcribe_tracks` (one Call track, server started
  with the `Hooks` yield logic, progress still reported as `transcribing` so the import card keeps
  its percentage), then speakers/notes/retention as today. `transcribe_samples` is deleted.
- `fofoca-cli eval-notes` moves to `transcribe_track(Track::Call, …)` on its WAV. Its numbers are
  no longer comparable with 005's earlier runs, and Implementation Notes should say so.

### Re-run (D2)

- `jobs.rs`: `Job::Retranscribe { id }`, on the same worker and FIFO as imports, honouring
  `pause_for_recording`. It runs `transcribe_tracks` (it isn't gated by `transcript_after`: an
  explicit request always runs). Then, as in `run_import`: `separate_speakers` if `speakers_after`,
  `write_notes` if `notes_after` and words ≥ `MIN_WORDS_TO_SUMMARIZE`, then `processed` = all ran
  OK, then `retention::apply` for that meeting. A `Skipped` transcript step stops the chain and
  reports the reason. `JobEvent` steps: `tracks`, `speakers`, `notes`, `done` | `failed`.
- Tauri `retranscribe(id)` is refused while that meeting is recording, when `audio_deleted_at` is
  set, or when neither WAV exists. Otherwise it queues the job and returns at once.
- UI (`MeetingView.svelte`, Transcript tab): a **Transcribe again from the audio** button (ghost,
  small, like *Separate again*). It's disabled with `title="The audio was deleted"` once the audio
  is gone, and disabled while a job for this meeting is queued or running. Progress uses the
  existing job card. `api.ts` `JOB_STEPS` gains `tracks: "Transcribing each track"`. The fake
  backend mirrors the command and job events.
- CLI: `fofoca-cli retranscribe <id>` runs the same chain in-process (no queue). It prints the
  You/Call line counts, the step's time, and the speakers/notes outcome, **never transcript text**.

### Recovering the live transcript (D1, CLI only)

`fofoca-cli transcript <id> show live|tracks|saved` prints each line as
`[mm:ss–mm:ss] You|Call|<speaker>: text`. A trailing `--counts` prints only the per-track line
counts and the total duration, with no text; use that when working with real meetings.

`fofoca-cli transcript <id> use live|tracks`:
- sets `meta.transcript_source`, then `replace_transcript` with that file (it errors if the file
  doesn't exist, e.g. `use live` on a record-only meeting);
- then runs speakers (if `speakers_after` and `system.wav` exists) and notes (if `notes_after`), so
  speakers and notes match the restored transcript;
- never deletes `transcript.live.jsonl` or `transcript.tracks.jsonl`.

### Settings (`config.rs`, `ProcessingTab.svelte`, `workflow.ts`)

- `Workflow.transcript_after: bool`, serde default `true` (older config files read as on), saved
  by `save_workflow`.
- *After the call* gets a first row: **Transcript from each track** — "transcribes your mic and the
  call separately, so every line is on the right side". The chip shows the Whisper device
  (`live_device`, or the layout's transcript device). In record-only, that row is the fixed,
  checked, disabled one, replacing today's "Transcribe the recording" row.
- `afterSteps(w)` puts "Transcript per track" first when `!w.live_transcript || w.transcript_after`.
  The line under the steps becomes: "Imports follow the same steps; they're always transcribed
  this way."
- `Processing.svelte` (the wrap-up screen): a new `tracks` step, "Transcribe each track · mic and
  call separately", chip = device, shown only when it runs. `types.ts` adds `"tracks"` to the
  `step` event union.

### Tests

Rust unit tests:

- `speech_spans`: silence + tone bursts at known times, a noise floor under the bursts, gap
  merging, tiny-span dropping, an all-silent track → no spans.
- `pack`: windows ≤ 30 s, joints of 1 s zeros, a 70 s span split at its quietest point, and a
  piece table that round-trips.
- `to_source` / segment splitting: a time inside a piece, a time in a joint snapping to the nearer
  edge, a segment whose words cross a joint split into two lines, a segment with no words.
- `merge_tracks`: ordering, ties (You first), and overlapping lines kept.
- `transcribe_tracks` against a tiny local HTTP server answering `/inference` with canned VTT (the
  same pattern as the downloader tests), covering:
  - both tracks → merged lines, and live/tracks/saved files as in the table;
  - a recording missing `mic.wav` → `Skipped`, files untouched;
  - both missing → `Skipped`;
  - an empty result → `Skipped`, live kept;
  - record-only (no `transcript.jsonl`) → no `transcript.live.jsonl` created, even after a later
    `separate_speakers`-style `replace_transcript`;
  - an import (`imported_from`, `system.wav` only) → Call lines;
  - a server error → `Err` and no file changed.
- `base_transcript` / `transcript_source`; `transcript use live` then `use tracks` round trip;
  `keep_live` is write-once.
- Config: `transcript_after` defaults to on, an older file reads as on, and saving keeps comments.
- Jobs: `Retranscribe` order and pause-for-recording (fake job, as the import tests do).

TS (`npm test`): `afterSteps` with the toggle on/off and record-only.

Existing tests that used `transcribe_samples` move to `transcribe_track`.

### Acceptance (functional; counts only)

The user names the meeting when the test runs. Real meeting data is **read in place** from
`~/.local/share/fofoca/meetings/` and **never copied**. Only counts and times are reported. No
transcript text, names, meeting ids or local paths go into commits, specs, PR text or tests, and
any examples use invented lines.

1. **Turn changes on the real 10:38 call:** `fofoca-cli retranscribe <id>` (the live transcript is
   kept, and `transcript use live` restores it). Then run a scratch script outside the repo that
   reads the two WAVs and `transcript.tracks.jsonl` in place. A line is "at a turn change" when the
   other track has a line starting within 2 s of its start. For each such line, the script compares
   the per-second RMS of mic vs system over the line's span. Report: lines at turn changes, how many
   sit on the louder track, how many fall in overlaps (both tracks loud), plus You/Call totals
   before and after. Expected: every non-overlap turn-change line is on its louder track, and any
   exception is explained.
2. **Time:** the step's time on that call on the MX230 (from `retranscribe`), reported in
   Implementation Notes. This is not a gate.
3. **Missing audio:** in a scratch meetings folder (the 75 s pt-BR test recording as both tracks, or
   generated tones), with `mic.wav` removed, `system.wav` removed, and both removed, show the step's
   outcome and that the saved and live transcripts are unchanged. Then set `audio_deleted_at`: the
   Tauri command refuses, and the button is disabled (browser preview).
4. **Re-run (D2):** on a scratch meeting, `retranscribe` re-writes the tracks file, speakers and
   notes, and `transcript.live.jsonl` is byte-identical before and after. In the browser preview,
   the button queues a job, the card shows progress, and the button is disabled while the job runs.
5. **Import (D3):** `fofoca-cli import` of the test recording goes through `transcribe_track`,
   leaving Call lines, `steps_done` containing `tracks`, and speakers + notes written.
6. All checks pass (`cargo test -p fofoca-core`, `cargo clippy --workspace --all-targets`,
   `svelte-check`, `npm test`, `npm run build`).

## Known Gaps

- **Echo:** without headphones, the call's words can also appear as You lines, because the mic
  hears the speakers. A later spec can add removal.
- **Track alignment:** the tracks are joined by their own timelines as recorded. An 85 ms start lag
  has been measured, and one call had an unexplained 2.2 s length difference. Until a separate spec
  measures and fixes capture alignment, a large start lag can misorder lines near turn changes.
- **No speed target:** the step is timed once (Decision 7). CPU-only layouts will be much slower
  and aren't measured.
- **The live transcript isn't visible in the app** (Decision 8); seeing or restoring it needs the
  CLI.
- **Word timings stay interpolated** (`vtt`), so a split at a joint or a speaker split (003) can be
  off by a word or two.
- **The energy speech detector** can pass steady noise on a noisy mic. Whisper's server-side VAD
  (when configured) and the noise-floor threshold are the only guards, and hallucinated You lines
  are possible in that case.
- **Overlapping speech** keeps both lines, ordered by start. There's no interleaving inside a line.
- **Speaker cap** from the calendar invite (003 `max_speakers`) is a separate fix.
- **Live transcription** is unchanged; per-track live work is 018.
