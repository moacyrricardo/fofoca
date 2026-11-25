# 023 — Export a meeting to a folder

> **Design reference (source of truth for the UI):** [`023-assets/export-mock.html`](023-assets/export-mock.html).
> It's a clickable facsimile of the meeting page with the Export dialog. Its "ⓘ annotations" mark what's
> invented. Open it with `#export` to land on the open dialog.

## Context

A meeting's content lives only inside fo-foca: the transcript, summary, checked notes (005), tasks
(016) and audio (`~/.local/share/fofoca/meetings/<id>/`). There is no way to hand it to someone,
archive it, or open it in another tool (a notes app, a to-do app, a subtitle editor) without
copying internal files by hand.

What exists today (read-only gap analysis of `main` `d0a9772`):
- **All data loads with one call**: `Store::load` (`store.rs:266-274`) returns meta, the saved
  transcript, summaries, speakers and checked notes. Audio comes from `Store::audio_tracks`
  (`store.rs:257-264`).
- **Text renderers exist only for other purposes:**
  - `prompts::transcript_text` (`prompts.rs:10-16`), the LLM prompt format, with `mmss` showing 75:10
    past an hour;
  - `notes::to_markdown` (`notes.rs:105-127`), which uses bold pseudo-headings and leaves out flagged
    items.
- **Missing:** an SRT writer (`whisper.rs` only parses VTT), a zip crate, a folder picker (the dialog
  plugin is used from Rust, but only to pick files), any way to open a folder, and a safe-filename
  helper. Default titles look like `Meeting 20/11 14:02` (`store.rs:83`), so a `/` would create
  nested folders.

## Decision

**Settled before the decisions below** (gap analysis):
- Export is core code, `fofoca_core::export`, called by a Tauri command and by
  `fofoca-cli export`. The web view never touches the filesystem, so the capabilities and CSP don't
  change.
- **Layout:** `<dest>/<YYYY-MM-DD Title>/` holding `transcript.md` and/or `transcript.srt`,
  `notes.md`, `tasks.md` and `audio/…`. An optional `<YYYY-MM-DD Title>.zip` sits next to it.
- **It runs on its own thread** with progress events, like `download_model`, **not in the GPU job
  queue**. That means it works while another meeting is recording.
  - For **the meeting being recorded**, export is refused (its WAVs are still growing).
  - While a meeting is **processing**, the Export button is disabled, as the other actions are.
  - The Audio part is disabled, with the reason, when the meeting has no audio track (011).
- **Safety:**
  - It writes into a temp folder in the destination, then renames it at the end, so a failure
    leaves no half folder.
  - The WAV files are opened before anything is written, so deletion by 011 during the export
    can't break the copy.
  - The meeting is loaded once, at the start, and rendered from that snapshot.

**The user's decisions (2026-11-25):**

1. **Language (D1·A):** the files follow the transcription language (`cfg.language`), as
   `speakers::display_name` and `notes::to_markdown` already do. pt gives Você / Chamada / Pessoa N
   and Resumo / Decisões / Próximos passos / Não verificado; anything else gives the English words.
2. **notes.md without checked notes (D2·A):**
   - If `notes.json` exists, render it.
   - Otherwise, write the latest final summary's text under the same header.
   - If neither exists, the "Summary & notes" checkbox is disabled with *"No summary for this
     meeting"*.
   - Partial summaries are never exported.
3. **Flagged items (D3·A):** notes.md ends with a "Couldn't verify" section (localized per 1): each
   flagged item with its reason and times.
4. **Next steps (D4·A):** notes.md keeps its Next steps section. tasks.md lists the same items as a
   checklist (`- [ ] text — owner · due [time]`), with no transcript quotes. Every item is unchecked,
   because tasks have no "done" state.
