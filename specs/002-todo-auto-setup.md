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

To be detailed when picked up: `hardware.rs` (probe), `planner.rs` (footprint table + layout
choice), `bench.rs` (first-run verify), profile at `$XDG_DATA_HOME/fofoca/profile.json`,
settings editing in the UI.

## Known Gaps

- Non-Vulkan backends (CUDA/Metal) are out of scope until other platforms (006).
