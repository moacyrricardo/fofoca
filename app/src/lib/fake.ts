// Scripted stand-in for the Rust backend, used only outside Tauri (browser preview).

import { normalizeLabels } from "./format";
import type { DownloadEvent, ExportEvent, ExportOptions, ExportPlan, JobEvent, ModelsView, PlannedFile, Workflow, WorkflowView } from "./types";
import type { CalendarEvent, CalendarSettings, ConfigView, Meeting, MeetingMeta, Segment, SessionEvent } from "./types";

const LINES: [string, "you" | "call"][] = [
  ["Bom dia a todos. Conforme deliberado na reunião anterior, apresento o relatório consolidado referente ao terceiro trimestre de 2026.", "you"],
  ["Ressalto, entretanto, que os indicadores de desempenho permanecem aquém das expectativas estabelecidas pela diretoria.", "you"],
  ["Pois é, galera, vou ser sincero, o bagulho tá tenso. O cliente lá de Belo Horizonte deu um perdido na gente e sumiu do mapa.", "call"],
  ["Tipo assim, mandou mensagem no zap dizendo que ia fechar o contrato e depois deu ghost.", "call"],
  ["Fala sério, né? Que mancada.", "call"],
  ["Dito isso, cumpre salientar que o orçamento previsto era de 120 mil reais. Porém, executamos apenas 87.500.", "you"],
  ["A equipe do Rodrigo e da Fernanda fez o deploy da nova feature na sexta-feira, dia 15 de novembro.", "call"],
  ["E o feedback dos usuários foi bem positivo. Enfim, bora fazer acontecer.", "call"],
  ["Vou mandar o follow-up por e-mail ainda hoje, beleza? Se alguém tiver alguma dúvida, me dá um toque.", "you"],
];
const segs: Segment[] = LINES.map(([text, track], i) => ({ start: i * 8, end: i * 8 + 7, text, track, words: [] }));
// after speaker separation (spec 003): two voices on the call
const VOICES = ["s1", "s1", "s2", "s1", "s2", "s1"];
let callIdx = 0;
const separated: Segment[] = segs.map((s) => ({ ...s, speaker: s.track === "you" ? "you" : VOICES[callIdx++ % VOICES.length] }));
const FINAL = `**Resumo**
O relatório do 3º trimestre de 2026 mostra indicadores abaixo das metas da diretoria. O cliente de Belo Horizonte prometeu fechar o contrato e parou de responder. A nova feature foi publicada na sexta, 15 de novembro, com feedback positivo.

**Pontos principais**
- Indicadores abaixo das expectativas da diretoria.
- Cliente de Belo Horizonte sumiu após prometer fechar.
- Orçamento: R$ 87.500 executados de R$ 120 mil previstos.

**Próximos passos**
- Enviar o follow-up por e-mail hoje.`;

