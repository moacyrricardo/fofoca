# 006 — Packaging: model servers, model downloads, AppImage

## Context

001 runs against locally provided `whisper-server` / `llama-server` binaries and model files.
Users need a single install that works.

## Decision

- Build **whisper.cpp** and **llama.cpp** from pinned tags with the Vulkan backend in CI
  (whisper.cpp ships no Linux Vulkan release), and bundle them as Tauri sidecars.
- Download models on first run from Hugging Face with checksums and resumable transfers;
  show size and progress; let the user pick a models folder.
- Ship an **AppImage** first; Windows and macOS later (macOS: Metal; Windows: Vulkan/CUDA).

## Implementation

- **CI** (GitHub Actions, `ubuntu-22.04` for an old enough glibc): build **whisper.cpp** (pinned tag)
  and **llama.cpp** (pinned build, today b12527) with `-DGGML_VULKAN=ON -DBUILD_SHARED_LIBS=ON`,
  installing `libvulkan-dev`, `glslc` (shaderc) and cmake. Upload `whisper-server`,
  `llama-server` and their `.so` files (RPATH `$ORIGIN`) as an artifact.
- **Tauri sidecars**: `bundle.externalBin` for the two servers, `bundle.resources` for the shared
  libraries; `servers.rs` resolves the bundled path first, config path as override. onnxruntime
  for 003 ships via `ort`'s `download-binaries`/`copy-dylibs` (or is linked statically) — decide by
  AppImage size.
- **Models** (`models.rs` + first-run screen): a catalog (`models.json`: name, URL on Hugging Face,
  size, sha256, purpose) chosen by the 002 planner; downloads with HTTP `Range` resume, sha256
  check, progress events, and a user-selectable models folder; nothing is downloaded without the
  user pressing *Download* (sizes shown: turbo 574 MB, Qwen3.5-2B 1.18 GB, VAD 0.9 MB, diarization
  ~34 MB).
- **AppImage** via `cargo tauri build --bundles appimage`; the desktop entry and icon come from the
  bundle (replaces `scripts/install-dev-desktop-entry.sh` for installed builds). Smoke test in CI:
  start the AppImage headless with `--version`, and run `fofoca-cli transcribe` on the bundled
  sample on CPU.
- Release workflow on tag `v*`: build, attach AppImage + checksums to a GitHub release.
- Can't be built on the baseline laptop (no cmake/glslc/Vulkan headers): developed and verified in
  CI only.

## Known Gaps

- No auto-update in the first release.
