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

To be detailed when picked up. Measure whether default Whisper word timings are precise enough;
evaluate whisper.cpp `--dtw large.v3.turbo` if not.

## Known Gaps

- Live (during-call) separation is out of scope.
- Overlapping speech is attributed to a single speaker.