const meetings: Meeting[] = [
  { meta: { id: "2026-11-19-140200", title: "Relatório Q3 — revisão com a diretoria", started_at: "2026-11-19T14:02:00-03:00", duration_s: 1104, finished: true, labels: ["trabalho", "diretoria"], attendees: ["Rodrigo Lima", "Fernanda", "Ana"], steps_done: ["speakers"] }, speakers: { s1: { name: "Rodrigo Lima" }, s2: {} }, notes: {
    summary: [
      { text: "O relatório do 3º trimestre de 2026 mostra indicadores abaixo das expectativas da diretoria.", refs: [0, 8] },
      { text: "O cliente de Belo Horizonte prometeu fechar o contrato e sumiu.", refs: [16, 24] },
      { text: "Orçamento previsto de R$ 120 mil; executados R$ 87.500.", refs: [40] },
      { text: "A nova feature foi publicada na sexta, 15 de novembro, com feedback positivo.", refs: [48, 56] },
    ],
    decisions: [],
    action_items: [
      { text: "Enviar o follow-up por e-mail", owner: "Você", due: "2026-11-19", refs: [64] },
      { text: "Retomar o contato com o cliente de Belo Horizonte", owner: "Rodrigo Lima", due: "2026-11-27", refs: [16, 24] },
    ],
    flagged: [{ section: "summary", text: "A diferença no orçamento foi de R$ 26.250.", reason: "26250 isn't said in the cited lines", refs: [40] }],
    model: "Qwen3.5-2B-Q4_K_M", gen_tokens_per_s: 17.2,
  }, segments: separated, summaries: [{ kind: "final", at_s: 72, text: FINAL, model: "Qwen3.5-2B-Q4_K_M", gen_tokens_per_s: 17.2 }] },
  { meta: { id: "2026-11-18-103000", title: "1:1 Fernanda", started_at: "2026-11-18T10:30:00-03:00", duration_s: 1920, finished: true, labels: ["trabalho", "1:1"], calendar_uid: "1" }, segments: segs.slice(0, 3), summaries: [],
    notes: { summary: [], decisions: [], flagged: [], model: "Qwen3.5-2B-Q4_K_M", gen_tokens_per_s: 17.2, action_items: [
      { text: "Revisar os indicadores do trimestre antes da diretoria", owner: "Fernanda", due: "2026-11-21", refs: [8] },
      { text: "Marcar a conversa sobre o orçamento", owner: null, due: null, refs: [0] },
    ] } },
  // No checked notes, no summary, audio deleted: the export dialog's disabled parts (spec 023).
  { meta: { id: "2026-11-17-163000", title: "Conversa rápida 17/11", started_at: "2026-11-17T16:30:00-03:00", duration_s: 300, finished: true, labels: [], audio_deleted_at: "2026-11-18T09:00:00-03:00" }, segments: segs.slice(5, 8), summaries: [] },
];

const config: ConfigView = {
  path: "~/.config/fofoca/config.toml",
  meetings_dir: "~/.local/share/fofoca/meetings",
  export_dir: "/home/you/Documentos/fo-foca",
  export_dir_remembered: false,
  whisper_model: "ggml-large-v3-turbo-q5_0",
  llm_model: "Qwen3.5-2B-Q4_K_M",
  config: {
    language: "pt", chunk_seconds: 30, min_chunk_seconds: 8,
    workflow: { live_transcript: true, live_summary: true, layout: "swap", live_device: "gpu", updates: "balanced", summary_every_s: 300, transcript_after: true, speakers_after: true, notes_after: true, notes_device: "gpu" },
    whisper: { server: "~/.local/share/fofoca/bin/whisper/whisper-server", model: "~/.local/share/fofoca/models/ggml-large-v3-turbo-q5_0.bin", device: 0, threads: 4, vad_model: "~/.local/share/fofoca/models/ggml-silero-v5.1.2.bin" },
    llm: { server: "llama-server", model: "Qwen3.5-2B-Q4_K_M.gguf", device: "Vulkan1", context: 32768, threads: 4 },
  },
};

const at = (h: number, m = 0) => { const d = new Date(); d.setHours(h, m, 0, 0); return d.toISOString(); };
const events: CalendarEvent[] = [
  { uid: "1", title: "Sync semanal — produto", start: at(new Date().getHours() + 1), end: at(new Date().getHours() + 1, 30), all_day: false, call_link: "https://meet.google.com/abc-defg-hij", attendees: ["Rodrigo Lima", "Fernanda", "Ana"], calendar: "Trabalho", labels: ["trabalho"] },
  { uid: "2", title: "Kickoff cliente BH", start: at(new Date().getHours() + 3), end: at(new Date().getHours() + 4), all_day: false, call_link: "https://zoom.us/j/123", attendees: ["Ana"], calendar: "Trabalho", labels: ["trabalho"] },
];
let calSettings: CalendarSettings = { sources: [{ id: "cal-1", kind: "link", label: "Trabalho", location: "calendar.google.com", default_labels: ["trabalho"] }], only_with_call_link: true, notify_before: true, auto_record: false };

