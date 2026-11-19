Status: doing
Branch: moacyrricardo/spec-001-walking-skeleton

# 001 — Walking skeleton: record, transcribe live, summarize on demand

> **Design reference:** [`001-assets/ui-mock.html`](001-assets/ui-mock.html) (source of truth for look & flow) ·
> **Evidence:** [`001-assets/benchmarks-mx230.html`](001-assets/benchmarks-mx230.html)

## Context

fo-foca is a local meeting-notes app that must run well on small GPUs. Benchmarks on the baseline
machine (i7-10510U, NVIDIA MX230 2 GB, Vulkan) established:

- **Whisper `large-v3-turbo` q5_0** on GPU is the transcription model: 824 MiB VRAM, ~2 % WER on
  pt-BR read speech with slang, ~0.59× real time per 30 s chunk (keeps up live).
- **Qwen3.5-2B Q4_K_M** (llama.cpp) is the summary model: 1.4–1.6 GB VRAM, ~17 tok/s on GPU.
- Both do **not** fit in 2 GB together. Swapping them on the GPU costs ~3 s. Running the LLM on CPU
  was 2–3× slower than swapping in the chunk experiment.
- Summarising after *every* chunk cannot hold the 30 s budget; transcription alone can.

This spec builds the thinnest end-to-end slice that proves the architecture on real hardware.

## Decision

1. **Desktop app = Tauri 2** with a **Svelte 5 + Vite + TypeScript** UI. All logic lives in a
   Tauri-free Rust library crate so it can be tested headless and driven by a CLI.
2. **Models run as child processes** — `whisper-server` (whisper.cpp) and `llama-server`
   (llama.cpp) — talked to over HTTP on `127.0.0.1`. fo-foca starts, stops and swaps them.
3. **Layout for 001 is fixed to _swap_**: Whisper resident on GPU during the call; LLM swapped in
   only for summaries. Choosing the layout automatically is spec 002.
4. **Audio capture via PipeWire CLI**: two `pw-record` processes (mic = default source; system =
   default sink monitor), raw `s16le` 16 kHz mono on stdout. Both tracks are saved separately.
5. **One transcription request per chunk, attribution by energy**: the two tracks are **mixed**
   into one 30 s chunk for Whisper (two requests per chunk would cost ~35 s > 30 s). Each
   transcribed segment is labelled **You** or **Call** by comparing mic vs system RMS energy over
   the segment's time range. Per-speaker separation of the call track is spec 003.
6. **Word timestamps are kept**: `verbose_json` word timings, shifted by the chunk offset to
   meeting time, are stored with the transcript (needed by 003 and 005).
7. **Summaries on demand only** in 001 (button + final summary at Stop). Periodic cadence is a
   setting added later.
8. **Storage = plain files** per meeting under `$XDG_DATA_HOME/fofoca/meetings/<id>/`.

## Implementation

### Repository layout

```
Cargo.toml                 workspace
crates/fofoca-core/        library + `fofoca-cli` binary
  src/config.rs            config file (paths, devices, language)
  src/servers.rs           child-process manager for whisper-server / llama-server
  src/capture.rs           pw-record tracks → sample channels
  src/audio.rs             mixing, RMS, WAV writing, chunking
  src/whisper.rs           /inference client (verbose_json → segments + words)
  src/llm.rs               /v1/chat/completions client (thinking off)
  src/session.rs           live session: capture → chunks → transcript events; swap for summary
  src/store.rs             meeting folders (meta.json, transcript.jsonl, summaries.json, *.wav)
  src/bin/fofoca-cli.rs    transcribe <wav> | summarize <meeting> | record --seconds N
app/                       Svelte UI (Vite)
app/src-tauri/             Tauri shell: commands + event bridge to fofoca-core
```

### Config (`$XDG_CONFIG_HOME/fofoca/config.toml`, created with defaults on first run)

```toml
language = "pt"
chunk_seconds = 30
[whisper]
server = "~/.local/share/fofoca/bin/whisper/whisper-server"
model  = "~/.local/share/fofoca/models/ggml-large-v3-turbo-q5_0.bin"
device = 0              # whisper.cpp counts discrete GPUs only
threads = 4
[llm]
server = "<path>/llama-server"
model  = "<path>/Qwen3.5-2B-Q4_K_M.gguf"
device = "Vulkan1"
context = 32768
threads = 4
```

### Servers (`servers.rs`)

- `ModelServer::start(kind)` picks a free port, spawns the binary with the benchmarked flags
  (whisper: `-m -dev -t --host 127.0.0.1 --port`; llama: `-m -dev -ngl 99 -t -c -ctk q8_0 -ctv q8_0
  -fa on --jinja --reasoning off`), polls readiness (`GET /` for whisper, `GET /health` for llama),
  and kills the child on `Drop`.
- `ModelServer::stop()` kills and waits; the caller treats VRAM as free after the process exits.

### Live session (`session.rs`)

- `Session::start(store, config)` → creates the meeting folder, starts whisper-server and both
  captures, spawns a worker thread.
- Every `chunk_seconds` of audio: append raw tracks to `mic.wav` / `system.wav`, mix to mono,
  POST to whisper (`language`, `response_format=verbose_json`, `prompt` = last ~200 chars of
  transcript), convert to `Segment { start, end, text, track: You|Call, words: [Word{start,end,text,p}] }`,
  append to `transcript.jsonl`, emit `SessionEvent::Segment`.
- Chunks queue while transcription is busy; `SessionEvent::Status { backlog_s, models }` reports it.
- `Session::summarize()` → stop whisper, start llama, send the accumulated transcript with the
  partial-summary prompt, emit `SessionEvent::Summary`, stop llama, restart whisper, drain backlog.
- `Session::stop()` → stop capture, flush last partial chunk, final summary (same swap), write
  `meta.json` (`title`, `started_at`, `duration_s`), emit `SessionEvent::Finished`.

### UI (port of the mock)

- **Meetings**: list from `store::list()`; *Start recording*.
- **Live**: timer, mic/call levels, streaming segments (You/Call), *Summary so far* with *Update
  now* showing the swap state, model chips, backlog, *Stop & summarize*.
- **After the call**: progress steps (finish transcription → swap → final summary).
- **Meeting notes**: Summary and Transcript tabs (Tasks/Speakers tabs hidden until 003/005).
- **Settings**: read-only view of the config and detected devices (editing is 002).

### Tauri bridge

Commands: `list_meetings`, `get_meeting(id)`, `start_session(title)`, `summarize_now`,
`stop_session`, `get_config`. Session events are forwarded as the Tauri event `session`.

### Verification

- `cargo test -p fofoca-core` (WAV/mix/RMS/attribution/whisper-JSON parsing units).
- `fofoca-cli transcribe <ptbr.wav>` reproduces the benchmark transcript.
- `fofoca-cli record --seconds 60` produces a meeting folder with both tracks and a transcript.
- `cargo tauri dev` runs the UI end-to-end on the baseline machine.

## Known Gaps

- Auto setup / hardware probe / overrides → **002**.
- Speaker separation of the call track after the call → **003**.
- Calendar via private iCal link → **004**.
- Grounded final summary with citations + task extraction → **005**.
- Shipping whisper.cpp / llama.cpp binaries and downloading models → **006**. Until then the
  config points at locally provided binaries and models.
- Linux/PipeWire only.
