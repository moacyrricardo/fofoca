Status: done
Branch: moacyrricardo/spec-010-recording-controls

# 010 — Recording controls: pause, and faster live text

## Context

Two gaps from using 001 on real calls:

1. **No Pause.** Side conversations or a break mean either recording things that shouldn't be
   kept or stopping and getting two meetings.
2. **Live text lags 15–40 s.** Chunks are 30 s; a line said early in a chunk waits up to 30 s plus
   ~10–18 s of transcription.

Constraint found while planning: whisper.cpp's encoder always processes a **30 s window** (shorter
audio is padded), so per-request cost is roughly fixed (~10 s with VAD for speech on the MX230,
~18 s without). Simply using 10 s chunks would triple GPU work and fall behind. Latency can only
improve by cutting **earlier when the GPU is idle**, at natural pauses, and possibly by shrinking
the encoder window (`audio_ctx`), which trades accuracy.

## Decision

- **Pause / Resume** button on the Live screen (and `fofoca-cli record` ignores it).
  - While paused, captured audio is **dropped**, not stored or transcribed; the meeting timeline
    skips the paused interval (timestamps stay continuous).
  - On Pause, the audio already buffered is cut and transcribed right away, so the last words show
    up without waiting.
  - Paused intervals are recorded in `meta.json` (`pauses: [{at_s, wall_from, wall_to}]`) and shown
    as a marker line in the transcript ("— paused 4 min —").
  - Auto-record (004) never resumes a paused meeting.
- **Adaptive chunking**: when Whisper is idle and the buffer holds at least `min_chunk_seconds`
  (default 12 s), cut at the first pause (≥ 350 ms of relative quiet) instead of waiting for 30 s.
  With a backlog, keep 30 s chunks (throughput first). `chunk_seconds` stays the maximum.
- **Measure before shipping a default**: record latency (speech end → segment shown) and backlog
  growth on the 75 s pt-BR recording and the MLS 189 s file for fixed 30 s vs adaptive, with and
  without `--audio-ctx` scaled to the chunk length. Ship adaptive only if backlog stays bounded and
  WER doesn't regress by more than 1 point.

## Implementation

- `fofoca-core`
  - `capture.rs`: `TrackBuffer::set_discard(bool)` — while set, incoming samples are dropped and
    the level meter still updates (so the UI shows the mic is live but not recorded).
  - `session.rs`: `Command::Pause` / `Command::Resume`; `Session::pause()` / `resume()`. On pause:
    flush buffered audio as a chunk (same path as Stop's tail), set discard on both tracks, append
    a pause entry. `SessionEvent::Paused { at_s }` / `Resumed { at_s }`. Status gets `paused: bool`.
  - `audio.rs`: `pause_cut(mic, sys, min, max) -> Option<usize>` — first ≥ 350 ms window whose RMS
    is below `max(noise_floor × 2, 0.006)`, noise floor = 10th percentile of 50 ms window RMS over
    the buffer (an absolute threshold doesn't work: mic noise 0.003–0.015 overlaps quiet speech).
    Unit tests with synthetic speech/silence.
  - Worker loop: if `asr` is Ready and no backlog beyond `max chunk`, try `pause_cut` once the
    buffer ≥ `min_chunk_seconds`; otherwise current behaviour.
  - `config.rs`: `min_chunk_seconds` (default 12; 0 disables adaptive), optional
    `whisper.audio_ctx`.
  - `model.rs`: `MeetingMeta.pauses: Vec<Pause>` (`serde(default)`).
- Tauri: `pause_session`, `resume_session`.
- UI (`Live.svelte`): Pause/Resume button next to Stop; paused state dims the meters and shows
  "Paused — nothing is being recorded"; timer stops while paused. Pause markers in transcript views.
- CLI: `fofoca-cli bench-latency <wav>` replays a file through the session chunker in real time and
  prints per-segment latency and backlog — the measurement above.

## Known Gaps

- Pause doesn't stop the model servers (VRAM stays used) — fine for short breaks.
- Streaming partial text inside a chunk (word-by-word) is out of scope; it needs a different ASR
  setup.

## Implementation Notes

- **Measured** with `fofoca-cli bench-latency` on the 75 s pt-BR recording (turbo q5_0 + Silero
  VAD, MX230). Time is simulated (audio arrives at real speed, Whisper requests are real), which
  replaced the planned real-time replay: same numbers, no waiting.

  | Mode | Requests | Whisper busy | Line latency median / p90 / max | WER* |
  |---|---:|---:|---|---:|
  | fixed 30 s | 3 | 30.2 s (40 %) | 21.5 / 30.3 / 30.7 s | 11.2 % |
  | min 12 s | 5 | 43.8 s (58 %) | 15.8 / 24.4 / 27.3 s | 11.2 % |
  | min 10 s | 5 | 43.8 s (58 %) | 16.0 / 24.5 / 27.4 s | 10.6 % |
  | **min 8 s (default)** | 6 | 52.3 s (70 %) | **12.7 / 20.5 / 22.4 s** | 10.6 % |

  \*plain word-level WER, digits vs spelled-out numbers count as errors — for comparing settings.
  Backlog stayed under 2 s in every run. Adaptive chunking can't fall behind: each chunk is at least
  the audio that arrived during the previous request, so at worst Whisper runs back to back.
  Default `min_chunk_seconds = 8` (spec proposed 12). The MLS 189 s file wasn't available on this
  machine; only the pt-BR recording was measured.
- **`--audio-ctx` dropped**: whisper-server applies it to every request, so a 30 s chunk with a
  15 s context would silently lose its second half. Not usable without a per-request setting.
- **Pause takes effect immediately**: `Session::pause()` flips the capture buffers to discard from
  the caller's thread (same idea as Stop killing capture), then the worker flushes what was
  buffered and records `Pause { at_s, from, to }` (spec said `wall_from/wall_to`). `at_s` is
  computed from the buffered length at pause time, so it is exact even while a model swap is in
  progress. A recording stopped while paused leaves `to: null`.
- The pause-cut scan runs at most every 0.5 s of new audio (it copies the buffer).
- Quiet detection threshold: 2× the buffer's 10th-percentile level, at least 0.006, **at most 30 %
  of the median** — without the cap, a mostly-speech buffer made speech count as quiet (caught by
  the unit test).
- `fofoca-cli record` has no pause flag; pausing is UI-only.
- Not verified by hand yet: the Pause button in the running app (headless preview can't click).
