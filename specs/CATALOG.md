# Catalog

| #   | Status | Title / slug                | Branch                                   | Issue | Notes |
|-----|--------|-----------------------------|------------------------------------------|-------|-------|
| 001 | done   | walking-skeleton            | moacyrricardo/spec-001-walking-skeleton  | —     | assets: mock, MX230 benchmarks |
| 002 | todo   | auto-setup                  | —                                        | —     |       |
| 003 | done   | speaker-separation          | moacyrricardo/spec-003-speaker-separation | —     |       |
| 004 | done   | calendar-ical               | moacyrricardo/spec-004-calendar-ical    | —     |       |
| 005 | done   | grounded-summary-tasks      | moacyrricardo/spec-005-grounded-notes    | —     |       |
| 006 | todo   | packaging-model-servers     | —                                        | —     | models no longer bundled → 015 |
| 007 | done   | meeting-labels              | moacyrricardo/spec-007-meeting-labels    | —     | ships before 008 |
| 008 | concern| mcp-server                  | —                                        | —     | builds on 007 labels |
| 009 | done   | auto-labels                 | moacyrricardo/spec-009-auto-labels       | —     | builds on 004 + 007 |
| 010 | done   | recording-controls          | moacyrricardo/spec-010-recording-controls | —     | pause + adaptive chunking |
| 011 | done   | audio-retention             | moacyrricardo/spec-011-audio-retention   | —     | after 003/005 (they need the audio) |
| 012 | done   | import-recording            | moacyrricardo/spec-012-import-recording  | —     | uses the 003 post-call pipeline; feeds 008 |
| 013 | done   | servers-die-with-app        | moacyrricardo/spec-013-servers-die-with-app | —  | found while measuring 005 |
| 014 | done   | replay                      | moacyrricardo/spec-014-replay           | —     | mock in 014-assets; needs audio kept (011) |
| 015 | done   | models-workflow             | moacyrricardo/spec-015-models-workflow  | —     | mock in 015-assets; global settings, tabs |
| 016 | done   | tasks-view                  | moacyrricardo/spec-016-tasks-view       | —     | filtered by meeting labels; no per-task labels |
| 017 | done   | tray-and-icon               | moacyrricardo/spec-017-tray-and-icon-build | —     | icon without white corners; tray start/stop + show/hide |
| 018 | todo   | per-track-transcription     | —                                        | —     | mic and call transcribed separately; replaces mix + loudness guess |
| 020 | done   | postcall-transcript         | moacyrricardo/spec-020-postcall-transcript-build | —     | after the call: mic and call transcribed separately; live kept; re-run button; imports same path |
| 022 | done   | diarization-fixes           | moacyrricardo/spec-022-diarization-fixes-build | —     | ort optimizer corrupts CAM++ embeddings; segmentation timing drift; small-cluster cleanup, no forced merges, threshold 0.4 |
| 023 | done   | export-meeting              | moacyrricardo/spec-023-export-meeting-build | —     | mock in 023-assets; folder (+ optional zip) with transcript md/srt, notes, tasks, audio |
| 024 | todo   | github-pages                | —                                        | —     | mock in 024-assets; docs/ served by Pages; roadmap follows the catalog |
