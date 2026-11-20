// Settings → Processing (spec 015): the pure parts — layouts, GPU memory, Live updates, titles.
// Type-only imports, so `npm test` can run it under plain Node.

import type { Device, Layout, Updates, Workflow } from "./types";

export type Lane = "asr" | "llm" | "gap";
export interface LayoutInfo {
  id: Layout;
  name: string;
  why: string;
  /** Speech → text / summary devices. */
  devices: [Device, Device];
  /** Six audio pieces on each lane, for the little diagram. */
  lanes: { gpu: Lane[]; cpu: Lane[] };
}

export const LAYOUTS: LayoutInfo[] = [
  { id: "swap", name: "Both on the GPU, taking turns", devices: ["gpu", "gpu"],
    why: "Speech → text runs on the GPU; between pieces of audio the summary model is swapped in (~3 s). Best quality and speed when the GPU is small.",
    lanes: { gpu: ["asr", "asr", "llm", "asr", "asr", "llm"], cpu: ["gap", "gap", "gap", "gap", "gap", "gap"] } },
  { id: "gpu_transcript", name: "Transcript on GPU · summary on CPU", devices: ["gpu", "cpu"],
    why: "Text keeps flowing with no swaps. Summaries are slower (~12 tokens/s) and the computer gets busy while one is written.",
    lanes: { gpu: ["asr", "asr", "asr", "asr", "asr", "asr"], cpu: ["gap", "gap", "llm", "llm", "gap", "llm"] } },
  { id: "gpu_summary", name: "Transcript on CPU · summary on GPU", devices: ["cpu", "gpu"],
    why: "Summaries are quick (~17 tokens/s), but live text arrives later and may fall behind on long calls.",
    lanes: { gpu: ["gap", "gap", "llm", "gap", "gap", "llm"], cpu: ["asr", "asr", "asr", "asr", "asr", "asr"] } },
  { id: "cpu", name: "Everything on the CPU", devices: ["cpu", "cpu"],
    why: "No GPU needed. Slowest: live text lags and the computer is busy for the whole call.",
    lanes: { gpu: ["gap", "gap", "gap", "gap", "gap", "gap"], cpu: ["asr", "asr", "llm", "asr", "asr", "llm"] } },
  { id: "gpu_both", name: "Both loaded on the GPU at once", devices: ["gpu", "gpu"],
    why: "No swaps and no waiting — needs room for both models on the GPU.",
    lanes: { gpu: ["asr", "asr", "asr", "asr", "asr", "asr"], cpu: ["gap", "gap", "gap", "gap", "gap", "gap"] } },
];

export const layoutInfo = (id: Layout) => LAYOUTS.find((l) => l.id === id)!;

/** GPU memory (MiB) at the busiest moment of a call, and what's loaded then. */
export function peakMib(w: Workflow, whisperMib: number | null, llmMib: number | null): { mib: number | null; whisper: number; llm: number } {
  const live = w.live_transcript, summary = live && w.live_summary;
  let onGpu: [boolean, boolean];
  if (!live) onGpu = [false, false];
  else if (!summary) onGpu = [w.live_device === "gpu", false];
  else { const [a, b] = layoutInfo(w.layout).devices; onGpu = [a === "gpu", b === "gpu"]; }
  if (summary && w.layout === "swap") {
    // one at a time: the bigger of the two
    if (whisperMib === null || llmMib === null) return { mib: null, whisper: 0, llm: 0 };
    return llmMib >= whisperMib ? { mib: llmMib, whisper: 0, llm: llmMib } : { mib: whisperMib, whisper: whisperMib, llm: 0 };
  }
  const need = (on: boolean, m: number | null) => (on ? m : 0);
  const a = need(onGpu[0], whisperMib), b = need(onGpu[1], llmMib);
  if (a === null || b === null) return { mib: null, whisper: 0, llm: 0 };
  return { mib: a + b, whisper: a, llm: b };
}

/** Why a layout can't be picked here, or null when it can. */
export function unavailable(id: Layout, gpuMib: number | null, whisperMib: number | null, llmMib: number | null): string | null {
  const info = layoutInfo(id);
  if (gpuMib === null && info.devices.includes("gpu")) return "Needs a GPU fo-foca can use — none was found.";
  if (id === "gpu_both" && gpuMib !== null && whisperMib !== null && llmMib !== null && whisperMib + llmMib > gpuMib * 0.95)
    return `Doesn't fit: ${fmt(whisperMib)} + ${fmt(llmMib)} MiB is more than this GPU's ${fmt(gpuMib)} MiB.`;
  return null;
}

export interface UpdatesInfo { id: Updates; name: string; what: string; how: string; measured: boolean }
export const UPDATES: UpdatesInfo[] = [
  { id: "fast", name: "Fast", what: "Text sooner, summary refreshed more often. The computer works harder.", how: "pieces 8–15 s · summary every 3 min", measured: false },
  { id: "balanced", name: "Balanced", what: "Text about 13 s after it's said; summary every 5 min.", how: "pieces 8–30 s · summary every 5 min", measured: true },
  { id: "light", name: "Light", what: "Computer mostly free. Text comes in bigger, later blocks; summary only when you ask.", how: "pieces 30 s · summary on request", measured: false },
];

/** The banner title, e.g. "Swap layout · Balanced updates". */
export function setupTitle(w: Workflow): string {
  if (!w.live_transcript) return "Record only";
  const pace = w.updates === "custom" ? "Custom updates" : `${UPDATES.find((u) => u.id === w.updates)!.name} updates`;
  if (!w.live_summary) return `Live transcript on the ${w.live_device.toUpperCase()} · ${pace}`;
  const short: Record<Layout, string> = { swap: "Swap layout", gpu_transcript: "Transcript on GPU · summary on CPU", gpu_summary: "Transcript on CPU · summary on GPU", cpu: "Everything on the CPU", gpu_both: "Both on the GPU" };
  return `${short[w.layout]} · ${pace}`;
}

/** The steps that run when a recording stops, in order. */
export function afterSteps(w: Workflow): { name: string; device: Device | null }[] {
  const out: { name: string; device: Device | null }[] = [];
  if (!w.live_transcript) out.push({ name: "Transcribe", device: w.live_device });
  if (w.speakers_after) out.push({ name: "Speakers", device: "cpu" });
  if (w.notes_after) out.push({ name: "Checked notes", device: w.notes_device });
  out.push({ name: "Storage", device: null });
  return out;
}

export const fmt = (n: number) => n.toLocaleString("en-US");
