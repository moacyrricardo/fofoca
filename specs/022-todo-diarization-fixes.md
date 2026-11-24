# 022 — Diarization fixes: embeddings, timing, cluster cleanup

## Context

Speaker separation (003) has been unusable on real calls. A 1:1 call (one remote voice) comes out as
4 speakers. A daily with 4 remote people comes out as 7–10 speakers, and the biggest "speaker" is
several people mixed together.

Measured on 2026-11-24 with test mixes built from real fofoca recordings with known speakers
(your mic, plus the call track of two 1:1 calls, so each source holds one voice). Speakers take
turns with 0.3 s gaps. **Speech correct** is the share of speech time given to the right person
after the best one-to-one mapping.

| Mix | Today | Fix 1 only | Fixes 1+2+3 (threshold 0.3–0.4) |
|---|---|---|---|
| 3 voices (mic + 2 call voices), 592 s | 6–10 speakers, 31–41% | 14 speakers, 88% | **3 speakers, 94%** |
| 2 call voices, 359 s | 3–9 speakers, 36–58% | 10 speakers, 90% | **2 speakers, 94–95%** |

On the real calls, with fixes 1–3 prototyped in Python:

- **1:1 call** (2026-11-23 "Moa <> Vini"): 4 speakers today → 1.
- **Daily** (2026-11-24, 4 remote people): 10 speakers today → 4 at threshold 0.4 (3 at 0.3).

Speech turns from segmentation-3.0 were already **99% single-speaker**, so segmentation isn't the
problem. These were ruled out: audio sample scale (mean normalization cancels it), Hamming vs Povey
window (no difference), and the fbank features themselves (the Rust `kaldi-fbank` output matches
`kaldi-native-fbank` to 4 decimals). A GPU or a bigger model isn't needed.

## Decision

Three bugs, fixed together:

1. **The embedding model runs with graph optimization off.** In the ONNX Runtime bundled with
   `ort = 2.0.0-rc.10`, **any** graph optimization (Level1 and up) corrupts the WeSpeaker CAM++
   embedding model. For the same features, Rust gives vectors with norm ~150 that have cosine ≈ 0
   to the correct embedding. With `GraphOptimizationLevel::Disable` the output matches Python
   onnxruntime 1.32 exactly (norm 18.97, same values). The segmentation model isn't affected, and
   its regions are identical either way.
2. **Segmentation frame times restart at each window.** The code numbers frames globally:
   `time = (721 + frame × 270) / 16000`. A 10 s window gives 589 frames × 270 = 159 030 samples,
   not 160 000, so times fall 60.6 ms behind per window: about 13 s by the end of a 35-minute call,
   and 22 s by the end of an hour. Words then land on the wrong turns (`speakers::assign`). Frame
   `f` of window `w` must map to `(w × 160 000 + 721 + f × 270) / 16000`.
3. **Clean up the clustering.**
   - Fold small clusters into the nearest large one (pyannote-style minimum cluster size).
   - `max_speakers` no longer forces merges during agglomeration. Forced merges are what turn real
     speakers into one blob while noisy short-turn clusters stay separate.
   - The default threshold changes from 0.5 to **0.4**.

A regression check comes with the fixes, so a broken embedding can't pass silently again.

## Implementation

### 1. Embedding session (`crates/fofoca-core/src/diarize.rs`)

- `session(path)` takes the optimization level. The embedding model uses
  `GraphOptimizationLevel::Disable`. Segmentation keeps `Level3`; it was verified unaffected, so
  don't change it.
- Add a comment above the embedding session saying why (the ort rc.10 / ONNX Runtime optimizer
  corrupts CAM++). That way a future `ort` upgrade knows to re-test before re-enabling
  optimization.
- Speed: embedding is a small part of runtime. The 592 s mix stays well under 0.05× real time
  (re-measure and report in Implementation Notes).

### 2. Frame timing (`speech_regions`)

- Keep the window index: frame `f` of window `w` → `w * window + FIRST + f * FRAME` samples, then
  convert to seconds and clamp to the audio length. Pull this into a pure
  `fn frame_time(w: usize, f: usize) -> f64` (or store `(window, frame)` per class entry) so it can
  be unit tested.
- `regions_from_classes` keeps working on a flat class list with a time function. Only the time
  mapping changes.

### 3. Clustering (`cluster`, `Options`)

- `Options { threshold, max_speakers: Option<usize>, min_cluster_s }`.
- Agglomerate with average linkage on cosine similarity, as today, but **stop only at the
  threshold** (no forced merges).
