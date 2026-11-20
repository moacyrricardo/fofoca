// Mirrors fofoca-core's serialized types.

export type Track = "you" | "call";
export type ModelState = "off" | "loading" | "ready" | "busy";

export interface Word { start: number; end: number; text: string; p: number }
export interface Segment { start: number; end: number; text: string; track: Track; words: Word[]; speaker?: string | null }
export interface SpeakerInfo { name?: string | null }
export interface Summary { kind: "partial" | "final"; at_s: number; text: string; model: string; gen_tokens_per_s: number }
export interface MeetingMeta { id: string; title: string; started_at: string; duration_s: number; finished: boolean; attendees?: string[]; labels?: string[]; calendar_uid?: string; pauses?: Pause[]; steps_done?: string[]; processed?: boolean; keep_audio?: boolean | null; audio_deleted_at?: string | null; imported_from?: string | null }
export interface Pause { at_s: number; from: string; to: string | null }
export interface Meeting { meta: MeetingMeta; segments: Segment[]; summaries: Summary[]; speakers?: Record<string, SpeakerInfo>; notes?: Notes | null }
/** Grounded notes (spec 005): every item cites transcript line start times (s). */
export interface NoteItem { text: string; refs: number[] }
export interface NoteAction { text: string; owner: string | null; due: string | null; refs: number[] }
export interface NoteFlag { section: string; text: string; reason: string; refs: number[] }
export interface Notes { summary: NoteItem[]; decisions: NoteItem[]; action_items: NoteAction[]; flagged: NoteFlag[]; model: string; gen_tokens_per_s: number }

export interface Status {
  elapsed_s: number; backlog_s: number; mic_level: number; call_level: number; asr: ModelState; llm: ModelState; paused?: boolean;
}

export type SessionEvent =
  | { type: "started"; meeting: MeetingMeta }
  | ({ type: "status" } & Status)
  | { type: "segment"; segment: Segment }
  | { type: "paused"; at_s: number }
  | { type: "resumed"; at_s: number }
  | { type: "step"; step: "transcribing" | "summarizing" | "saving" }
  | { type: "summary"; summary: Summary }
  | { type: "finished"; meeting: MeetingMeta }
  | { type: "discarded"; meeting_id: string }
  | { type: "notice"; message: string }
  | { type: "error"; message: string };

export interface CalendarEvent {
  uid: string; title: string; start: string; end: string; all_day: boolean;
  call_link: string | null; attendees: string[]; calendar: string; labels: string[];
}
export interface LabelCount { label: string; count: number }
export interface CalendarSource { id: string; kind: "link" | "file"; label: string; location: string; default_labels?: string[] }
export interface CalendarSettings { sources: CalendarSource[]; only_with_call_link: boolean; notify_before: boolean; auto_record: boolean }
export interface Upcoming { events: CalendarEvent[]; errors: string[] }

/** Background job progress (spec 012). */
export interface JobEvent { meeting_id: string; step: "queued" | "converting" | "transcribing" | "speakers" | "notes" | "done" | "failed"; progress: number; error?: string }
export interface AudioConfig { keep: "always" | "after_processing" | "days"; keep_days: number }
export interface StorageView { audio: AudioConfig; audio_bytes: number; meetings_with_audio: number }

/** One action item with its meeting (spec 016); filtered by the meeting's labels. */
export interface TaskItem { meeting_id: string; meeting_title: string; started_at: string; labels: string[]; text: string; owner: string | null; due: string | null; refs: number[] }

/** Settings → Processing (spec 015). */
export type Device = "gpu" | "cpu";
export type Layout = "swap" | "gpu_transcript" | "gpu_summary" | "cpu" | "gpu_both";
export type Updates = "fast" | "balanced" | "light" | "custom";
export interface Workflow {
  live_transcript: boolean; live_summary: boolean; layout: Layout; live_device: Device; updates: Updates;
  summary_every_s: number; speakers_after: boolean; notes_after: boolean; notes_device: Device;
}
export interface Gpu { device: string; name: string; total_mib: number; free_mib: number }
export interface Footprint { mib: number; measured: boolean }
export interface WorkflowView {
  workflow: Workflow; gpu: Gpu | null; suggested: Layout; recording: boolean;
  whisper: { name: string; footprint: Footprint | null }; llm: { name: string; footprint: Footprint | null };
}
/** Settings → Models (spec 015). */
export type ModelSetting = "whisper" | "vad" | "llm" | "segmentation" | "embedding" | "whisper_server" | "llm_server";
export type ModelTask = "whisper" | "vad" | "llm" | "speakers";
export interface ModelEntry { id: string; name: string; note: string; bytes: number; present: boolean; downloading: boolean }
export interface TaskRow { task: ModelTask; paths: string[]; found: boolean; current: string | null; entries: ModelEntry[] }
export interface EngineRow { setting: ModelSetting; path: string; found: boolean }
export interface ModelsView { dir: string; tasks: TaskRow[]; engines: EngineRow[] }
export interface DownloadEvent { id: string; done: number; total: number; state: "running" | "done" | "failed" | "cancelled"; error?: string | null }

export interface ConfigView {
  path: string;
  meetings_dir: string;
  whisper_model: string;
  llm_model: string;
  config: {
    language: string;
    chunk_seconds: number;
    workflow?: Workflow;
    min_chunk_seconds?: number;
    whisper: { server: string; model: string; device: number; threads: number; vad_model: string | null };
    llm: { server: string; model: string; device: string; context: number; threads: number };
  };
}
