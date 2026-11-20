// Scripted stand-in for the Rust backend, used only outside Tauri (browser preview).

import { normalizeLabels } from "./format";
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
    action_items: [{ text: "Enviar o follow-up por e-mail", owner: "Você", due: "2026-11-19", refs: [64] }],
    flagged: [{ section: "summary", text: "A diferença no orçamento foi de R$ 26.250.", reason: "26250 isn't said in the cited lines", refs: [40] }],
    model: "Qwen3.5-2B-Q4_K_M", gen_tokens_per_s: 17.2,
  }, segments: separated, summaries: [{ kind: "final", at_s: 72, text: FINAL, model: "Qwen3.5-2B-Q4_K_M", gen_tokens_per_s: 17.2 }] },
  { meta: { id: "2026-11-18-103000", title: "1:1 Fernanda", started_at: "2026-11-18T10:30:00-03:00", duration_s: 1920, finished: true, labels: ["trabalho", "1:1"], calendar_uid: "1" }, segments: segs.slice(0, 3), summaries: [] },
];

const config: ConfigView = {
  path: "~/.config/fofoca/config.toml",
  meetings_dir: "~/.local/share/fofoca/meetings",
  whisper_model: "ggml-large-v3-turbo-q5_0",
  llm_model: "Qwen3.5-2B-Q4_K_M",
  config: {
    language: "pt", chunk_seconds: 30, min_chunk_seconds: 8,
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

export function onSession(cb: (e: SessionEvent) => void): () => void {
  listener = cb;
  return () => (listener = null);
}

export async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  switch (cmd) {
    case "get_config": return config as T;
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
    case "stop_session":
      timers.forEach(clearTimeout); timers = [];
      emit({ type: "step", step: "transcribing" });
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
  segs.forEach((s, i) => timers.push(window.setTimeout(() => emit({ type: "segment", segment: s }), 1500 + i * 1700)));
  return meta;
}
