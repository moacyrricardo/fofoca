# 008 — Local MCP server: read transcripts, import audio

## Problem

Meeting notes are more useful when other tools can reach them: an AI assistant answering "what did
we decide with the BH client last week?" or "list my open follow-ups" needs to **read transcripts
and summaries**. The reverse also matters: recordings made elsewhere (a phone memo, a Zoom cloud
recording, a WhatsApp voice note) should be **injectable** — transcribed and summarized by fo-foca
like a live meeting.

Categorization comes from **labels (spec 007)**, which ship first: assistants filter by label, and
imported audio is labelled on the way in.

Constraints: everything stays local; the MCP server must not become a door into private
conversations.

## Hypotheses / Options

### Exposure

- **A. stdio MCP server bundled as `fofoca-mcp`** (a binary of `fofoca-core`), launched by the
  assistant (Claude Code / Claude Desktop config). Reads the meetings folder directly; no daemon.
  + simplest, no port, only the local user's processes can use it · − importing audio must run the
  models itself, competing with the app for the GPU.
- **B. MCP endpoint inside the running app** (localhost HTTP). + one owner of the GPU/model swap;
  imports queue behind a live recording · − only works while the app is open; a listening port
  needs auth (token in the keyring).
- **C. A + hand-off**: the stdio server reads freely; for imports it hands the job to the app if
  it's running (local socket) and otherwise runs it itself.

### Tools (first cut)

- Read: `list_meetings(label?, since?, query?)`, `get_meeting(id)` (summary + transcript with
  timestamps and speakers), `search_transcripts(text, label?)`, `list_labels()`.
- Write: `import_audio(path, title?, labels?, started_at?)` → transcribe (+ summary) as a meeting;
  `set_labels(id, labels)`.
- Read-only by default; write tools opt-in in Settings.

### Importing audio

- Any format via ffmpeg → 16 kHz mono → the chunk pipeline offline. No live budget, so it can use
  `verbose_json` word timestamps and run speaker separation (003) on the single track.
- Also reachable from the UI ("Import recording…"), not only MCP — possibly its own spec first.

## Open Questions

- A, B or C? (Depends on whether the app becomes a tray/background app — not decided.)
- Should write tools (import, set labels) be available to assistants at all, or UI-only?
- Search: plain substring over files first, or an embedding index?
- Privacy: per-label "never expose via MCP"?
- Does "Import recording…" in the UI become its own spec before the MCP server?
