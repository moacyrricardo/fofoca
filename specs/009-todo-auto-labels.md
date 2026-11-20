# 009 — Automatic labels from calendars and recurring meetings

## Context

Labels (007) are only useful if they're applied consistently, and typing them for every recording
is the step people skip. Two cheap signals cover most meetings: **which calendar** the event came
from (work vs personal) and **which recurring series** it belongs to (the weekly 1:1 is labelled
the same way every week).

A label per call service (`meet`, `zoom`) was considered and rejected: almost every recording has
a call link, so it would tag nearly everything and crowd the filter chips without narrowing
anything.

## Decision

- **Calendar name as the default label.** When a calendar is added, its `default_labels` start as
  `[normalize(calendar name)]` (e.g. "Trabalho" → `trabalho`). Still editable/removable in
  Settings → Calendars. Existing calendars are not changed.
- **Recurring series inherit labels.** A meeting recorded from a calendar event stores the event's
  `UID`. When recording another event with the same UID, the labels of the **most recent** meeting
  with that UID are added. Google keeps one UID per recurring series (moved instances included),
  so "1:1 Fernanda" every Monday shares it.
- Labels applied at start = `calendar default_labels ∪ series labels`, normalized and deduped.
  The user can still edit them while recording or afterwards (007).
- **Up next shows them**: each upcoming event lists the label chips it will get, so nothing is
  applied silently.

## Implementation

- `fofoca-core`
  - `model.rs`: `MeetingMeta.calendar_uid: Option<String>` (`#[serde(default, skip_serializing_if
    = "Option::is_none")]`).
  - `store.rs`: `create(title, attendees, labels, calendar_uid)`; `labels_for_series(uid) ->
    Vec<String>` — labels of the newest meeting (by `started_at`) with that `calendar_uid`, empty
    if none.
  - `calendar.rs`: in `add()`, `default_labels = labels::normalize_all(&[label])`. Unit test.
  - `session.rs`: `Session::start` takes a `StartInfo { title, attendees, labels, calendar_uid }`
    struct instead of growing positional arguments (CLI and Tauri updated).
- Tauri
  - `start_session(title, attendees?, labels?, calendar_uid?)`: when `calendar_uid` is set, merge
    `store.labels_for_series(uid)` into `labels` before starting.
  - `upcoming_events`: fill each `CalendarEvent.labels` with `default_labels ∪
    labels_for_series(uid)` so the UI shows exactly what will be applied (one `store.list` scan per
    refresh, cached with the events).
- UI
  - `UpNext.svelte`: label chips under each event; `onrecord(title, attendees, labels, uid)`.
  - `App.svelte`: heads-up banner and auto-record pass `e.uid`.
- Tests: store `labels_for_series` picks the newest meeting; calendar add pre-fills the label;
  start merges calendar + series labels without duplicates.

## Known Gaps

- Rules by attendee e-mail domain (`@client.com` → `client`) — later; needs a small rules editor.
- No back-fill: meetings recorded before 009 have no `calendar_uid`, so series inheritance starts
  with the next recording.