let audio: { keep: "always" | "after_processing" | "days"; keep_days: number } = { keep: "after_processing", keep_days: 30 };
let listener: ((e: SessionEvent) => void) | null = null;
let timers: number[] = [];
let live: MeetingMeta | null = null;
const emit = (e: SessionEvent) => listener?.(e);

let jobListener: ((e: JobEvent) => void) | null = null;
export function onJob(cb: (e: JobEvent) => void): () => void {
  jobListener = cb;
  return () => (jobListener = null);
}

export function onSession(cb: (e: SessionEvent) => void): () => void {
  listener = cb;
  return () => (listener = null);
}

export async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  switch (cmd) {
    case "list_tasks": return meetings.flatMap((m) => (m.notes?.action_items ?? []).map((t) => ({ meeting_id: m.meta.id, meeting_title: m.meta.title, started_at: m.meta.started_at, labels: m.meta.labels ?? [], ...t }))) as T;
    case "get_config": return structuredClone(config) as T;
    case "workflow_view": case "probe_hardware": return workflowView() as T;
    case "set_workflow": config.config.workflow = args?.workflow as Workflow; return workflowView() as T;
    case "models_view": return structuredClone(models) as T;
    case "use_model": {
      const row = models.tasks.find((t) => t.entries.some((e) => e.id === args?.id))!;
      row.current = args?.id as string; row.found = true;
      if (row.task === "llm") config.llm_model = row.entries.find((e) => e.id === args?.id)!.name;
      return structuredClone(models) as T;
    }
    case "pick_model_file": return "/home/you/Downloads/gemma-3-1b-it-Q4_K_M.gguf" as T;
    case "set_model_path": {
      const row = models.tasks.find((t) => t.task === (args?.setting === "segmentation" || args?.setting === "embedding" ? "speakers" : args?.setting));
      if (row) { row.paths = [args?.path as string]; row.current = null; row.found = true; }
      const eng = models.engines.find((e) => e.setting === args?.setting);
      if (eng) { eng.path = args?.path as string; eng.found = true; }
      return structuredClone(models) as T;
    }
    case "download_model": fakeDownload(args?.id as string); return undefined as T;
    case "cancel_download": cancelled.add(args?.id as string); return undefined as T;
    case "calendar_settings": return calSettings as T;
    case "upcoming_events":
      return { events: events.map((e) => ({ ...e, labels: normalizeLabels([...e.labels, ...(meetings.find((m) => m.meta.calendar_uid === e.uid)?.meta.labels ?? [])]) })), errors: [] } as T;
    case "set_calendar_options": calSettings = { ...calSettings, only_with_call_link: Boolean(args?.onlyWithCallLink), notify_before: Boolean(args?.notifyBefore), auto_record: Boolean(args?.autoRecord) }; return calSettings as T;
    case "notify": console.log("notify", args); return undefined as T;
    case "remove_calendar": calSettings = { ...calSettings, sources: calSettings.sources.filter((s) => s.id !== args?.id) }; return undefined as T;
    case "add_calendar": {
      const src = { id: `cal-${Date.now()}`, kind: "link" as const, label: "Pessoal", location: "calendar.google.com", default_labels: ["pessoal"] };
      calSettings = { ...calSettings, sources: [...calSettings.sources, src] };
      return src as T;
    }
    case "rename_speaker": {
      const m = meetings.find((x) => x.meta.id === args?.id)!;
      const name = String(args?.name ?? "").trim();
      m.speakers = { ...m.speakers, [String(args?.speaker)]: { name: name || null } };
      return structuredClone(m) as T;
    }
    case "pick_recording": return "/home/you/Gravações/reuniao-cliente-bh.m4a" as T;
    case "retranscribe": {
      const m = meetings.find((x) => x.meta.id === args?.id);
      if (!m || m.meta.audio_deleted_at) throw new Error("The audio was deleted");
      if (live?.id === m.meta.id) throw new Error("This meeting is still recording");
      const w = config.config.workflow!;
      const steps: [JobEvent["step"], number][] = [["tracks", 0], ["tracks", 0.5], ["tracks", 1]];
      if (w.speakers_after) steps.push(["speakers", 0]);
      if (w.notes_after) steps.push(["notes", 0]);
      steps.push(["done", 1]);
      jobListener?.({ meeting_id: m.meta.id, step: "queued", progress: 0 });
      steps.forEach(([step, progress], i) => window.setTimeout(() => {
        if (step === "done") m.meta.steps_done = ["tracks", ...(w.speakers_after ? ["speakers"] : []), ...(w.notes_after ? ["notes"] : [])];
        jobListener?.({ meeting_id: m.meta.id, step, progress });
      }, 600 + i * 750));
      return undefined as T;
    }
    case "import_recording": {
      const id = `2026-11-20-0900${meetings.length}`;
      const meta: MeetingMeta = { id, title: String(args?.title ?? "") || "reuniao-cliente-bh", started_at: "2026-11-20T09:00:00-03:00", duration_s: 0, finished: false, imported_from: "reuniao-cliente-bh.m4a", labels: normalizeLabels((args?.labels as string[]) ?? []) };
      meetings.unshift({ meta, segments: [], summaries: [] });
      jobListener?.({ meeting_id: id, step: "queued", progress: 0 });
      const steps: [JobEvent["step"], number][] = [["converting", 0], ["transcribing", 0.1], ["transcribing", 0.4], ["transcribing", 0.8], ["speakers", 0], ["notes", 0], ["done", 1]];
      steps.forEach(([step, progress], i) => window.setTimeout(() => {
        if (step === "done") {
          const m = meetings.find((x) => x.meta.id === id)!;
          m.meta = { ...m.meta, finished: true, duration_s: 1920, steps_done: ["tracks", "speakers", "notes"] };
          m.segments = separated.filter((s) => s.track === "call");
          m.speakers = { s1: {}, s2: {} };
        }
        jobListener?.({ meeting_id: id, step, progress });
      }, 600 + i * 1100));
      return meta as T;
    }
    case "pick_export_folder": return "/home/you/Desktop" as T;
    case "export_plan": return fakePlan(String(args?.id), args?.options as ExportOptions) as T;
    case "export_meeting": fakeExport(String(args?.id), args?.options as ExportOptions); return undefined as T;
    case "cancel_export": if (exporting) exporting.cancelled = true; return undefined as T;
    case "open_export_folder": console.log("open", args?.path); return undefined as T;
    case "storage": return { audio: audio, audio_bytes: 412_000_000, meetings_with_audio: 3 } as T;
    case "set_audio_policy": audio = args?.audio as typeof audio; return { audio, audio_bytes: audio.keep === "always" ? 412_000_000 : 120_000_000, meetings_with_audio: audio.keep === "always" ? 3 : 1 } as T;
    case "set_keep_audio": {
      const m = meetings.find((x) => x.meta.id === args?.id)!;
      m.meta = { ...m.meta, keep_audio: args?.keep ? true : null };
      return structuredClone(m) as T;
    }
    case "delete_audio": {
      const m = meetings.find((x) => x.meta.id === args?.id)!;
      m.meta = { ...m.meta, audio_deleted_at: new Date().toISOString() };
      return structuredClone(m) as T;
    }
    case "write_notes": {
      const m = meetings.find((x) => x.meta.id === args?.id)!;
      return new Promise((r) => setTimeout(() => r(structuredClone(m) as T), 1200));
    }
    case "separate_speakers": {
      const m = meetings.find((x) => x.meta.id === args?.id)!;
      return new Promise((r) => setTimeout(() => r(structuredClone(m) as T), 900));
    }
    case "list_meetings": return meetings.map((m) => m.meta).filter((m) => !args?.label || m.labels?.includes(String(args.label))) as T;
    case "known_labels": {
      const counts = new Map<string, number>();
      meetings.forEach((m) => m.meta.labels?.forEach((l) => counts.set(l, (counts.get(l) ?? 0) + 1)));
      return [...counts].map(([label, count]) => ({ label, count })).sort((a, b) => b.count - a.count || a.label.localeCompare(b.label)) as T;
    }
    case "set_meeting_labels": {
      const m = meetings.find((x) => x.meta.id === args?.id);
      if (!m) throw new Error("meeting not found");
      m.meta = { ...m.meta, labels: normalizeLabels(args?.labels as string[]) };
      return m.meta as T;
    }
    case "set_calendar_default_labels":
      calSettings = { ...calSettings, sources: calSettings.sources.map((s) => (s.id === args?.id ? { ...s, default_labels: normalizeLabels(args?.labels as string[]) } : s)) };
      return calSettings as T;
    case "get_meeting": return (meetings.find((m) => m.meta.id === args?.id) ?? meetings[0]) as T;
    case "active_session": return live as T;
    case "start_session": {
      const uid = args?.calendarUid as string | null;
      const series = uid ? meetings.find((m) => m.meta.calendar_uid === uid)?.meta.labels ?? [] : [];
      return startFake(String(args?.title ?? ""), normalizeLabels([...((args?.labels as string[]) ?? []), ...series])) as T;
    }
    case "summarize_now":
      emit(status(0, "off", "loading"));
      timers.push(window.setTimeout(() => emit(status(3, "off", "busy")), 1200));
      timers.push(window.setTimeout(() => {
        emit({ type: "summary", summary: { kind: "partial", at_s: 30, text: "- Q3 abaixo das metas da diretoria.\n- Cliente de BH sumiu depois de prometer fechar.", model: "Qwen3.5-2B-Q4_K_M", gen_tokens_per_s: 17.3 } });
        emit(status(0, "ready", "off"));
      }, 3600));
      return undefined as T;
    case "pause_session":
      if (pausedAt === null) {
        pausedAt = Date.now();
        queueMicrotask(() => emit({ type: "paused", at_s: (pausedAt! - t0 - pausedTotal) / 1000 }));
      }
      return undefined as T;
    case "resume_session":
      if (pausedAt !== null) {
        pausedTotal += Date.now() - pausedAt;
        pausedAt = null;
        queueMicrotask(() => emit({ type: "resumed", at_s: (Date.now() - t0 - pausedTotal) / 1000 }));
      }
      return undefined as T;
    case "discard_session":
      timers.forEach(clearTimeout); timers = [];
      live = null;
      queueMicrotask(() => emit({ type: "discarded", meeting_id: "fake-live" }));
      return undefined as T;
    case "stop_and_quit":
    case "stop_session":
      emit({ type: "stopping" });
      timers.forEach(clearTimeout); timers = [];
      emit({ type: "step", step: "transcribing" });
      if (!config.config.workflow!.live_transcript || config.config.workflow!.transcript_after)
        timers.push(window.setTimeout(() => emit({ type: "step", step: "tracks" }), 750));
      timers.push(window.setTimeout(() => emit({ type: "step", step: "summarizing" }), 1500));
      timers.push(window.setTimeout(() => emit({ type: "summary", summary: meetings[0].summaries[0] }), 4500));
      timers.push(window.setTimeout(() => emit({ type: "step", step: "saving" }), 4700));
      timers.push(window.setTimeout(() => { const m = { ...live!, finished: true, duration_s: 72 }; live = null; emit({ type: "finished", meeting: m }); }, 5200));
      return undefined as T;
  }
  throw new Error(`fake backend: unknown command ${cmd}`);
}

