Status: done
Branch: moacyrricardo/spec-015-models-workflow

# 015 — Models & workflow: choose the models and what runs where

> **Design reference:** [`015-assets/models-workflow-mock.html`](015-assets/models-workflow-mock.html) —
> clickable mock of the new Settings (toggle "ⓘ annotations" for what's invented).

## Context

Today the setup is fixed: Whisper live on the GPU, the summary model swapped in on demand, the
model files and server paths set by hand in `config.toml`, and every post-call step always runs.
That fits the baseline laptop (MX230, 2 GB) but not a machine without a usable GPU, one with
more VRAM, or a user who wants the computer free during calls. The benchmarks (001) showed the
choices that matter are *which model per task* and *where each runs*; the user should make them
once, globally, without editing files. Packaging (006) should not bundle models: the user points
at files they have or downloads a suggested one.

## Decision

Settings gets **tabs**: **Processing** (default) · **Models** · **Calendars** · **Storage**.
Above the tabs, the **Current setup** banner summarises the choices (e.g. "Swap layout · Balanced
updates") and stays visible on every tab. All choices are **global**; a recording uses the
settings it started with ("changes apply to the next recording" while one runs).

**Processing**
- A line naming the models it plans for, linking to Models.
- **During the call**: *Live transcript* and *Live summary*, each on/off.
  - Live summary needs the live transcript: turning the transcript off turns the summary off.
  - Both off = **record only**: nothing runs during the call; the transcript is made when you stop.
- **Where each runs** (when both are on):
  | Layout | Speech → text | Summary |
  |---|---|---|
  | Both on the GPU, taking turns (*swap*, today's) | GPU | GPU, swapped in (~3 s) |
  | Transcript on GPU · summary on CPU | GPU | CPU |
  | Transcript on CPU · summary on GPU | CPU | GPU |
  | Everything on the CPU | CPU | CPU |
  | Both loaded on the GPU at once | GPU | GPU, no swap — only when both fit |
  Each option shows a GPU/CPU lane diagram and a one-line consequence. With only one live task,
  a single GPU/CPU choice replaces the cards. A **GPU memory** meter shows the peak for the
  choice; options that need a GPU are disabled when none is found, and "both at once" is disabled
  (with the numbers) when the two models don't fit.
- **Live updates**: **Fast** · **Balanced** (default) · **Light** — one setting instead of chunk
  length and summary cadence:
  | | Audio pieces | Summary so far |
  |---|---|---|
  | Fast | adaptive 8–15 s | every 3 min |
  | Balanced | adaptive 8–30 s (spec 010) | every 5 min |
  | Light | 30 s | only when asked |
  Balanced is marked *measured* (text ~13 s after it's said); Fast and Light *estimate*. The
  "Summary so far" button on the Record screen works in every mode. Editing the raw values in
  `config.toml` shows **Custom**.
- **After the call**: *Separate speakers* (CPU) and *Write checked notes* (GPU or CPU), each
  on/off, with a "When you stop: Transcribe → Speakers → Checked notes → Storage" line. The same
  steps apply to imports (012).

**Models**
- One row per task — *Speech → text*, *Summaries & notes*, *Speakers*, plus *Voice detection* —
  showing the file, whether it's on this computer, and a picker of suggested models plus
  "Another file on this computer…". A missing suggested model offers **Download** (size shown,
  progress, cancel); downloads go to `~/.local/share/fofoca/models/` and are checked by size.
- **Engines** (`whisper-server`, `llama-server`): path, found or not, "Use another…". Downloading
  engines is packaging (006).

**Calendars / Storage**: the existing cards, unchanged; the config-file and meetings-folder lines
move into Storage.

## Implementation

- **Config** (`config.rs`): new `[workflow]` table — `live_transcript`, `live_summary`,
  `layout` (`swap` | `gpu_transcript` | `gpu_summary` | `cpu` | `gpu_both`), `updates` (`fast` |
  `balanced` | `light` | `custom`), `summary_every_s` (custom only), `speakers_after`,
  `notes_after`, `notes_device` (`gpu` | `cpu`). Defaults reproduce today's behaviour except the
  new automatic summary every 5 min. `Workflow::pace()` → `(min_chunk_s, chunk_s, summary_every)`;
  `custom` uses the top-level `chunk_seconds` / `min_chunk_seconds`. `Config::save_workflow` and
  `save_models` edit the file with `toml_edit`, keeping comments (as `save_audio` does).
- **Devices** (`servers.rs`): `ModelServer::start(kind, cfg, device)` with `Device::{Gpu, Cpu}`.
  CPU: whisper `-ng`; llama `-dev none -ngl 0`. GPU keeps today's flags (`[whisper].device`,
  `[llm].device`).
- **Session** (`session.rs`): devices from the layout; Whisper is stopped for a summary only when
  both run on the GPU and don't both fit (*swap*); the summary every N s comes from `pace()`; the
  chunk lengths from `pace()`. Record only: chunks are written to the WAV files but not
  transcribed; on stop the files are read back and transcribed in chunks the same way (You/Call
  attribution kept). After the call, speakers and notes run only if enabled, notes on
  `notes_device`; `processed` means "every *enabled* step succeeded" (retention, 011).
- **Imports / Write notes**: `pipeline` honours `speakers_after`, `notes_after`, `notes_device`.
- **GPU probe** (`hardware.rs`): `llama-server --list-devices`, parsed for the configured
  `[llm].device` (name, total and free MiB); "no usable GPU" when it's missing. The *Check this
  computer again* button re-runs it. A footprint table holds measured sizes (turbo 824 MiB,
  Qwen3.5-2B ~1,414 MiB at 8K context) and estimates file size × 1.15 for anything else
  (labelled *estimate*). Suggested layout: both at once if they fit, else swap if a GPU exists,
  else CPU. (Benchmarking speed on first run stays in 002.)
- **Models** (`models.rs`): a catalog of suggested files (task, name, URL, size): Whisper
  large-v3-turbo q5_0 (574 MB) and small q5_1 (190 MB), Silero VAD v5.1.2 (0.9 MB), Qwen3.5-2B
  Q4_K_M (1.28 GB), Qwen3-1.7B Q4_K_M (1.11 GB), segmentation-3.0 (6 MB) + WeSpeaker CAM++
  (29 MB). `download(id)` streams to `<file>.part` with progress events, checks the size, renames;
  cancellable. A catalog entry counts as present if a file with its name exists in the models
  folder or is the configured path.
- **Tauri**: commands `workflow_view`, `set_workflow`, `models_view`, `set_model_path`,
  `pick_model_file`, `download_model`, `cancel_download`, `probe_hardware`; `download` events.
  The in-memory config is updated with the saved file.
- **UI**: `Settings.svelte` becomes the tabbed screen; new `ProcessingTab.svelte`,
  `ModelsTab.svelte`; `CalendarsCard` / `StorageCard` reused. `Live.svelte`: no transcript area
  in record-only mode, no live-summary panel when it's off, a single "■ Stop" when notes are off.
  Fake backend covers every command.
- **Tests**: config round-trip and `pace()`, layout → devices and swap decision,
  `--list-devices` parsing, footprint/fit, catalog presence, download to a local HTTP server
  (size check, cancel), `processed` with disabled steps.

## Known Gaps

- Speed estimates for Fast/Light and CPU transcription aren't measured — 002's first-run
  benchmark should replace them.
- No per-recording choice (by decision).
- Engines can't be downloaded yet (006).

## Implementation Notes

- Built as specified, with these differences:
  - **Live summary needs the live transcript** (decided here): turning the transcript off turns
    the summary off and disables it, instead of transcribing silently in the background.
  - `[workflow].live_device` was added: where the transcript runs when it's the only live task.
  - **The summary model stays loaded between summaries** in every layout that doesn't swap.
    Measured while building: llama-server on the CPU took **57 s** to load Qwen3.5-2B from this
    disk (cold), so restarting it for every automatic summary wasn't viable. Whisper on the CPU
    loaded in ~5 s. Qwen3.5-2B on the CPU wrote at **10.9 tokens/s** (earlier: 11–13).
  - "Both loaded on the GPU at once" is a real layout (`gpu_both`), selectable when the two
    footprints fit in 95 % of the GPU, or when a footprint is unknown.
  - The suggested layout comes from the GPU probe + footprints only; speed benchmarking stays
    in 002.
  - Models tab: a menu per task lists the suggested models (with "· N MB to download" when
    missing); "Use another…" picks your own file (two files in a row for Speakers). Downloads
    are checked by exact size (no sha256 — 006 may add it).
  - CPU flags: whisper `-ng` (log: `use gpu = 0`, model in CPU memory); llama `-dev none -ngl 0`
    (no Vulkan buffers). Both checked by hand against the real binaries.
- **Tests**: Rust — workflow defaults/pace/save (older config files read as today's behaviour),
  live-summary rule and devices, `--list-devices` parsing, footprints and suggestion, catalog
  presence/current entry, downloader against a local HTTP server (size check, cancel, no partial
  file left). TS (`npm test`, 9) — GPU memory peak per layout, availability rules, banner titles,
  after-call steps. The browser preview was driven headlessly through every tab, the layouts,
  Live updates, record-only, notes off, a fake download and the Record screen.
- **Not verified yet** (needs `cargo tauri dev` and a real call): each layout during a real
  recording, record-only transcription after stop, an automatic summary every 5 min, a real
  download through the app, and the GPU probe inside the app.
- Known gap added: `processed` with steps turned off has no dedicated test (the rule is "every
  enabled step succeeded").
