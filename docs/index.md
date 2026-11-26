<!--
  Canonical text of the public site. docs/index.html lays this out in the app's design system;
  edit both together, in place (AGENTS.md, "Public docs"). Design: specs/024-assets/github-pages-mock.html.
-->

# fo-foca

**Private meeting notes that run entirely on your machine — even on a 2 GB laptop GPU.**

Records your mic and system audio, transcribes it, and turns it into a summary and a task list.
No bot joins your call. No cloud. No account.

Badges: 🚧 Pre-alpha · 🐧 Linux x86-64 · Vulkan GPU or CPU · pt-BR first, English too · MIT

## Download

The newest build from [GitHub Releases](https://github.com/moacyrricardo/fofoca/releases):
AppImage, .deb and `fofoca-setup.sh`. A test build (tag like `v0.1.0-pr32`) carries the note:
*"Test build — no stable release yet. Models and engines are not bundled: run `fofoca-setup.sh`
first."*

Teaser (invented lines): a live transcript where *You* and *Pessoa 1/2* take turns, next to the
tasks pulled out of it.

## What it does

*Notes from your calls, without sending them anywhere.* Local meeting-notes apps usually assume a
big GPU or an Apple Silicon Mac. fo-foca starts from the other end: it is built around what a small GPU
can actually do, from measured numbers, and lets you pick the layout that fits.

- **Transcription** — records your microphone and the system audio (Meet, Zoom, Teams, anything)
  and transcribes it locally with Whisper, live, while you talk.
- **Summary** — a local LLM writes a "summary so far" during the call and a final summary at the
  end, grounded in the transcript so every claim can be checked.
- **To-dos & tasks** — action items, decisions, names, numbers and dates pulled out of the meeting.
- **100% local** — audio, transcripts and notes never leave your computer. Models run as local
  servers on your GPU or CPU.
- **Fits your hardware** — choose where each model runs (GPU, CPU, or both on a 2 GB GPU taking
  turns); a GPU memory meter shows what fits. Automatic hardware probing and benchmarking is
  *planned* (spec 002).
- **Fast and light** — a native desktop app (Rust + Tauri), not a browser in a box.

## How it works

*Two tracks in, summary and tasks out.* Your mic and the system audio are recorded on separate
tracks, so your own lines are labelled from the start. Models run as separate local servers that
fo-foca loads, unloads and swaps without restarting.

Pipeline: Mic + System (PipeWire, 2 tracks) → 30 s chunks (voice detection) → Whisper
(whisper.cpp) → Transcript (+ speakers after the call) → LLM (llama.cpp) → Summary + tasks.

**During the call**
- **Speech-to-text** with whisper.cpp (`large-v3-turbo`), in 30 s chunks.
- **You vs. them** from the separate tracks.
- **Summary so far** with a small open-weight model (Qwen3.5), on demand or every few minutes.

**After the call**
- **Speakers**: the system track is split per speaker (*Pessoa 1*, *Pessoa 2*…) by a small CPU
  model, then aligned with the transcript. Rename them; calendar names are suggested.
- **Final summary & tasks** over the full, speaker-labelled transcript.

**Small GPU? It swaps.** When both models fit in VRAM they stay loaded. When they don't, Whisper
stays on the GPU while you talk and the LLM is swapped in for summaries — about 3 seconds.
Settings shows the GPU memory each layout needs.

## Reference numbers

*Measured on a 2 GB laptop GPU* — i7-10510U (4 cores) · NVIDIA MX230 (2 GB) · Vulkan.

| What | Result |
|---|---|
| Whisper `large-v3-turbo` q5_0 on GPU | **824 MiB** VRAM · ~0.36× real time on a whole file · ~0.59× per 30 s chunk |
| pt-BR word error rate | **~2 %** (vs ~12 % for Whisper small) |
| Qwen3.5-2B Q4_K_M on GPU | **1.4–1.6 GB** VRAM (8K–32K context) · ~17 tokens/s |
| Swap Whisper ⇄ LLM | **~3 s** |
| Both models on the GPU at once | ✗ doesn't fit in 2 GB → swap layout |

## Install on Linux

Linux x86-64, test builds. Takes a few minutes plus the model download (~1.9 GB).

1. **Install the system pieces** (Debian / Ubuntu names). With an NVIDIA GPU, use the NVIDIA
   driver, which includes Vulkan.
   ```bash
   sudo apt install pipewire-bin mesa-vulkan-drivers libwebkit2gtk-4.1-0
   ```
2. **Download the models and engines** with `fofoca-setup.sh` from the release. Everything goes
   to `~/.local/share/fofoca/`, where the default config looks. Variants (a switch on the page):
   ```bash
   chmod +x fofoca-setup.sh
   ./fofoca-setup.sh                # default models (~1.9 GB) + engines
   ./fofoca-setup.sh --small        # smaller models (~1.3 GB) + engines
   ./fofoca-setup.sh --models-only  # you already have whisper-server and llama-server
   ```
   - `llama-server` (llama.cpp, Vulkan) is downloaded.
   - `whisper-server` is built from source (whisper.cpp ships no Linux build), which needs
     `sudo apt install git cmake build-essential glslc libvulkan-dev`.
3. **Run fo-foca** — AppImage or .deb (a switch on the page):
   ```bash
   chmod +x fo-foca_*.AppImage && ./fo-foca_*.AppImage
   sudo apt install ./fo-foca_*_amd64.deb
   ```
4. **Check the models.** In **Settings → Models**, check that every row is green, and pick the GPU
   for each engine.

## Status

Linux first (PipeWire); Windows and macOS are planned. Any GPU that works with Vulkan should be
usable — NVIDIA, AMD or Intel — or CPU only. Hardware reports and pt-BR test recordings are very
welcome.

Follows [`specs/CATALOG.md`](https://github.com/moacyrricardo/fofoca/blob/main/specs/CATALOG.md):
`done` → ✓ shipped, `todo` → ☐ planned, `concern` → ? exploring.

- ✓ Live transcript — mic and call on separate tracks, voice detection, adaptive chunks (001, 010)
- ✓ Summary so far, and a final summary and tasks checked against the transcript (001, 005)
- ✓ Speaker separation after the call, with names suggested from the invite (003, 022)
- ✓ After the call, mic and call transcribed separately, re-run on demand (020)
- ✓ Calendar via a private iCal link (004)
- ✓ Meeting labels, set automatically from the calendar (007, 009)
- ✓ Pause and resume a recording (010)
- ✓ Import a recording made elsewhere (012)
- ✓ Replay a meeting with its audio (011, 014)
- ✓ Settings: where each model runs (GPU / CPU / swap), model downloads (015)
- ✓ Tasks across meetings, filtered by label (016)
- ✓ Tray icon: start, stop, show / hide (017)
- ✓ Export a meeting: transcript, notes, tasks, audio (023)
- ☐ Live transcript per track during the call (018)
- ☐ Auto setup: hardware probe, layout planner, first-run benchmark (002)
- ☐ Engines bundled in the package; then Windows and macOS (006)
- ? Local MCP server so assistants can read your meetings (008)

## Footer

**fofoca** is Brazilian Portuguese for *gossip* — and **foca** means *seal*. A seal that listens
to your meetings and tells you what was said. 🦭

[GitHub](https://github.com/moacyrricardo/fofoca) ·
[Design decisions](https://github.com/moacyrricardo/fofoca/blob/main/specs/CATALOG.md) ·
[Report an issue](https://github.com/moacyrricardo/fofoca/issues) ·
[MIT license](https://github.com/moacyrricardo/fofoca/blob/main/LICENSE) ·
[Why I built it](https://iskeru.com/posts/building-fofoca/) ·
[An iskeru product](https://iskeru.com/products/#fofoca)