let t0 = 0;
let pausedAt: number | null = null;
let pausedTotal = 0;
function status(backlog: number, asr: "off" | "ready" | "busy" | "loading", llm: "off" | "loading" | "busy"): SessionEvent {
  const now = Date.now();
  const elapsed = (now - t0 - pausedTotal - (pausedAt ? now - pausedAt : 0)) / 1000;
  return { type: "status", elapsed_s: elapsed, backlog_s: backlog, mic_level: Math.random() * 0.05, call_level: Math.random() * 0.04, asr, llm, paused: pausedAt !== null };
}

function startFake(title: string, labels: string[]): MeetingMeta {
  t0 = Date.now();
  pausedAt = null;
  pausedTotal = 0;
  live = { id: "fake-live", title: title || "Sync semanal — produto", started_at: new Date().toISOString(), duration_s: 0, finished: false, labels };
  const meta = live;
  queueMicrotask(() => emit({ type: "started", meeting: meta }));
  timers.push(window.setInterval(() => emit(status(0, "ready", "off")), 250));
  if (config.config.workflow?.live_transcript !== false) segs.forEach((s, i) => timers.push(window.setTimeout(() => emit({ type: "segment", segment: s }), 1500 + i * 1700)));
  return meta;
}

// Replay (spec 014): short generated tracks so the browser preview plays real sound — a hum with
// syllable-like bursts wherever that track speaks (mic: one low voice; call: a pitch per voice).
const audioCache = new Map<string, string>();
function tone(segments: Segment[], pick: (s: Segment) => number | null): string {
  const rate = 8000, secs = Math.ceil(Math.max(0, ...segments.map((s) => s.end))) + 2;
  const n = rate * secs, buf = new ArrayBuffer(44 + n * 2), v = new DataView(buf);
  const str = (o: number, t: string) => [...t].forEach((c, i) => v.setUint8(o + i, c.charCodeAt(0)));
  str(0, "RIFF"); v.setUint32(4, 36 + n * 2, true); str(8, "WAVE"); str(12, "fmt ");
  v.setUint32(16, 16, true); v.setUint16(20, 1, true); v.setUint16(22, 1, true); v.setUint32(24, rate, true);
  v.setUint32(28, rate * 2, true); v.setUint16(32, 2, true); v.setUint16(34, 16, true); str(36, "data"); v.setUint32(40, n * 2, true);
  for (const s of segments) {
    const f = pick(s);
    if (f === null) continue;
    for (let i = Math.floor(s.start * rate); i < Math.min(n, Math.floor(s.end * rate)); i++) {
      const t = i / rate, syllable = Math.max(0, Math.sin(Math.PI * 4 * (t - s.start)));
      v.setInt16(44 + i * 2, Math.round(5000 * syllable * Math.sin(2 * Math.PI * f * t)), true);
    }
  }
  return URL.createObjectURL(new Blob([buf], { type: "audio/wav" }));
}

