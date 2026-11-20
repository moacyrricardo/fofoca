# 002 — Auto setup: hardware probe, layout planner, first-run benchmark

> **Design reference:** [`001-assets/ui-mock.html`](001-assets/ui-mock.html) — Settings screen.

## Context

Picking models from guessed hardware is what makes other local meeting apps unusable on small
machines (e.g. treating an integrated GPU as discrete and choosing a 14 GB model). fo-foca must
choose a setup from *measured* facts and let the user override everything.

## Decision

- **Probe** with the same backends that will run the models: `llama-server --list-devices`
  (Vulkan devices with total/free MiB), plus RAM and CPU threads. Prefer discrete GPUs; never count
  an integrated GPU's shared memory as VRAM.
- **Plan** from a shipped table of *measured loaded footprints* per model/quant/context (not file
  sizes), with a safety margin. Layouts: `both-gpu`, `swap` (ASR resident, LLM swapped in),
  `llm-cpu`, `cpu-only`.
- **Verify** on first run: load the chosen models, transcribe a bundled 30 s pt-BR sample, generate
  a short summary; record speed factor, tok/s, swap time, peak VRAM in a machine profile.
- **Fall back** automatically on load failure (OOM) to the next layout, and tell the user.
- **Control**: Settings shows *Auto (recommended)* with the reason; *Manual* unlocks device, models,
  context, layout, live-summary cadence, language. VRAM plan updates live; *Re-run benchmark*;
  *Reset to auto*.

## Implementation

- `hardware.rs` — **probe**:
  - GPUs from `llama-server --list-devices` (lines like `Vulkan1: NVIDIA GeForce MX230 (2294 MiB,
    2234 MiB free)`). Classify **integrated** by name (Intel UHD/Iris/HD Graphics, AMD "Radeon
    Graphics"/Vega APUs, llvmpipe) and by reported memory ≥ 40 % of system RAM (shared memory).
  - whisper.cpp device index = position among **discrete** GPUs (it ignores integrated ones;
    measured: `-dev 0` = MX230 while llama.cpp calls it `Vulkan1`).
  - RAM and swap from `/proc/meminfo` (MemTotal, MemAvailable, SwapTotal, SwapFree); CPU physical
    cores and AVX2 from `/proc/cpuinfo`. Threads default = physical cores (measured: `-t 4` beats 8
    on 4c/8t).
  - **Memory pressure check**: if MemAvailable < models' RAM need + 1 GB, or swap use > 25 % of
    RAM, warn ("other programs are using most of the memory — loading will be slow").
- `planner.rs` — **plan** from `footprints.json` (shipped, measured loaded VRAM in MiB, not file
  sizes): whisper turbo-q5_0 824, small-q5_1 439; Qwen3.5-2B Q4_K_M @8K 1414 / @32K 1592 / @64K
  1828; Qwen3-1.7B @8K 1598; Qwen3.5-0.8B (to measure); runtime overhead 230; safety margin 10 %.
  Choose the first layout that fits free VRAM: `both-gpu` → `swap` → `llm-cpu` → `cpu-only`; ASR
  model preference turbo over small for pt (WER 2 % vs 12 % on the pt-BR test). Output `Plan {
  layout, asr, llm, context, devices, threads, reason }` — `reason` is shown to the user.
  Unit-tested with the MX230 probe and synthetic machines (no GPU, 4 GB, 8 GB, iGPU-only).
- `bench.rs` — **verify** on first run and on demand: load ASR, transcribe the bundled 30 s pt-BR
  sample (an MLS clip, CC BY 4.0, attributed in `ASSETS.md`), load the LLM, generate a short
  summary; record speed factor, tok/s, swap time, peak VRAM (`--list-devices` free before/after) in
  `$XDG_DATA_HOME/fofoca/profile.json`.
- **Fallback**: model-server start failure with OOM (`ErrorOutOfDeviceMemory` in its log, or exit
  before health) → mark the layout failed in the profile, move to the next one, notify the user.
- **Layouts in the session**: `swap` is today's behaviour; `both-gpu` keeps both servers loaded;
  `llm-cpu` starts llama-server with `-ngl 0`; `cpu-only` also runs whisper on CPU (warn: turbo on
  CPU is 2.5× slower than real time — pick `small` there).
- **Settings UI**: *Auto (recommended)* with the reason and measured numbers; *Manual* unlocks
  device, models, context, layout, language, chunk length; a live VRAM bar from the footprint
  table; *Re-run benchmark*; *Reset to auto*. Writes `config.toml` (keeps comments via
  `toml_edit`).
- Tauri: `probe_hardware`, `plan_setup`, `run_benchmark` (events), `save_config`.

## Known Gaps

- Non-Vulkan backends (CUDA/Metal) are out of scope until other platforms (006).
