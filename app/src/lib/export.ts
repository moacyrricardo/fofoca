// Export dialog logic (spec 023): what's picked, the "You'll get" tree, sizes and progress text.
// Pure, with type-only imports, so `npm test` runs it under plain Node.

import type { ExportEvent, ExportOptions, ExportParts, ExportPlan, PlannedFile, TranscriptFormat } from "./types";

export interface Selection { transcript: boolean; format: TranscriptFormat; notes: boolean; tasks: boolean; audio: boolean; zip: boolean }

/** Options that ask for everything, so the plan lists every file the meeting can export. */
export function everything(dest: string): ExportOptions {
  return { transcript: "both", notes: true, tasks: true, audio: true, zip: false, dest };
}

/** What the dialog opens with: every text part the meeting has; audio and zip off. */
export function defaults(parts: ExportParts): Selection {
  return { transcript: parts.transcript_lines > 0, format: "markdown", notes: parts.notes !== null, tasks: parts.tasks > 0, audio: false, zip: false };
}

export function options(sel: Selection, dest: string): ExportOptions {
  return { transcript: sel.transcript ? sel.format : null, notes: sel.notes, tasks: sel.tasks, audio: sel.audio, zip: sel.zip, dest };
}

/** The planned files the selection picks. */
export function picked(files: PlannedFile[], sel: Selection): PlannedFile[] {
  return files.filter((f) => {
    switch (f.part) {
      case "transcript_md": return sel.transcript && sel.format !== "srt";
      case "transcript_srt": return sel.transcript && sel.format !== "markdown";
      case "notes": return sel.notes;
      case "tasks": return sel.tasks;
      case "audio": return sel.audio;
    }
  });
}

/** `6 KB`, `43.1 MB`, `1.2 GB` (decimal units, as file managers show them). */
export function formatBytes(n: number): string {
  if (n >= 1e9) return `${(n / 1e9).toFixed(1)} GB`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)} MB`;
  return `${n > 0 ? Math.max(1, Math.round(n / 1e3)) : 0} KB`;
}

const sum = (files: PlannedFile[]) => files.reduce((t, f) => t + f.bytes, 0);

/** Rough zip size: WAV is stored as is, text deflates to about 40 %. */
export function zipEstimate(files: PlannedFile[]): string {
  const wav = sum(files.filter((f) => f.path.endsWith(".wav")));
  return `≈ ${formatBytes(wav + 0.4 * (sum(files) - wav))}`;
}

/** The size shown next to a part: what it would add with the current format. */
export function partBytes(files: PlannedFile[], part: "transcript" | "notes" | "tasks" | "audio", format: TranscriptFormat): number {
  const sel: Selection = { transcript: part === "transcript", format, notes: part === "notes", tasks: part === "tasks", audio: part === "audio", zip: false };
  return sum(picked(files, sel));
}

export interface TreeLine { prefix: string; name: string; kind: "dir" | "file" | "zip"; size?: string }

/** The "You'll get" tree: the destination, the meeting's folder with its files, the zip next to it. */
export function tree(destDisplay: string, folder: string, files: PlannedFile[], zip: boolean): TreeLine[] {
  const out: TreeLine[] = [{ prefix: "", name: `${destDisplay}/`, kind: "dir" }, { prefix: zip ? "├─ " : "└─ ", name: `${folder}/`, kind: "dir" }];
  const bar = zip ? "│  " : "   ";
  const top = files.filter((f) => !f.path.includes("/"));
  const audio = files.filter((f) => f.path.startsWith("audio/"));
  top.forEach((f, i) => out.push({ prefix: bar + (i === top.length - 1 && !audio.length ? "└─ " : "├─ "), name: f.path, kind: "file", size: formatBytes(f.bytes) }));
  if (audio.length) {
    out.push({ prefix: `${bar}└─ `, name: "audio/", kind: "dir" });
    audio.forEach((f, i) => out.push({ prefix: `${bar}   ${i === audio.length - 1 ? "└─ " : "├─ "}`, name: f.path.slice("audio/".length), kind: "file", size: formatBytes(f.bytes) }));
  }
  if (zip) out.push({ prefix: "└─ ", name: `${folder}.zip`, kind: "zip", size: zipEstimate(files) });
  return out;
}

/** `6 files · 43.1 MB + zip`, or a nudge when nothing is picked. */
export function total(files: PlannedFile[], zip: boolean): string {
  if (!files.length) return "Pick at least one part";
  return `${files.length} file${files.length > 1 ? "s" : ""} · ${formatBytes(sum(files))}${zip ? " + zip" : ""}`;
}

/** What the working view says: `Writing notes.md`, `Copying audio/mic.wav · 21.5 MB`, `Making X.zip`. */
export function progressText(e: ExportEvent | null, plan: ExportPlan | null): string {
  if (!e || !e.step) return "Starting…";
  if (e.step === "writing") return `Writing ${e.file}`;
  if (e.step === "zipping") return `Making ${e.file}`;
  const f = plan?.files.find((x) => x.path === e.file);
  return `Copying ${e.file}${f ? ` · ${formatBytes(f.bytes)}` : ""}`;
}

export function progressRatio(e: ExportEvent | null): number {
  return e && e.total > 0 ? Math.min(1, e.done / e.total) : 0;
}

/** The audio row's description, from the files the meeting has. */
export function audioWhat(files: PlannedFile[], imported: boolean): string {
  const names = files.filter((f) => f.part === "audio").map((f) => f.path);
  if (imported) return "the imported recording, as WAV";
  const mic = names.includes("audio/mic.wav"), call = names.includes("audio/call.wav");
  return mic && call ? "your mic and the call, as WAV" : mic ? "your mic, as WAV" : "the call, as WAV";
}

/** The toast for a finished export when no export dialog is open to show it (e.g. the user left
 *  the meeting page meanwhile); `null` while running, when cancelled, or when the dialog shows it. */
export function exportNotice(e: ExportEvent, dialogOpen: boolean): { text: string; error: boolean } | null {
  if (dialogOpen) return null;
  if (e.state === "done") return { text: `Exported to ${baseName(e.folder ?? "")}${e.zip ? " (and a .zip)" : ""}.`, error: false };
  if (e.state === "failed") return { text: `Export failed: ${e.error ?? "unknown error"}`, error: true };
  return null;
}

/** The last part of a path (the exported folder's name). */
export function baseName(path: string): string {
  return path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() ?? path;
}
