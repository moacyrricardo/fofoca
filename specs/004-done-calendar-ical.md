Status: done
Branch: moacyrricardo/spec-004-calendar-ical

# 004 — Calendar via private iCal link

## Context

Knowing the upcoming meeting lets fo-foca name recordings, offer to record when a call starts, and
suggest participant names for speakers — without OAuth or a Google Cloud app.

## Decision

- The user pastes one or more **secret iCal links** (Google "Secret address in iCal format",
  Outlook/iCloud/Fastmail equivalents) or imports a local `.ics`.
- fo-foca fetches each link every 5–15 min (read-only GET), parses with `icalendar` and expands
  recurrences with `rrule`.
- Per event: title, start/end, **call link** (`X-GOOGLE-CONFERENCE`, else a Meet/Zoom/Teams URL in
  the description), attendees (names for speaker suggestions).
- UI: "Up next" card, notification shortly before a call ("Record?"), optional auto-record, filter
  *only events with a call link*.
- **The link is a credential**: stored in the OS keyring (Secret Service on Linux), never logged;
  Settings shows only the host and a *Remove* button.

## Implementation

- `fofoca-core/src/calendar.rs`: `Calendars<S: SecretStore>` owns `calendars.json`
  (`$XDG_CONFIG_HOME/fofoca/`: sources as id + label + host/path, plus options) and the secret
  links in the keyring (service `fofoca-calendar`, one entry per source id). `add()` fetches and
  parses first, so a bad link is rejected before anything is stored; `upcoming(from, to)` merges
  all sources and returns per-calendar errors (label only, links redacted) alongside the events.
- Parsing: line unfolding, `TZID`/UTC/floating/all-day times, `RRULE` + `RDATE` + `EXDATE` via
  `rrule`, `RECURRENCE-ID` overrides drop the instance they replace. Call link from
  `X-GOOGLE-CONFERENCE`, else the first Meet / Teams / Zoom / Jitsi / Whereby URL in
  location/description/URL. Attendees from `CN` (else the e-mail local part).
- Options: `only_with_call_link` (default on), `notify_before` (default on), `auto_record`
  (default off).
- Tauri commands: `calendar_settings`, `add_calendar`, `remove_calendar`, `set_calendar_options`,
  `upcoming_events(force)` (cached 5 min), `notify` (tauri-plugin-notification).
- UI: Settings → Calendars card (add/remove, help for finding the Google link, options); "Up next"
  on Meetings (calls in the next 18 h, "Record this"); a scheduler in `App.svelte` (every 20 s)
  shows a banner + desktop notification ~1 min before a call and, if enabled, starts recording at
  the start time. Recordings started from an event store its attendees in `meta.json`
  (`MeetingMeta.attendees`), shown as "Invited:" on the meeting page.
- CLI: `fofoca-cli calendar list | add <link|path> | remove <id> | upcoming [days]`.

## Known Gaps

- Read-only: no writing notes back to calendar events.

## Implementation Notes

- **No `icalendar` crate**: a small parser of our own (unfolding + properties + params) was enough
  for Google/Outlook feeds and keeps dependencies down; `rrule` still expands recurrences. Tested
  with a fixture covering RRULE + EXDATE + RECURRENCE-ID.
- **No background polling loop**: feeds are fetched on demand (Up next every 60 s, scheduler every
  20 s) through a 5-minute cache, so the network sees at most one GET per calendar per 5 min while
  the app is open, and none when it is closed.
- **Speaker names** are not suggested yet: attendees are saved on the meeting for spec 003.
- Auto-record only works while the app is open (no tray/daemon).
- **Bug fixed before merge**: `add_calendar` / `upcoming_events` were `#[tauri::command(async)]`
  but used `reqwest::blocking`, which panics inside the tokio runtime in debug builds; the command
  never answered and the UI stayed on "Checking…". They now run in `spawn_blocking`. With adds
  running in parallel, writes to `calendars.json` are serialized, duplicate links/files are
  rejected, and ids stay unique.
- Verified by hand with a real Google secret link (add, several calendars, Up next).
