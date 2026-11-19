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

To be detailed when picked up. Build deps on Linux: cmake, glslc (shaderc), Vulkan headers.

## Known Gaps

- No auto-update in the first release.
