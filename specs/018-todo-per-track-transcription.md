# 018 — Per-track transcription during the call

> **Reworked 2026-11-25.** The first version of this spec covered live **and** after-call
> per-track transcription. The after-call part shipped as **020** (`transcript_after`,
> `pipeline/tracks.rs`), so this spec now covers **live only**. The first build (PR #23, not merged,
> branch `moacyrricardo/spec-018-per-track-transcription`) and the slice-queue build that followed
> (019, PR #25) were dropped. What they measured is kept below as context, and decides how this
> spec should be built. **How live audio is cut and scheduled is spec 019.** This spec decides
> *what* live produces; 019 decides *how*.

## Context

**Live is still mixed.** During a call, `Session::transcribe_chunk` (`session.rs`) sends
`audio::mix(mic, sys)` to Whisper. Each returned segment is then labelled You or Call by
`audio::attribute`: the louder track over the segment's time range wins, with the mic needing
≥ 1.2× the call. Whisper cuts at sentences, not at speaker changes, so a line often holds both
people, and the longer or louder speaker takes it. On a real 2-person, 10:38 pt-BR call the live
transcript had **19 You / 78 Call** lines while the two took turns, although the audio is clean (the
mic is 30–800× louder than the call when the user speaks).

**After the call this is already fixed (020).** The saved transcript is rebuilt from each track
separately: speech spans per track (`audio/speech.rs`: `speech_spans`), packed into windows of up
to 30 s (`pack`), transcribed per track (`pipeline/tracks.rs`: `transcribe_track`), and merged by
start (`merge_tracks`). On the same call that took **376.9 s (0.59× the call)** on the MX230 and
gave **67 You / 94 Call** lines. The live transcript is kept as `transcript.live.jsonl` and isn't
shown.

So what's left wrong is **what the user sees while the call is on**: the Record screen's lines, the
live summary (015 Balanced, every 5 min) and anything read live. All of it carries the mixed
labels.

### What the dropped builds measured (real 10:38 call read in place, MX230, large-v3-turbo + VAD, simulated real time)

| Live variant | Requests | Whisper busy | Median delay | Worst | Text done after audio | You / Call |
|---|---|---|---|---|---|---|
| Mixed (today) | 45–56 | **0.86–0.93×** | 10.8–22 s | 63–86 s | 9–13 s | 19 / 78 (guessed) |
| Per track, both tracks cut at the same points (#23) | 73–80 | 1.03–1.20× | 31–33 s | 84–147 s | 31–142 s | 31 / 42 |
| Per track, each track cut at its own pauses, one queue (019, #25) | 79–81 | 1.02–1.06× | 15–17.5 s | 44–52 s | 30–44 s | 32–37 / 47–59 |

Findings that shape this spec:
- **Cost tracks requests, not seconds.** A Whisper request costs about 8–10.5 s on the MX230, of
  which about 4.7 s is fixed. Per-track live sends about 1.4× the requests of mixed, because
  people take turns about 4.5 times a minute and a short "uhum" still costs a request.
- **Shared cuts grow the backlog without bound**: 1.2×, and the text finished 2+ minutes after the
  call ended. **Own cuts per track plus a single queue bound the backlog** (10–33 s, emptying at the
  end), with a median delay of about 17 s, but **stay just above 1.0×**.
- **Joining both tracks in one request with a 1 s silence gap doesn't work.** Whisper's VAD removes
  the gap, so the split point is lost.
- **VAD glues one speaker's separate turns into one line.** Each track is silent while the other
  person talks, VAD drops that silence, and Whisper writes several turns as one line. Cutting each
  track at its own pauses (019) and 020's packing reduce it, but don't remove it.
- **Echo** (synthetic call at −20 dB in the mic): a text rule dropped all 6 echoes, but also dropped
  a real turn glued to one. The level-based "hold back likely echo" in 019 fired 14–21 times on a
  headphone call with no echo at all.
- **Track timing:** a constant 85 ms start lag between the captures, with no drift over 5 minutes.
  One call had an unexplained 2.2 s length difference. That's spec 021.

## Decision

1. **Live lines get their track from the file they were spoken on.** During a call, each track's
   speech is transcribed on its own. `audio::attribute` and the mixed request are removed from the
   live path. Imports are not live and are unaffected (020 already sends them through the per-track
   path).
2. **Reuse 020's per-track building blocks; don't duplicate them.** Speech detection
   (`audio::speech::speech_spans`), the mapping from window time back to recording time (`Window`,
   `Piece`, `map_segment`) and `merge_tracks` are shared by live and after-call. Where live needs a
   streaming variant (spans that grow as audio arrives), it extends `audio/speech.rs` rather than
   adding a second detector.
3. **Cutting and scheduling are spec 019.** Each track is cut at its own pauses, and one Whisper
   worker takes the earliest ready slice. 018 and 019 are built together. 018 without 019 is the
   shared-cut variant, which measured 1.2× with an unbounded backlog, and is **not to be built**.
4. **Live summary and live view** read lines in start order. Lines can arrive out of order, so the
   view inserts by start rather than appending.
5. **After the call nothing changes.** 020's step still rebuilds the saved transcript from the whole
   tracks. Live per-track only improves what's shown during the call and in `transcript.live.jsonl`.
   With live already per-track, whether 020's rebuild stays on by default is revisited when this
   ships, not here.
6. **Acceptance: the live cost must fit on the MX230.** On the 10:38 call (read in place, counts
   only), at the Balanced profile with the automatic summary running, **Whisper busy ≤ 1.0×** and a
   **bounded backlog**: the backlog may rise but returns below 30 s at least once every 5 minutes,
   and the text is done ≤ 30 s after the audio ends. The median delay must not be worse than mixed
   by more than 10 s. Labels: at least as many You lines as mixed (19) and no Call line spanning a
   You turn of ≥ 2 s. **If 019's design can't meet ≤ 1.0×, this spec isn't shipped.** It goes back
   to a decision instead of landing a live path that falls behind.
7. **Out of scope:** echo removal (020's Known Gaps apply live too, and echo work gets its own spec),
   track alignment (021), and speaker separation live (003 stays after-call only).

## Implementation

### 1. Live path (`session.rs`)

- Replace the mixed `transcribe_chunk` with 019's per-track slices and queue. Each returned segment
  keeps the slice's track; times are mapped back with the same `Window` logic 020 uses.
- `transcript.live.jsonl` is appended in arrival order (crash-safe). Every reader sorts by start
  (020's readers already do for the saved transcript; extend to the live file).
- Remove `audio::attribute` and its test, and `audio::mix` if nothing else uses it.

### 2. Shared code with 020 (`audio/speech.rs`, `pipeline/tracks.rs`)

- Move what both need (span detection, window mapping, `merge_tracks`) into a module both paths
  import. Don't copy it into `session.rs`.
- A streaming span detector (rolling noise floor, as 019 specifies) lives next to `speech_spans`,
  with tests showing both agree on the same complete buffer.

### 3. UI (`app/src/lib/live.svelte.ts`, Record screen)

- Insert incoming lines at their start position (a pure helper in `app/src/lib/` with a test).
  Keep auto-scroll pinned to the bottom unless the user scrolled up.
- `fake.ts`: the fake live session emits one out-of-order pair, so the browser preview shows it.

### 4. Measurement

- `fofoca-cli bench-latency <call.wav> --mic <mic.wav>` gains the per-track live mode (019 defines
  the queue options) and **a summary swap every 5 min** (Balanced), which the dropped builds didn't
  simulate. Report the Decision 6 numbers for mixed against per-track in the same sitting. Run each
  configuration **twice**: run-to-run variation was about 0.1×.

### 5. Tests and checks

- A model-free regression for the original bug: a turn-taking chunk, which the loudness rule
  labelled Call, gives a Call line and a You line.
- An ordering test for out-of-order arrival, plus the UI helper test.
- Checks: `cargo test -p fofoca-core`, `cargo test -p fofoca`,
  `cargo clippy --workspace --all-targets` (no warnings), and in `app/`:
  `npx svelte-check && npm test && npm run build`.
- Hand test: a real call with headphones, Balanced, past the 5-minute summary. Check the labels while
  taking turns, how far the text lags, and that the backlog empties.

## Known Gaps

- **Fitting the MX230 is not proven.** The best measured variant was 1.02–1.06×. 019 lists the
  levers (`--audio-ctx`, a longer minimum slice, which costs label detail). If none fit, the
  options are a smaller live model, or per-track only while Whisper is idle with mixed under
  backlog. Each would be its own decision.
- **Turns glued into one line** still happen within a track when VAD drops the other person's
  turn. Splitting lines at gaps in word timing is a possible follow-up.
- **Echo without headphones** gives You lines that repeat the call, live as after the call (020).
- **Overlapping speech** gives two lines with close starts. There's no interleaving inside a line.