5. **Header (D5·A):** every `.md` starts with YAML front matter: `title`, `date` (ISO, with the
   meeting's own offset), `duration`, `attendees`, `labels`, plus `model` in notes.md. Then a
   `# Title` heading. transcript.srt has no header.
6. **transcript.md (D6·A):** one line per segment,
   `**[h:mm:ss] Speaker:** text` (`[mm:ss]` under an hour). A `— paused N min —` line marks each
   recording pause (010).
7. **SRT (D7·A):** each cue starts with `Speaker: `. A segment longer than 2 lines (~84 characters) or
   7 s is split into several cues at its word timings. Overlapping You/Call cues are kept as they are.
8. **Source (D8·A):** export reads the **saved** transcript, exactly what the meeting page shows. The
   dialog offers no choice (020 keeps the live transcript hidden).
9. **Remembered folder (D9·A):** `[export] dir` in config.toml, saved with toml_edit like
   `[workflow]`, and exposed through the config view. The default is the documents folder
   (`dirs::document_dir()`, which is localized, e.g. `~/Documentos`) plus `fo-foca`, falling back to
   home. A remembered folder that no longer exists falls back to the default when the dialog opens.
10. **Existing folder (D10·A):** add a number suffix, `2026-11-20 Title (2)/`, `(3)`…. The zip
    follows the folder name.
11. **Zip (D11·A):** when ticked, write the folder, then `<name>.zip` next to it. Text files are
    deflated; WAV files are stored uncompressed.
12. **Audio (D12·A):** WAV only, copied as stored.
13. **Open folder (D13·A):** `tauri-plugin-opener`, called from Rust (no new UI permission).
14. **Audio names (D14·B):** recordings export `audio/mic.wav` and `audio/call.wav` (stored as
    `system.wav`). **Imports** export their single track under the **original file name**,
    `audio/<sanitized imported_from stem>.wav`.
15. **Cancel (D15·A):** the working view has Cancel. It stops the export and deletes the temp folder
    and any partial zip.

## Implementation

### 1. Core `export.rs` (new, Tauri-free)

```rust
pub struct ExportOptions { pub transcript: Option<TranscriptFormat>, pub notes: bool, pub tasks: bool,
                           pub audio: bool, pub zip: bool, pub dest: PathBuf }
pub enum TranscriptFormat { Markdown, Srt, Both }
pub struct ExportPlan { pub folder: PathBuf, pub files: Vec<PlannedFile>, pub zip: Option<PathBuf>,
                        pub parts: PartsAvailability /* notes/audio enabled + reason */ }
pub fn plan(store: &Store, cfg: &Config, id: &str, opts: &ExportOptions) -> Result<ExportPlan>;
pub fn run(store: &Store, cfg: &Config, id: &str, opts: &ExportOptions,
           progress: &dyn Fn(ExportProgress), cancel: &AtomicBool) -> Result<ExportDone>;
```
- **Renderers**, each a pure function of the loaded `Meeting` plus language:
  - `front_matter`
  - `transcript_md` (pause lines from `meta.pauses`; `h:mm:ss` time)
  - `transcript_srt` (`srt_time` → `HH:MM:SS,mmm`; cues split at `Segment.words`; numbering)
  - `notes_md` (structured notes, or the final-summary fallback; flagged section)
  - `tasks_md`
- **`folder_name(meta)`:**
  - the date from `started_at` in its own offset;
  - the title with `/ \ : * ? " < > |` and control characters replaced, dots and spaces trimmed,
    accents kept, capped at 200 bytes on a character boundary;
  - an empty title becomes "Meeting".
  - The collision suffix follows decision 10.
- **Audio:**
  - Open the handles first, then `io::copy`.
  - Recordings write `mic.wav` / `call.wav`.
  - Imports write `<stem>.wav` from `imported_from`, sanitized like titles.
  - Per-file sizes go into the plan.
- **Zip:** the `zip` crate (default features off, `deflate`). Stream the files from the finished
  folder, using `Stored` for `.wav` and `Deflated` for the rest. Write to `<name>.zip.part`, then
  rename.
- **Temp folder:** `<dest>/.<name>.fofoca-export-<pid>/`, renamed to the final name at the end.
  Check `cancel` between files and every few MB inside the copy loop. On cancel or error, remove the
  temp folder and the `.part` zip.

### 2. Config (`config.rs`)

- `ExportConfig { dir: Option<String> }` under `[export]`, `#[serde(default)]`.
- `Config::save_export_dir(path)` with the existing toml_edit helpers. `~` is expanded on read and
  collapsed for display (add `collapse_home`).

### 3. Tauri (`app/src-tauri`)

- Add `tauri-plugin-opener` and register it.
- Commands:
  - `pick_export_folder(start)` uses the dialog plugin's folder picker;
  - `export_plan(id, options)` returns the plan, so the dialog shows real sizes, the sanitized
    folder name and which parts are available;
  - `export_meeting(id, options)` spawns a thread, emits `export` events (step, file, bytes done and
    total, done or failed with the path), and saves the destination as the remembered folder on
    success;
  - `cancel_export()`;
  - `open_export_folder(path)`.
- `export_meeting` refuses the meeting currently recording and any meeting with a queued or running
  job (the same checks as `delete_audio` / `check_retranscribe`).
- `ConfigView` carries `export_dir`.

### 4. UI (`app/src`)

- `screens/ExportDialog.svelte` with the form, working and done states, following the mock:
  - the part checkboxes with sizes;
  - the Markdown · SRT · Both switch;
  - the destination and Choose…;
  - "Also make a .zip";
  - the "You'll get" tree;
  - Export, then progress with Cancel, then "Exported" with Open folder.

  Use the native `<dialog>` + `showModal()` pattern from the quit dialog (`App.svelte`).
- `MeetingView.svelte` gets an **⤓ Export** button next to the title. It's disabled while the meeting
  is processing or being recorded.
- Promote the `.seg` segmented control from `Tasks.svelte` to `app.css`.
- `lib/export.ts`: the tree preview and size formatting, as pure helpers, plus `export.test.ts`.
  `types.ts` and `api.ts` get the commands and `onExport`. `fake.ts` gets every command plus a
  scripted progress sequence.

### 5. CLI (`fofoca-cli`)

`fofoca-cli export <id> [--to DIR] [--parts transcript,notes,tasks,audio] [--format md|srt|both]
[--zip]` calls the same `export::run` and prints progress. Fix the stale usage string.

### 6. Tests and checks

- Core golden-string tests for each renderer, in pt and en, including:
  - the notes fallback to the final summary;
  - the flagged section;
  - a transcript past an hour;
  - a pause line;
  - SRT splitting and numbering.
- `folder_name` cases: `Meeting 20/11 14:02`, `1:1 Fernanda`, accents, a 300-byte title, an empty
  title.
- An end-to-end export into a temp dir, covering collision `(2)`, missing audio, an import's audio
  name, the zip listing (WAV stored), and cancel cleaning up.
- A config round-trip for `[export] dir`. TS tests for the tree helper.
- Checks (AGENTS.md): `cargo test -p fofoca-core`, `cargo clippy --workspace --all-targets` (no
  warnings), and in `app/`: `npm run check && npm test && npm run build`. Also run
  `cargo test -p fofoca` for the Tauri-side tests.
- Hand tests in `cargo tauri dev`:
  - a meeting with audio, exported with a zip, then Open folder;
  - deleted audio;
  - an import;
  - a meeting with a `/` in its title;
  - exporting while recording another meeting;
  - cancelling a large export;
  - the folder in the Files app.

## Known Gaps

- **Compressed audio** (Opus via ffmpeg) isn't offered; exports are WAV only (D12).
- **The live transcript** can only be exported via `fofoca-cli transcript <id> show live` (D8).
- **Tasks have no "done" state**, so every checklist item is unchecked.
- **Double disk use with a zip**: the folder and the zip both hold the audio (D11).
- **Re-exports pile up** as `(2)`, `(3)`… folders (D10).
- The language follows the transcription, not the UI (D1). It's revisited if the UI gets
  translated.
- **Not verified:** `tauri-plugin-opener` inside the AppImage; that's a hand test.