- **Minimum cluster size.** A cluster is *large* if its total speech (sum of its regions'
  durations) is at least `min_cluster_s = min(10 s, 10 % of all embedded speech)`. If no cluster
  qualifies, the one with the most speech is the only large one. Each region of a small cluster
  moves to the large cluster whose centroid (mean of normalized member embeddings) is most similar
  to it.
- **Cap, after cleanup only.** If `max_speakers` is `Some(n)` and there are still more than `n`
  large clusters, merge the two with the most similar centroids until `n` remain.
- `pipeline.rs` stops passing `(attendees + 1).max(4)`. It passes `max_speakers = None`. The
  attendee-based cap is unreliable (020 already calls it a separate fix) and has a floor of 4 that
  hid the problem. `diarize-wav --max N` still sets the cap for experiments.
- Regions without an embedding (< `MIN_EMBED_S` = 0.8 s) still take the label of the nearest
  embedded region in time. Labels are still renumbered by first appearance.

### 4. Config (`config.rs`)

- `DiarizationConfig::default().threshold` changes from 0.5 to **0.4**. The user's config file has
  no `[diarization]` table, so the new default applies directly. A config that does set
  `threshold` keeps its value.
- Update the doc comment on `threshold`, and any 003 or settings text that says 0.5.

### 5. Regression check

- **Unit tests (always run):**
  - `frame_time`: window 0 frame 0 = 721/16000. Window 3 frame 0 = 30 s + 721/16000. Frames never
    run backwards across a window boundary.
  - Clustering: two tight groups plus one far-off singleton with a short duration → 2 speakers,
    with the singleton folded into the nearer group. Same input with `max_speakers = Some(1)` → 1
    speaker. A threshold above every similarity, with no cap and no small clusters → nothing is
    forced together.
- **Golden embedding test (`#[ignore]`, needs local models, like the existing ignored ONNX test in
  `pipeline.rs`):**
  - Build a fixed 3 s input in the test: a deterministic sum of sines plus seeded noise, at int16
    scale.
  - Run `embed()` and compare with a checked-in reference vector,
    `crates/fofoca-core/tests/data/campp_reference.json` (512 floats plus a description of the
    input). Pass if cosine ≥ 0.999.
  - Generate the reference with Python `onnxruntime` + `kaldi-native-fbank` (80 bins, dither 0,
    Povey window, mean-normalized) from a small checked-in script,
    `scripts/diarize_reference.py`. The script rebuilds the same input and writes the JSON.
  - This test fails on today's code (cosine ≈ 0).
- **Accuracy tools (CLI, `fofoca-cli`):**
  - `diarize-mix <out-dir> <name>=<wav> …` builds a test mix from single-speaker WAVs: speech
    regions from segmentation, blocks of 3–15 s, a 0.3 s gap, fixed seed. It writes `mix.wav` and
    `truth.txt` (`start end name`).
  - `diarize-score <wav> <truth.txt> [--threshold T] [--max N]` runs `separate` and prints speakers
    found vs truth, speech correct %, and a per-speaker × per-truth seconds table.
  - The scoring (overlap matrix, greedy one-to-one mapping) lives in `diarize` as a pure function
    with a unit test.
- **Accuracy test (`#[ignore]`):** if `FOFOCA_DIAR_FIXTURE` names a directory holding
  `mix.wav` + `truth.txt`, assert the speaker count equals the truth count and speech correct is
  ≥ 90%. Fixtures stay local: they're built from private recordings and must not be committed.

### 6. Verify on real meetings

Use `diarize-wav` (or `fofoca-cli diarize <id>`) on the 1:1 call 2026-11-23-143243 (expect 1
speaker) and the daily 2026-11-24-102202 (expect 4). Report the numbers in Implementation Notes.

## Known Gaps

- **Overlap regions.** Segmentation's overlap classes (two people at once) become their own regions
  and get an embedding of mixed voices. Measured impact is small (99% region purity), so they're
  left as they are.
- **Local speaker classes across windows.** Speaker-class numbers are local to each 10 s window, so
  a run of the same class number across a window boundary can join two people. It's rare in
  practice and not fixed here.
- **Better clustering** (VBx / PLDA as in pyannote community-1), a stronger embedding model
  (ResNet34-LM), or a GPU path. Not needed for the measured gains; revisit if accuracy on real
  calls stays below about 90%.
- **Call codec voices stay the hardest case.** Two remote voices through the call codec separate
  less cleanly than mic vs call (same-speaker 0.48 vs different-speaker 0.25 cosine, against 0.54
  vs 0.11). Expect occasional mistakes between similar remote voices.
- **Attendee-based speaker cap** from the calendar invite stays out of scope, as in 020.
