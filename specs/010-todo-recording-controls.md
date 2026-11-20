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
