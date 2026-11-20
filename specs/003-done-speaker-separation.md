Status: done
Branch: moacyrricardo/spec-003-speaker-separation

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

## Implementation Notes

- **No pyannote-rs dependency.** Its fbank crate (`knf-rs-sys`) builds C++ with cmake + bindgen
  (libclang); neither is on the baseline laptop, and the AppImage build (006) is simpler without a
  C++ toolchain. Its ONNX handling (MIT) is ported in `diarize.rs` onto **`ort` 2.0.0-rc.10**
  (prebuilt ONNX Runtime downloaded at build time) and **`kaldi-fbank`** (pure-Rust port of
  kaldi-native-fbank; same options: 80 bins, no dither, snip edges, mean-normalized). The
  `diarize::separate` surface is what the spec asked for, so sherpa-onnx can still replace it.
- Two changes from pyannote-rs: turns are split where the segmentation's **speaker class changes**
  (pyannote-rs only splits at silence, so two people talking back to back became one turn;
  changes shorter than 0.4 s are merged), and clustering is **offline average linkage** over all
  turns (cosine ≥ 0.5) instead of online first-come matching. Turns under 0.8 s get no embedding and
  take their nearest identified neighbour's voice.
- Models: `segmentation-3.0.onnx` (5.98 MB, sha256 `b78fc481…`) and
  `wespeaker_en_voxceleb_CAM++.onnx` (29.3 MB, sha256 `c46fad10…`) from the pyannote-rs v0.1.0
  release, in `~/.local/share/fofoca/models/`; config `[diarization]`. Missing models → the step is
  skipped with a notice. Downloads come with 006.
- **Speed**: pyannote-rs' 40.7 s `6_speakers.wav` in 1.2 s on the i7-10510U **in a debug build**
  (0.03× real time) — far under the 3 min/hour budget. Similarities between turns: same voice
  0.82–0.94, different voices 0.18–0.56. It finds 4 voices; the 2 others only have turns under
  0.8 s. The sample has no reference labels, so this is a sanity check, not an accuracy number.
- **Not measured**: word-boundary accuracy against a hand-labelled pt-BR multi-speaker recording
  (none exists yet), so the precise-words pass (`verbose_json` / `--dtw`) and the
  `diarization.precise_words` setting were **not built**. Words keep the interpolated timings from
  001; a line is split only where at least 2 words change voice. Revisit with a real call.
- The session frees Whisper's VRAM before the speakers step, then runs the final summary with
  speaker names in the prompt (`[mm:ss] Fernanda: …`).
- The pipeline module holds the speakers step only; the final summary still runs in the session
  and moves to `pipeline.rs` with 005 as planned.
- `max_speakers = max(attendees + 1, 4)`; re-running keeps names by key (`s1`, `s2`, …), which
  can attach a name to a different voice if clustering changes — the Speakers tab shows sample
  lines so it's visible.
- Tests: assignment/splitting, gap filling, roster, display names, region building, clustering,
  store round trip; `pipeline::tests::speakers_step_on_a_real_recording` (ignored by default,
  runs the real models on a WAV given in `FOFOCA_DIAR_WAV`) passed on the six-speaker sample.
- UI checked in the browser preview (Speakers tab, named transcript). Not yet tried on a real call.