export function audioTracks(id: string): { mic: string | null; call: string | null } {
  const m = meetings.find((x) => x.meta.id === id);
  if (!m || m.meta.audio_deleted_at || !m.meta.finished || !m.segments.length) return { mic: null, call: null };
  const key = (t: string) => `${id}:${t}`;
  const voice = (s: Segment) => (s.speaker && s.speaker !== "you" ? 260 + 90 * (parseInt(s.speaker.slice(1), 10) || 1) : 330);
  if (!audioCache.has(key("call"))) audioCache.set(key("call"), tone(m.segments, (s) => (s.track === "call" ? voice(s) : null)));
  if (!m.meta.imported_from && !audioCache.has(key("mic"))) audioCache.set(key("mic"), tone(m.segments, (s) => (s.track === "you" ? 196 : null)));
  return { mic: m.meta.imported_from ? null : audioCache.get(key("mic"))!, call: audioCache.get(key("call"))! };
}

// Settings → Processing / Models (spec 015)
function workflowView(): WorkflowView {
  return {
    workflow: structuredClone(config.config.workflow!), gpu: { device: "Vulkan1", name: "NVIDIA GeForce MX230", total_mib: 2294, free_mib: 2234 },
    suggested: "swap", recording: !!live,
    whisper: { name: config.whisper_model, footprint: { mib: 824, measured: true } },
    llm: { name: config.llm_model, footprint: { mib: 1414, measured: true } },
  };
}
const M = "~/.local/share/fofoca/models/";
const models: ModelsView = {
  dir: M,
  tasks: [
    { task: "whisper", paths: [M + "ggml-large-v3-turbo-q5_0.bin"], found: true, current: "whisper-turbo", entries: [
      { id: "whisper-turbo", name: "Whisper large-v3-turbo · q5_0", note: "best for pt-BR (~2 % errors on the test recording)", bytes: 574041195, present: true, downloading: false },
      { id: "whisper-small", name: "Whisper small · q5_1", note: "~3× faster, ~5× more errors in pt-BR", bytes: 190085487, present: false, downloading: false }] },
    { task: "llm", paths: ["~/models/Qwen3.5-2B-Q4_K_M.gguf"], found: true, current: "qwen3.5-2b", entries: [
      { id: "qwen3.5-2b", name: "Qwen3.5-2B · Q4_K_M", note: "~17 tokens/s on a 2 GB GPU", bytes: 1280835840, present: true, downloading: false },
      { id: "qwen3-1.7b", name: "Qwen3-1.7B · Q4_K_M", note: "a bit faster, fits less context", bytes: 1107409472, present: false, downloading: false }] },
    { task: "speakers", paths: [M + "segmentation-3.0.onnx", M + "wespeaker_en_voxceleb_CAM++.onnx"], found: false, current: "pyannote", entries: [
      { id: "pyannote", name: "segmentation-3.0 + WeSpeaker CAM++", note: "runs on the CPU", bytes: 35276520, present: false, downloading: false }] },
    { task: "vad", paths: [M + "ggml-silero-v5.1.2.bin"], found: true, current: "silero-vad", entries: [
      { id: "silero-vad", name: "Silero VAD v5.1.2", note: "skips silence: no invented lines, faster chunks", bytes: 885098, present: true, downloading: false }] },
  ],
  engines: [
    { setting: "whisper_server", path: "~/.local/share/fofoca/bin/whisper/whisper-server", found: true },
    { setting: "llm_server", path: "~/llama.cpp/build/bin/llama-server", found: true },
  ],
};
const downloadListeners: ((e: DownloadEvent) => void)[] = [];
const cancelled = new Set<string>();
export async function onDownload(cb: (e: DownloadEvent) => void): Promise<() => void> {
  downloadListeners.push(cb);
  return () => downloadListeners.splice(downloadListeners.indexOf(cb), 1);
}
function fakeDownload(id: string) {
  const row = models.tasks.find((t) => t.entries.some((e) => e.id === id))!;
  const entry = row.entries.find((e) => e.id === id)!;
  entry.downloading = true;
  cancelled.delete(id);
  let done = 0;
  const t = setInterval(() => {
    const emit = (e: DownloadEvent) => downloadListeners.forEach((l) => l(e));
    if (cancelled.has(id)) { clearInterval(t); entry.downloading = false; emit({ id, done: 0, total: entry.bytes, state: "cancelled" }); return; }
    done = Math.min(entry.bytes, done + entry.bytes / 12);
    if (done >= entry.bytes) {
      clearInterval(t); entry.downloading = false; entry.present = true; row.current = id; row.found = true;
      emit({ id, done, total: entry.bytes, state: "done" });
    } else emit({ id, done, total: entry.bytes, state: "running" });
  }, 250);
}

