# 014 — Replay: listen to a meeting with the transcript following along

> **Design reference:** [`014-assets/replay-mock.html`](014-assets/replay-mock.html) — clickable mock
> of the meeting page's Transcript tab (audio simulated; toggle "ⓘ annotations" for what's invented).

## Context

The transcript, speakers (003) and checked notes (005) all point at moments in the call, but there
is no way to *hear* those moments. Checking a doubtful line ("did she really say 87.500?"), or
catching tone the text loses, means opening `mic.wav` / `system.wav` in another player and seeking
by hand. The notes' time chips already jump to the transcript line; they should be able to play it.

Bounds from earlier specs: audio may be deleted by the retention policy (011), so replay must
degrade cleanly; paused stretches (010) are not in the files, so the audio timeline equals the
transcript timeline; imports (012) have only a call track.

## Decision

- **Player bar** pinned at the top of the Transcript tab: play/pause, **↺ 10 s** (hear it again),
  current time / duration, speed (1× → 1.25× → 1.5× → 2×), **mute You** and **mute Call**, and
  **Follow** (auto-scroll to the playing line, on by default).
- **The bar is a speaker lane** (who spoke when, in the transcript's speaker colours, with a tick
  per pause) and it is the seek control: click or drag anywhere; hover shows the time. No waveform.
- **Following along**: the playing line is shaded, the playing word is boxed, words already said
  brighten. Word timing comes from the stored words (interpolated live; precise when a later pass
  writes them).
- **Click a line** to play from its start. Keyboard: Space play/pause, ←/→ 5 s.
- **Muting** a track drops it from what you hear and dims its lines (call voices = Call track).
  Imports have no mic track, so the You toggle is hidden there.
- If you scroll away while Follow is on, auto-scroll pauses and **"Back to playing line"** appears.
- **Time chips** in Summary and Tasks play from that moment (switching to the Transcript tab).
- Switching tabs while playing keeps a **mini player** (play/pause, progress, current line, "go to
  transcript") at the bottom of the meeting page. Leaving the meeting page stops playback.
- **No audio** (deleted by 011, or never recorded): the bar is replaced by one line — "The audio of
  this meeting was deleted on … — the transcript stays, replay isn't available." Time chips keep
  jumping to the line without playing.

## Implementation

- **Serving audio** — Tauri's asset protocol (`tauri` feature `protocol-asset`), scope granted at
  startup to the meetings folder only (`app.asset_protocol_scope().allow_directory(meetings_dir,
  true)`, since `meetings_dir` is configurable). The webview loads
  `convertFileSrc(<meeting dir>/mic.wav)`; the protocol serves HTTP `Range`, so long files stream
  instead of loading into memory. New command `audio_tracks(id) -> { mic: Option<path>, call:
  Option<path> }` (only files that exist), so the UI never builds paths itself.
- **Two `<audio>` elements, one clock** — `mic` and `call` play together; the call element is the
  master. Every 250 ms, if `|mic.currentTime − call.currentTime| > 0.08 s`, set
  `mic.currentTime` to the master's. Mute = `element.muted`; speed = `playbackRate` on both;
  seek sets both. (Decoding both tracks with Web Audio would cost ~460 MB of RAM per hour of
  meeting, so streaming elements are used instead.)
- **State** in `app/src/lib/player.svelte.ts` (meeting id, time, playing, rate, muted, follow) so
  the Transcript tab, the mini player and time chips share one player. `Player.svelte` (bar + lane),
  `MiniPlayer.svelte`; `MeetingView.svelte` renders words as spans (`data-start`/`data-end`) and
  marks `now`/`said` from the player time on `timeupdate` + `requestAnimationFrame` while playing.
- **Speaker lane** drawn from segments (start/end/speaker colour via `speakerColor`) and
  `meta.pauses[].at_s` ticks; pointer events with capture for dragging.
- **Follow**: `scrollIntoView({block: "center"})` on line change unless the user scrolled the
  container since the last programmatic scroll (wheel/touch/keyboard scroll sets a flag; "Back to
  playing line" clears it).
- **Fake backend**: `audio_tracks` returns a short generated WAV (data URL) so the browser preview
  plays real sound.
- **Tests**: the sync/drift rule, current line/word lookup (binary search over segment starts) and
  lane geometry as pure TS functions; Rust: `audio_tracks` returns only existing files and refuses
  ids outside the store. Manual: a real recording, both tracks, mute each, 2×, seek by drag, a
  meeting whose audio was deleted, an import.

## Known Gaps

- No waveform; no per-speaker mute (only You / Call tracks exist as audio).
- Playing while recording another meeting isn't blocked (it only uses the speakers, not the GPU).
- Word highlight drifts within long lines while word timings are interpolated.
