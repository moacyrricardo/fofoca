// Run with `npm test`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { audioWhat, baseName, defaults, exportNotice, everything, formatBytes, options, partBytes, picked, progressRatio, progressText, total, tree, zipEstimate } from "./export";
import type { ExportEvent, ExportParts, ExportPlan, PlannedFile } from "./types";

const files: PlannedFile[] = [
  { part: "transcript_md", path: "transcript.md", bytes: 6_200 },
  { part: "transcript_srt", path: "transcript.srt", bytes: 7_100 },
  { part: "notes", path: "notes.md", bytes: 2_900 },
  { part: "tasks", path: "tasks.md", bytes: 600 },
  { part: "audio", path: "audio/mic.wav", bytes: 21_500_000 },
  { part: "audio", path: "audio/call.wav", bytes: 21_600_000 },
];
const parts: ExportParts = { transcript_lines: 64, notes: "checked", tasks: 3, audio_tracks: 2, audio_deleted_at: null, imported: false };
const sel = { ...defaults(parts) };

test("sizes read like a file manager's", () => {
  assert.equal(formatBytes(0), "0 KB");
  assert.equal(formatBytes(120), "1 KB");
  assert.equal(formatBytes(6_200), "6 KB");
  assert.equal(formatBytes(43_100_000), "43.1 MB");
  assert.equal(formatBytes(1_230_000_000), "1.2 GB");
});

test("defaults: the text parts the meeting has, no audio, no zip", () => {
  assert.deepEqual(defaults(parts), { transcript: true, format: "markdown", notes: true, tasks: true, audio: false, zip: false });
  assert.deepEqual(defaults({ ...parts, transcript_lines: 0, notes: null, tasks: 0 }), { transcript: false, format: "markdown", notes: false, tasks: false, audio: false, zip: false });
  assert.deepEqual(everything("/d"), { transcript: "both", notes: true, tasks: true, audio: true, zip: false, dest: "/d" });
  assert.deepEqual(options({ ...sel, transcript: false, zip: true }, "/d"), { transcript: null, notes: true, tasks: true, audio: false, zip: true, dest: "/d" });
  assert.deepEqual(options({ ...sel, format: "srt" }, "/d").transcript, "srt");
});

test("the format switch picks the transcript files", () => {
  const paths = (s: typeof sel) => picked(files, s).map((f) => f.path);
  assert.deepEqual(paths(sel), ["transcript.md", "notes.md", "tasks.md"]);
  assert.deepEqual(paths({ ...sel, format: "srt" }), ["transcript.srt", "notes.md", "tasks.md"]);
  assert.deepEqual(paths({ ...sel, format: "both", audio: true }), ["transcript.md", "transcript.srt", "notes.md", "tasks.md", "audio/mic.wav", "audio/call.wav"]);
  assert.deepEqual(paths({ ...sel, transcript: false, notes: false, tasks: false }), []);
  assert.equal(partBytes(files, "transcript", "both"), 13_300);
  assert.equal(partBytes(files, "audio", "markdown"), 43_100_000);
});

test("the tree without a zip ends at the folder; files then audio/", () => {
  const t = tree("~/Documentos/fo-foca", "2026-11-20 Sync", picked(files, { ...sel, audio: true }), false);
  assert.deepEqual(t.map((l) => l.prefix + l.name), [
    "~/Documentos/fo-foca/",
    "└─ 2026-11-20 Sync/",
    "   ├─ transcript.md",
    "   ├─ notes.md",
    "   ├─ tasks.md",
    "   └─ audio/",
    "      ├─ mic.wav",
    "      └─ call.wav",
  ]);
  assert.deepEqual(t.map((l) => l.kind), ["dir", "dir", "file", "file", "file", "dir", "file", "file"]);
  assert.equal(t[6].size, "21.5 MB");
});

test("with a zip, the folder gets a sibling and the bar continues", () => {
  const t = tree("~/x", "F", picked(files, sel), true);
  assert.deepEqual(t.map((l) => l.prefix + l.name), ["~/x/", "├─ F/", "│  ├─ transcript.md", "│  ├─ notes.md", "│  └─ tasks.md", "└─ F.zip"]);
  assert.equal(t.at(-1)!.kind, "zip");
  assert.equal(t.at(-1)!.size, "≈ 4 KB", "text deflates to about 40 %");
  assert.equal(zipEstimate(picked(files, { ...sel, audio: true })), "≈ 43.1 MB", "WAV is stored");
});

test("the footer total", () => {
  assert.equal(total(picked(files, { ...sel, audio: true }), true), "5 files · 43.1 MB + zip");
  assert.equal(total(picked(files, { ...sel, notes: false, tasks: false }), false), "1 file · 6 KB");
  assert.equal(total([], false), "Pick at least one part");
});

test("progress text and ratio", () => {
  const plan = { files } as ExportPlan;
  const e = (step: ExportEvent["step"], file: string, done = 0, total = 0): ExportEvent =>
    ({ meeting_id: "m", state: "running", step, file, done, total, folder: null, zip: null, error: null });
  assert.equal(progressText(null, plan), "Starting…");
  assert.equal(progressText(e("writing", "notes.md"), plan), "Writing notes.md");
  assert.equal(progressText(e("copying", "audio/mic.wav"), plan), "Copying audio/mic.wav · 21.5 MB");
  assert.equal(progressText(e("zipping", "F.zip"), plan), "Making F.zip");
  assert.equal(progressRatio(e("copying", "x", 50, 200)), 0.25);
  assert.equal(progressRatio(e("copying", "x", 0, 0)), 0);
});

test("a finished export is announced only when no dialog shows it", () => {
  const e = (state: ExportEvent["state"], extra: Partial<ExportEvent> = {}): ExportEvent =>
    ({ meeting_id: "m", state, step: null, file: "", done: 0, total: 0, folder: "/home/you/fo-foca/2026-11-20 Sync", zip: null, error: null, ...extra });
  assert.equal(exportNotice(e("done"), true), null);
  assert.deepEqual(exportNotice(e("done"), false), { text: "Exported to 2026-11-20 Sync.", error: false });
  assert.deepEqual(exportNotice(e("done", { zip: "/x.zip" }), false), { text: "Exported to 2026-11-20 Sync (and a .zip).", error: false });
  assert.deepEqual(exportNotice(e("failed", { error: "disk full" }), false), { text: "Export failed: disk full", error: true });
  assert.equal(exportNotice(e("failed"), true), null);
  assert.equal(exportNotice(e("cancelled"), false), null);
  assert.equal(exportNotice(e("running"), false), null);
});

test("audio description and folder names", () => {
  assert.equal(audioWhat(files, false), "your mic and the call, as WAV");
  assert.equal(audioWhat(files.filter((f) => f.path !== "audio/mic.wav"), false), "the call, as WAV");
  assert.equal(audioWhat([{ part: "audio", path: "audio/cliente.wav", bytes: 1 }], true), "the imported recording, as WAV");
  assert.equal(baseName("/home/you/Documentos/fo-foca/2026-11-20 Sync (2)"), "2026-11-20 Sync (2)");
  assert.equal(baseName("C:\\x\\F\\"), "F");
});