// Meeting export (spec 023): plans from the fake meetings, a scripted run with progress and cancel.
const takenFolders = new Set<string>();
let exporting: { cancelled: boolean } | null = null;
const exportListeners: ((e: ExportEvent) => void)[] = [];
export function onExport(cb: (e: ExportEvent) => void): () => void {
  exportListeners.push(cb);
  return () => exportListeners.splice(exportListeners.indexOf(cb), 1);
}

function fakePlan(id: string, o: ExportOptions): ExportPlan {
  const m = meetings.find((x) => x.meta.id === id) ?? meetings[0];
  const title = m.meta.title.replace(/[\\/:*?"<>|]/g, "-").replace(/^[.\s]+|[.\s]+$/g, "") || "Meeting";
  const base = `${m.meta.started_at.slice(0, 10)} ${title}`;
  let name = base;
  for (let n = 2; takenFolders.has(`${o.dest}/${name}`); n++) name = `${base} (${n})`;
  const final = m.summaries.filter((s) => s.kind === "final").at(-1);
  const notes = m.notes ? "checked" as const : final ? "summary" as const : null;
  const tasks = m.notes?.action_items.length ?? 0;
  const audio = !m.meta.audio_deleted_at && m.meta.finished && m.segments.length > 0;
  const stem = m.meta.imported_from?.replace(/\.[^.]*$/, "");
  const track = Math.round(m.meta.duration_s * 32_000); // 16 kHz, 16-bit mono
  const all: PlannedFile[] = [
    { part: "transcript_md", path: "transcript.md", bytes: 240 + m.segments.reduce((t, s) => t + s.text.length + 24, 0) },
    { part: "transcript_srt", path: "transcript.srt", bytes: m.segments.reduce((t, s) => t + s.text.length + 48, 0) },
    ...(notes ? [{ part: "notes" as const, path: "notes.md", bytes: notes === "checked" ? 1_400 : 300 + (final?.text.length ?? 0) }] : []),
    ...(tasks ? [{ part: "tasks" as const, path: "tasks.md", bytes: 260 + tasks * 70 }] : []),
    ...(audio ? (stem ? [{ part: "audio" as const, path: `audio/${stem}.wav`, bytes: track }] : [{ part: "audio" as const, path: "audio/mic.wav", bytes: track }, { part: "audio" as const, path: "audio/call.wav", bytes: track + 4_000 }]) : []),
  ];
  const files = all.filter((f) =>
    f.part === "transcript_md" ? o.transcript === "markdown" || o.transcript === "both"
    : f.part === "transcript_srt" ? o.transcript === "srt" || o.transcript === "both"
    : f.part === "notes" ? o.notes : f.part === "tasks" ? o.tasks : o.audio);
  return {
    dest: o.dest, dest_display: o.dest.replace(/^\/home\/you/, "~"), folder_name: name, folder: `${o.dest}/${name}`,
    zip: o.zip ? `${o.dest}/${name}.zip` : null, files,
    parts: { transcript_lines: m.segments.length, notes, tasks, audio_tracks: audio ? (stem ? 1 : 2) : 0, audio_deleted_at: m.meta.audio_deleted_at ?? null, imported: !!stem },
  };
}

function fakeExport(id: string, o: ExportOptions) {
  if (live?.id === id) throw new Error("This meeting is still recording");
  if (exporting) throw new Error("An export is already running");
  const plan = fakePlan(id, o);
  if (!plan.files.length) throw new Error("nothing to export: pick at least one part");
  const job = (exporting = { cancelled: false });
  const sum = plan.files.reduce((t, f) => t + f.bytes, 0);
  const total = o.zip ? sum * 2 : sum;
  // one tick per text file, a few per audio file and for the zip (like the mock's pacing)
  const ticks: Omit<ExportEvent, "meeting_id" | "state" | "total" | "folder" | "zip" | "error">[] = [];
  let done = 0;
  for (const f of plan.files) {
    const n = f.part === "audio" ? 4 : 1;
    for (let i = 0; i < n; i++) { ticks.push({ step: f.part === "audio" ? "copying" : "writing", file: f.path, done }); done += f.bytes / n; }
  }
  if (o.zip) for (let i = 0; i < 4; i++) { ticks.push({ step: "zipping", file: `${plan.folder_name}.zip`, done }); done += sum / 4; }
  const emit = (e: Partial<ExportEvent> & Pick<ExportEvent, "state">) =>
    exportListeners.forEach((l) => l({ meeting_id: id, step: null, file: "", done: 0, total, folder: null, zip: null, error: null, ...e }));
  let i = 0;
  const tick = () => {
    if (job.cancelled) { exporting = null; emit({ state: "cancelled" }); return; }
    if (i >= ticks.length) {
      exporting = null;
      takenFolders.add(plan.folder);
      config.export_dir = o.dest; config.export_dir_remembered = true;
      emit({ state: "done", done: total, folder: plan.folder, zip: plan.zip });
      return;
    }
    const t = ticks[i++];
    emit({ state: "running", ...t, done: Math.round(t.done) });
    window.setTimeout(tick, t.step === "writing" ? 300 : 450);
  };
  window.setTimeout(tick, 100);
}
