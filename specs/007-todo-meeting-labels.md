# 007 — Meeting labels

## Context

A meeting has only a title, a date and (with 004) attendees. To find "the Liatax meetings" or "my
1:1s with Fernanda" — in the app today, and through an MCP server later (concern 008) — meetings
need categories. Labels are useful on their own, so they ship first; 008 builds on them.

## Decision

- **Free-form labels, many per meeting.** No separate "project" concept.
- **Normalized on save**: trimmed, lower-case, inner whitespace → `-`, max 32 characters, no
  duplicates per meeting (`"Cliente BH"` → `cliente-bh`). Display as entered after normalization.
- **Stored in the meeting's `meta.json`** as `labels: Vec<String>`; plain files stay the source of
  truth. The set of known labels is derived by scanning meetings (no separate registry).
- **Where labels are set**
  - On the meeting page: add (with autocomplete from known labels) and remove.
  - When starting a recording: optional label chips next to the title.
  - **Per calendar default labels** (004): each calendar source can carry `default_labels`, applied
    when recording from one of its events ("Record this" or auto-record). E.g. the work calendar →
    `trabalho`.
- **Where labels are used**: the Meetings list gets a label filter (chips with counts); each row
  shows its labels.

## Implementation

- `fofoca-core`
  - `model.rs`: `MeetingMeta.labels: Vec<String>` (`#[serde(default)]`, older meetings load fine).
  - `labels.rs`: `normalize(&str) -> Option<String>`, unit-tested (accents kept, `"  A  b "` →
    `a-b`, empty → `None`, length cap).
  - `store.rs`: `set_labels(id, &[String])` (normalizes, dedups, rewrites `meta.json` atomically),
    `known_labels() -> Vec<(String, usize)>` (label + meeting count, sorted by count), and
    `list()` accepting an optional label filter.
  - `Store::create` / `Session::start` take initial labels.
  - `calendar.rs`: `CalendarSource.default_labels: Vec<String>` (`#[serde(default)]`) + a setter.
- Tauri commands: `set_meeting_labels(id, labels)`, `known_labels()`,
  `set_calendar_default_labels(id, labels)`; `start_session` gains `labels`.
- UI: label chips + input with autocomplete (meeting page, start form, calendar source row),
  filter chips on Meetings.
- CLI: `fofoca-cli labels` (known labels), `fofoca-cli label <meeting-id> add|rm <label>`,
  `fofoca-cli list --label <label>`.

## Known Gaps

- Suggesting labels automatically (from attendees' past meetings or the summary model) — later.
- Renaming / merging a label across all meetings — later (can be done by hand via the CLI).
- MCP exposure and filtering by label there → concern **008**.
