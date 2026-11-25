Status: done
Branch: moacyrricardo/spec-023-export-meeting-build

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

## Implementation Notes

Built on `moacyrricardo/spec-023-export-meeting-build`, stacked on the spec-only PR #37.

### How the build differs from the spec
- **SRT splitting (D7):** a segment **without word timings** is split by length (84 characters) only,
  not by the 7 s rule. Its word times would be spread by character count, so a duration split would
  put words at invented times. Exporting an invented meeting with the CLI showed a 10 s two-word line
  turning into two cues. Segments with word timings follow D7 as written. Cues longer than one
  subtitle line (42 characters) break at the space nearest the middle.
- **Plan shape:** `ExportPlan.parts` is `Parts { transcript_lines, notes: Option<NotesSource>, tasks,
  audio_tracks, audio_deleted_at, imported }`, facts rather than the spec's "enabled + reason". The
  UI words the reasons ("No summary for this meeting", "No tasks for this meeting", "Deleted Nov 18",
  "No audio was kept", "Nothing was transcribed"), so the core stays free of UI English (D1). The dialog
  asks for the plan once on open and again after Choose…, with every part on. The checkboxes and the
  format switch then filter `plan.files` locally (`lib/export.ts`).
- **tasks.md** is disabled when the meeting has no action items. The spec only covered the notes
  fallback.
- **Default folder:** `dirs::document_dir()` + `fo-foca`. Without a documents folder it's `~/fo-foca`,
  not bare `~`.
- **Front matter `duration`** is a quoted clock string (`"11:04"`, `"1:02:05"`). Pause lines read
  `— pausa de N min —` in pt (`— paused N min —` otherwise), with at least 1 min; a pause that never
  resumed reads `— paused —`.
- **Zip:** it is built from the finished temp folder **before** the rename, so a cancel while zipping
  leaves nothing behind. Entries sit under `<name>/` in the archive and are dated at export time.
  The folder name is re-checked right before the rename, in case something took it meanwhile.
- **Cancel** is checked between files and every 1 MB in the copy and zip loops. Progress events go
  out at each file and every 4 MB.
- **Additions:** only one export runs at a time ("An export is already running").
  `open_export_folder` opens only folders exported in this session (and only directories), so the
  web view can't use the opener on arbitrary paths. `Jobs::is_pending(id)` was added so
  `export_meeting` can refuse a meeting with a queued or running job. App.svelte announces a
  finished or failed export when no export dialog is open (found in eval: leaving the meeting page
  lost the notice).
- **CLI:** without `--parts`, it exports every text part the meeting has and no audio (like the
  dialog). `--to` defaults to the remembered folder. Only a successful dialog export updates the
  remembered folder.
- **UI vs the mock:**
  - The native `<dialog>` backdrop also covers the sidebar.
  - The deleted-audio reason drops "(Storage setting)", because audio can also be deleted by hand.
  - The progress bar follows bytes, not steps.
  - After Choose… the remember line says "fo-foca will remember this folder for the next export."
  - `.seg` moved to `app.css` unchanged; the dialog's teal "on" state is a local override, as in the mock.
- **Preview hooks:** `fake.ts` gained a third meeting with no notes, no summary and deleted audio.
  `#export` opens the dialog on the main fake meeting and `#export-bare` on that one (preview only).

### What was verified, and where
- **Unit and end-to-end tests** (temp dirs, invented meetings only):
  - Golden strings for every renderer in pt and en.
  - The `folder_name` cases.
  - Collision `(2)` for the folder and the zip.
  - Deleted audio refused before anything is written.
  - Import audio name.
  - Zip listing (WAV stored, text deflated).
  - Audio deleted mid-export still copied.
  - Cancel while copying and while zipping leaves nothing.
  - The `[export] dir` config round-trip and fallback.
  - The Tauri refusal for the meeting being recorded or processed.
  - The TS tree, sizes, defaults, progress text and the notice.

  Final run: core 88 passed (3 ignored, as before), Tauri 5, UI 22. Clippy clean; svelte-check 0/0;
  build ✓.
- **CLI on an invented meeting** with scratch XDG dirs, not the real config or meetings: a
  full export with `--format both --zip` wrote 6 files (3.8 MB, two 1.9 MB WAVs) plus the zip.
  Re-runs gave `(2)` and `(3)` and left no temp files. The output was deleted afterwards. No real
  meeting was exported.
- **Browser preview** (`vite preview` + `fake.ts`, headless Chromium over CDP):
  - form → audio + zip + Both tree → Choose… → progress → Exported → Open folder;
  - reopen shows the remembered folder and `(2)`;
  - Cancel returns to the form;
  - the no-summary / no-tasks / deleted-audio states;
  - the toast after leaving the meeting page mid-export.

### Not verified
- No `cargo tauri dev` run, so none of the spec's hand tests ran in the real app:
  - the **native folder picker** (`pick_export_folder`);
  - real `export` events in the web view;
  - Open folder through **`tauri-plugin-opener`** (in dev, or inside the AppImage);
  - exporting while recording another meeting;
  - cancelling a large real export;
  - the folder in the Files app.
- No export of a real meeting (privacy), so there are no sizes or timings from real recordings.

### Change division
`CONTRIBUTING.md` is absent, so the split follows AGENTS.md's commit sequence:
1. the spec-only PR #37;
2. `todo → doing` on its own;
3. one commit per concern: renderers + folder naming, `[export] dir`, the export run, a labelled
   fix (SRT without word timings, zip dates), the CLI, the Tauri commands + opener, the
   behavior-neutral `.seg` move before the dialog, the dialog + button + fakes, and the eval fix;
4. this close-out.

Every commit passed the full AGENTS.md checks and was pushed right after. One PR, stacked on #37.
`Jobs::is_pending` rides in the Tauri commit, as the enabler of its one check.
