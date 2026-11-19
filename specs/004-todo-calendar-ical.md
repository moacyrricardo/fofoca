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

To be detailed when picked up.

## Known Gaps

- Read-only: no writing notes back to calendar events.
