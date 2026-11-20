# 003 — Speaker separation after the call

## Context

The mic track already identifies **You**. The system-audio track can hold several remote speakers.
Diarization models are language-independent and run well on CPU, so they don't compete with
Whisper for VRAM.

## Decision

- Run **after the call** on the system-audio track only, on CPU.
- Use **pyannote-rs** (segmentation-3.0 + WeSpeaker embeddings, ONNX via `ort`); sherpa-onnx is
  the fallback if accuracy or packaging disappoints.
- Assign each transcript **word** (timestamps kept since 001) to the speaker segment with the
  largest overlap; segments become *Pessoa 1, Pessoa 2, …*.
- Speakers tab: rename speakers; suggestions from calendar attendees (004). Rename applies across
  the meeting.

## Implementation

### Post-call pipeline (new, shared with 005 and 012)

`fofoca-core/src/pipeline.rs` runs after the live session has drained its backlog (and for imports,
012). Steps, each emitting `SessionEvent::Step` and recorded in `meta.json` (`steps_done`) so a
failed step can be retried without redoing the others:

1. `speakers` — separation on `system.wav` (CPU), this spec.
2. `summary` — the final summary (today's prompt; 005 replaces it with grounded extraction).

The session's current final-summary code moves here. The UI's "after the call" screen lists the
pipeline steps instead of today's fixed three.

### Speaker separation

- Crate **`pyannote-rs`** (ONNX via `ort`): segmentation-3.0 → speech turns; WeSpeaker embedding
  per turn → online clustering (`EmbeddingManager`, cosine threshold, `max_speakers =
  max(attendees + 1, 4)`). Verify the crate API/version at build time; keep the call surface behind
  `diarize::separate(wav: &[i16], cfg) -> Vec<Turn { start, end, speaker: u32 }>` so sherpa-onnx can
  replace it.
- Models (`segmentation-3.0.onnx` ~6 MB, `wespeaker_en_voxceleb_CAM++.onnx` ~28 MB) go in
  `~/.local/share/fofoca/models/`; config `[diarization] segmentation_model`, `embedding_model`,
  `threshold = 0.5`. Missing models → step skipped with a notice (downloads arrive with 006).
- Runs on **CPU only**, in parallel with nothing else on the GPU being needed; budget target ≤ 3 min
  per hour of audio on the i7-10510U (measure with the MLS file and the user's recording; record in
  the spec notes).

### Assigning speakers to the transcript

- Only `Call` segments are re-attributed; `You` segments keep speaker `you`.
- Per word (interpolated timings from 001), pick the turn with the largest overlap; a word with no
  overlap takes its neighbour's speaker. A segment whose words change speaker is **split** at that
  word boundary (text re-joined from words).
- **Measure** on the test recording whether interpolated word timings put the boundaries in the
  right place (target: ≥ 90 % of words on the correct speaker vs a hand-labelled reference). If not,
  add a precise pass: re-transcribe `system.wav` with `verbose_json` (+ `--dtw large.v3.turbo` if
  it helps) before assigning. That pass costs ~0.5× audio length on the GPU, so it is a setting
  (`diarization.precise_words`, default off) unless the measurement says otherwise.
- `model.rs`: `Segment.speaker: Option<String>` (`"you"`, `"s1"`, `"s2"`, …); `speakers.json` per
  meeting: `{ "s1": { "name": "Fernanda" }, … }`. `transcript.jsonl` is rewritten atomically
  (temp + rename) after assignment; the live version is kept as `transcript.live.jsonl` until the
  step succeeds.

### UI and commands

- Speakers tab on the meeting page: each speaker with talk time and two sample lines; rename with
  a text field + suggestions from `meta.attendees`; renames apply everywhere (`speakers.json`).
- Transcript shows speaker names (colour per speaker) instead of only You/Call.
- Tauri: `rename_speaker(id, speaker, name)`, `rerun_step(id, step)`.
- CLI: `fofoca-cli diarize <meeting-id>` (runs the step and prints turns + timings).
- Tests: overlap assignment and segment splitting on synthetic turns; `speakers.json` round trip;
  pipeline step bookkeeping. The ONNX run itself is an `#[ignore]` test using local models.

## Known Gaps

- Live (during-call) separation is out of scope.
- Overlapping speech is attributed to a single speaker.
