# 011 — Audio retention

## Context

Every meeting keeps `mic.wav` and `system.wav` forever (~115 MB per hour at 16 kHz mono s16 ×2).
Audio is the most sensitive artefact — other people's voices — and after transcription,
speaker separation (003) and the grounded summary (005) it is rarely needed. Keeping it should be
a choice, not the default by accident.

## Decision

- Setting **Keep meeting audio**: `always` | `after_processing` (**default**) | `days` (N, default
  30).
  - `after_processing`: audio is deleted once the post-call pipeline (transcription drain, 003
    speaker separation, 005 summary) has finished successfully. If a step failed, audio is kept so
    it can be retried.
  - `days`: deleted N days after the meeting started, checked at app start and after each meeting.
- **Per meeting**: "Keep audio" toggle (overrides the global setting, e.g. for a call that must be
  re-listened) and "Delete audio now".
- Transcript, summaries, labels and speakers are **never** deleted by this setting.
- Imported recordings (012): the original file outside fofoca's folder is never touched.

## Implementation

- `fofoca-core`
  - `config.rs`: `[audio] keep = "after_processing" | "always" | "days"`, `keep_days = 30`.
  - `model.rs`: `MeetingMeta.keep_audio: Option<bool>` (per-meeting override),
    `audio_deleted_at: Option<String>`, `processed: bool` (all post-call steps succeeded).
  - `retention.rs`: `apply(store, cfg, now) -> Vec<(id, freed_bytes)>` — pure decision function
    (`should_delete(meta, cfg, now)`) + deletion of `*.wav` and recording `audio_deleted_at` via
    `Store::update_meta`. Unit tests for every policy/override combination.
  - Post-call pipeline sets `processed = true` and calls `retention::apply` for that meeting.
- Tauri: run `retention::apply` at startup (background thread); commands `set_keep_audio(id, bool)`,
  `delete_audio(id)`; Settings → Storage section with the policy and total audio size.
- UI: meeting page shows "Audio kept" / "Audio deleted on …" and the two controls; actions that
  need audio (re-run speakers, precise words) are disabled once it's gone.
- CLI: `fofoca-cli retention [--dry-run]`.

## Known Gaps

- No compression (e.g. Opus) as a middle ground — later if people want to keep audio cheaply.
- No secure-erase; files are unlinked normally.
