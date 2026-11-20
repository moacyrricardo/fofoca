Status: doing
Branch: moacyrricardo/spec-017-tray-and-icon-build

# 017 — Tray menu, and an icon without white corners

## Context

- **White corners.** The app icon shows a white square behind the rounded seal tile (dock,
  alt-tab, window). The source `app/src-tauri/app-icon.png` is RGB with no alpha channel and a
  near-white margin around the rounded tile; every icon generated from it (`icons/*.png`,
  `icon.ico`, `icon.icns`) has opaque corners — measured: corner pixel `(255, 255, 253, 255)` in
  `icon.png`, `128x128@2x.png` and `32x32.png`.
- **Tray.** Recording is controlled only from the window. During a call the window is usually
  out of the way, and there's no glanceable sign that fo-foca is recording. A tray icon with a
  menu lets the user start/stop a recording and show/hide the window without switching to it.
  On this machine (Ubuntu 26.04, GNOME) the tray works through AppIndicator:
  `libayatana-appindicator3` is installed and the *Ubuntu AppIndicators* extension is enabled.

## Decision

**Icon**
- Make everything outside the rounded tile transparent in the source, keeping the tile's
  anti-aliased edge, and regenerate every icon from it. Same artwork, no white corners, in the
  dock, alt-tab, the window icon and the bundles.

**Tray icon**
- Always present while fo-foca runs. Two images:
  - **Idle**: the seal tile (the app icon at tray size).
  - **Recording**: the same tile with a coral recording dot in the bottom-right corner (the
    sidebar's live dot colour, `#fd8f6e`, with a dark outline so it reads on light and dark
    panels). **Paused** uses the idle image (nothing is being recorded).
- Tooltip / title: "fo-foca — ready", "fo-foca — recording 12:34", "fo-foca — paused".

**Tray menu** (labels change with the state)
| Item | Idle | Recording / paused |
|---|---|---|
| Recording | **● Start recording** | **■ Stop recording** (and summarise if checked notes are on in Settings → Processing, spec 015) |
| Pause | — | **❚❚ Pause** / **▶ Resume** |
| Window | **Hide window** / **Show window** (whichever applies) | same |
| — | separator | separator |
| Quit | **Quit fo-foca** | **Quit fo-foca** — asks in the window first: "A recording is running. Stop and save it, or keep recording?" |

- **Start recording** from the tray starts the same session as the Record screen. If a calendar
  event is starting now (the one the "is starting" banner would show), it's used for the title,
  invitees and labels, as the banner's ● Record does; otherwise an untitled meeting ("Meeting
  dd/mm hh:mm", as today). The window doesn't pop up; the UI switches to the Record screen in
  the background so it's there when shown.
- **Stop recording** behaves like the Record screen's stop: it shows the Processing screen in
  the window and follows the after-call steps from Settings.
- **Show window** raises and focuses it; **Hide window** hides it (the app keeps running and
  recording). The window's own close button keeps today's behaviour (quits) — closing to the
  tray is a separate decision (see Known Gaps).
- A recording started or stopped in the window updates the tray immediately, and vice versa.

## Implementation

- **Icon**: a script `scripts/make-icon.py` (Pillow) — flood-fill the near-white margin from
  the four corners to alpha 0, feather the tile edge by 1 px, save `app-icon.png` as RGBA — then
  `cargo tauri icon app-icon.png` to regenerate `icons/`. Commit the script so the icon can be
  rebuilt from new artwork. Check: every generated PNG has alpha 0 at its corners.
- **Tray icons**: `icons/tray-idle.png` and `icons/tray-recording.png` (64×64, RGBA), generated
  by the same script from the fixed source (recording = idle + dot); embedded with
  `include_bytes!` like the window icon.
- **Tauri**: `tauri` feature `tray-icon`. A `tray.rs` module in `app/src-tauri`:
  - `TrayIconBuilder` with the idle image, the menu and a tooltip; built in `setup`.
  - `MenuItem`s kept in state so labels/enabled flags can be updated; `set_icon` /
    `set_tooltip` on state changes.
  - Menu events call the same code as the commands: `start_session` is split into
    `start_recording(app, StartInfo)` (used by the command and the tray); stop/pause/resume
    call the session directly.
  - "Start" with a current calendar event: reuse `fetch_upcoming` (cached) and pick the event
    that started in the last 2 minutes or starts within 1 minute.
  - State changes come from one place: the session event sink (`Started`, `Paused`,
    `Resumed`, `Finished`, `Discarded`) updates the tray, so window- and tray-started
    recordings behave the same. The elapsed time in the tooltip updates from `Status` events
    (at most once per second).
  - Window show/hide: `get_webview_window("main")` → `show()` + `set_focus()` / `hide()`; the
    menu label follows `is_visible()`, refreshed when the menu opens and on window events.
  - Quit while recording: emit a `quit-requested` event; the window shows the question (it is
    shown first if hidden). "Stop and save" stops the session and exits once it has finished;
    "Keep recording" cancels.
- **UI**: `App.svelte` listens for `quit-requested` (a small dialog like Discard's). Nothing
  else changes — tray-started sessions arrive as the usual `started` event.
- **Linux notes**: AppIndicator ignores left-click events, so everything is in the menu (no
  click-to-toggle). Without an AppIndicator host (plain GNOME without the extension) the tray
  is simply absent; the app works as before.
- **Tests**: icon script — corners transparent, tile centre unchanged; tray state → labels
  and icon as a pure function (`tray::menu_state(Recording { paused, elapsed })`), unit-tested.
  Manual: start/stop/pause from the tray with the window hidden, start from the tray during a
  calendar event, quit while recording, icon on light and dark panels.

## Known Gaps

- **Close to tray** (closing the window hides it instead of quitting) is not part of this spec:
  it changes what the close button means and needs a "fo-foca is still running" hint. Can be
  added later as a setting.
- No global keyboard shortcut for start/stop.
- macOS/Windows tray behaviour (left-click) isn't covered — Linux first.
