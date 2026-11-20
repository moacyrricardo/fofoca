# 012 — Import a recording

## Context

Recordings made elsewhere — a phone voice memo, a WhatsApp audio, a Zoom cloud recording, an
in-person meeting recorded on the laptop — should become fo-foca meetings: transcript, speakers,
grounded summary, labels. Concern 008 needs the same capability for an MCP `import_audio` tool;
building it as a UI feature first gives MCP a tested path to call.

## Decision

- **Meetings → "Import recording…"**: pick a file (any format ffmpeg reads), optional title
  (default: file name), labels, and start time (default: the file's creation time from ffprobe
  metadata, else its modification time).
- The file is converted with **ffmpeg** to 16 kHz mono and stored as the meeting's `system.wav`;
  there is no mic track, so every segment is track `call` and speaker separation (003) runs on the
  whole recording.
- Processing runs **offline**: no real-time budget, so it uses the post-call quality settings
  (precise word timestamps when 003 asks for them) and the shared post-call pipeline (003 speakers →
  005 summary). Progress is shown per step on the meeting page.
- **One job at a time, queued.** Imports never compete with a live recording for the GPU: a queued
  import waits while a recording runs, and starting a recording while an import is transcribing
  pauses the import after its current chunk.
- The original file is never modified or deleted; retention (011) applies only to fofoca's copy.

## Implementation

- `fofoca-core`
  - `import.rs`: `probe(path) -> ImportInfo { duration_s, created_at }` (ffprobe JSON),
    `convert(path, out_wav)` (`ffmpeg -i <in> -ac 1 -ar 16000 -f wav`), errors for missing ffmpeg
    or unreadable input.
  - `pipeline.rs` (introduced by 003): `transcribe_file(meeting, wav)` — the chunk loop from
    `fofoca-cli transcribe` (VAD, quiet cuts, prompt carry-over) writing `transcript.jsonl`, then
    the post-call steps.
  - `jobs.rs`: a single worker thread with a FIFO queue of jobs (`Import`, later re-runs), events
    `JobEvent { meeting_id, step, progress }`; `pause_for_recording()` / `resume()` hooks called by
    `Session::start` / finish.
  - `model.rs`: `MeetingMeta.source: Option<Source>` (`Recorded` default | `Imported { file_name }`).
- Tauri: `import_recording(path, title?, labels?, started_at?)` → meeting id; event `job`. File
  picker via `tauri-plugin-dialog`.
- UI: import dialog on Meetings; imported meetings show an "Imported" chip and a progress card
  until processed.
- CLI: `fofoca-cli import <file> [--title T] [--label L]… [--at RFC3339]`.
- Tests: ffprobe/ffmpeg argument building; queue ordering and pause-for-recording logic (with a
  fake job); an integration test behind `#[ignore]` that imports the bundled sample.

## Known Gaps

- Stereo files with one speaker per channel are mixed down; per-channel You/Call split is later.
- Video files work (audio is extracted) but the video isn't kept.
