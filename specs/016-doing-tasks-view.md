# 016 — Tasks: every meeting's next steps in one list

## Context

Checked notes (005) pull action items out of each meeting — text, owner, due date, and the
transcript lines they came from — but they're only visible meeting by meeting, on its Tasks tab.
To see "what did I agree to this week" you open every meeting. Meetings already carry labels
(007, 009), which is how the user groups them (work, a client, a 1:1).

## Decision

- A **Tasks** screen in the sidebar (between Meetings and Record) listing the action items of
  every meeting that has checked notes.
- **Label filter** with the meetings' labels, like the Meetings list: pick one or more labels →
  tasks from meetings carrying all of them. Tasks have **no labels of their own** — a task is
  filtered by the labels of the meeting it came from (by decision: no per-task tagging).
- Two views: **By meeting** (newest meeting first; the meeting's title, date and labels as the
  group header) and **By due date** (overdue first, then by date, then tasks without a date).
- Each task shows its text, owner and due date (*overdue* in coral when the date has passed).
  Clicking a task opens its meeting on the **Tasks** tab, where the quotes and time chips are.
- Read-only: no done/undone state, no editing (the notes are the record of the meeting).
- Empty states: no meeting has checked notes yet; the filter matches no tasks.

## Implementation

- **Core** (`store.rs`): `Store::tasks()` → every meeting's `notes.action_items` with the
  meeting's id, title, start time and labels (`TaskItem`), newest meeting first; meetings without
  `notes.json` are skipped.
- **Tauri**: `list_tasks` command. Fake backend: tasks from the sample meetings.
- **UI**: `screens/Tasks.svelte`; grouping, sorting, overdue and the label filter as pure
  functions in `lib/tasks.ts` (tested with `npm test`). `MeetingView` takes an optional initial
  tab so a task opens the meeting on Tasks. The filter chips reuse the Meetings list style.
- **Tests**: Rust — `tasks()` collects items with their meeting and skips meetings without notes.
  TS — label filter (all selected labels), by-meeting grouping, due-date order with overdue first.

## Known Gaps

- No done state: a task stays listed after it's done (by decision for now).
- Due dates are whatever the notes extracted (`YYYY-MM-DD`); relative dates the model couldn't
  resolve stay empty.
