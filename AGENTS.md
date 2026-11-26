# AGENTS.md

Working rules for anyone (human or coding agent) changing fo-foca. The README says what the app is;
this file says how work is done here.

## Layout

- `crates/fofoca-core/` — the Rust core, Tauri-free: capture, model servers, transcription,
  speakers, notes, store, CLI (`src/bin/fofoca-cli.rs`).
- `app/src-tauri/` — Tauri commands over the core. `app/src/` — Svelte 5 + TS UI.
- `app/src/lib/*.ts` — pure UI logic with **type-only imports**, so `npm test` runs it under plain
  Node. Keep logic there, not in `.svelte` files.
- `app/src/lib/fake.ts` — a fake backend that mirrors every Tauri command; new commands and events
  get a fake too.
- `specs/` — the decision catalog (below). `scripts/fofoca-setup.sh` — models and engines.

## Checks (run all before every commit)

```bash
cargo test -p fofoca-core
cargo clippy --workspace --all-targets        # must be warning-free
cd app && npm run check && npm test && npm run build
```

Tests that need real models or real audio are `#[ignore]` and take their inputs from env vars.

## Running it

- Desktop app: `cd app && npm install && cargo tauri dev`.
- UI without the backend: `cd app && npm run build && npx vite preview` — the fake backend serves
  every screen (hash routes such as `#settings`, `#meeting`).
- Headless: `cargo run -p fofoca-core --bin fofoca-cli -- <command>` (`transcribe`, `record`,
  `import`, `retranscribe`, `transcript`, `diarize`, `notes`, …; the full list is the `match` in
  `main()` of `fofoca-cli.rs` — the usage line doesn't name them all).

## Specs and process

- Every feature starts as a spec in `specs/NNN-status-slug.md`; `specs/CATALOG.md` indexes them.
  A **concern** explores options; a **spec** is a decision, prescriptive enough to build from.
  Status (`todo` → `doing` → `done`, or `concern`) changes by renaming the file; numbers are
  permanent. A resolved concern gets a WARNING pointing to its spec. Reviewers leave `#comment:`
  lines in specs to be acted on and removed.
- One branch and one PR per spec: `moacyrricardo/spec-NNN-slug`. PRs may be **stacked** on each
  other; don't wait for merges.
- Commit sequence per spec: (1) the spec as `todo` (spec-only PR), (2) the `todo → doing` rename
  on its own, (3) the feature commit(s), (4) close-out: rename to `done`, `Status`/`Branch`
  header, **Implementation Notes** (how the build differed, what was measured, what is *not*
  verified yet), CATALOG row.
- Commit subjects start `spec-NNN …`. One logical change per commit, compiling at every commit;
  renames get their own commit; a fix gets its own commit labelled with the spec that introduced
  the bug. Stage explicit paths (no `git add -A` / `git add .`).
- UI changes start from a clickable mock in `specs/NNN-assets/`, a facsimile of the real screen
  with only the change applied.
- Measure before claiming: numbers in specs come from the real binaries on the reference laptop;
  anything only checked in the browser preview is said so.

## Public docs

`docs/` is the project site, served by GitHub Pages from `main` (spec 024):
`docs/index.md` is its text, `docs/index.html` lays it out in the design system below, and
`docs/assets/` holds copies of the logo and icons (`assets/` is the source).

- **The roadmap follows `specs/CATALOG.md`.** A spec's close-out commit updates, in the same commit:
  - README "Status & roadmap" (`done` checked, `todo` open, concern *exploring*),
  - the Status list in `docs/index.md` and `docs/index.html`,
  - any feature text that claimed the spec as planned.
- **Install steps and release assets change together.** A change to `scripts/fofoca-setup.sh`
  flags, system packages, or release file names updates README "Install" and the Install
  section of both `docs/` files.
- **Edit `docs/index.html` in place**, next to the same edit in `docs/index.md`. Regenerate it from
  the `.md` (with `/rich-html`) only for a redesign, and start that from a new mock.
- The release card reads the GitHub releases API in the browser. Nothing in `docs/` needs editing
  when a release is published.

## Reference hardware

Tuned for a 4-core laptop with a **2 GB NVIDIA MX230**: Whisper large-v3-turbo and the notes LLM
don't fit on the GPU together (see spec 015 for the layouts, CPU-only included). Changes that add
GPU work say how they fit that budget.

## Privacy

Meetings hold other people's voices. Real meeting data in `~/.local/share/fofoca/meetings/` is
**read in place, never copied** into scratch folders, tests or the repo. Never put transcript text,
names, meeting ids or personal local paths into commits, code, tests, specs or PR text — report
counts and timings, and use invented lines for examples.

## Design system

The palette is the CSS tokens in `app/src/app.css`: navy surfaces `--navy` / `--navy-2` /
`--navy-3`, lines `--line`, ink `--ink` / `--ink-2` / `--ink-3`, accent `--teal`, highlights
`--coral` / `--peach`, states `--ok` / `--warn` / `--bad`, the rounded `--font` stack, 14 px card
radius. The app is **dark-only**; documents and reports built from these tokens derive a matching
light variant when they need one.
