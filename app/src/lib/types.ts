// Mirrors fofoca-core's serialized types.

export type Track = "you" | "call";
export type ModelState = "off" | "loading" | "ready" | "busy";

export interface Word { start: number; end: number; text: string; p: number }
export interface Segment { start: number; end: number; text: string; track: Track; words: Word[] }
export interface Summary { kind: "partial" | "final"; at_s: number; text: string; model: string; gen_tokens_per_s: number }
export interface MeetingMeta { id: string; title: string; started_at: string; duration_s: number; finished: boolean }
export interface Meeting { meta: MeetingMeta; segments: Segment[]; summaries: Summary[] }

export interface Status {
  elapsed_s: number; backlog_s: number; mic_level: number; call_level: number; asr: ModelState; llm: ModelState;
}

export type SessionEvent =
  | { type: "started"; meeting: MeetingMeta }
  | ({ type: "status" } & Status)
  | { type: "segment"; segment: Segment }
  | { type: "step"; step: "transcribing" | "summarizing" | "saving" }
  | { type: "summary"; summary: Summary }
  | { type: "finished"; meeting: MeetingMeta }
  | { type: "discarded"; meeting_id: string }
  | { type: "notice"; message: string }
  | { type: "error"; message: string };

export interface CalendarEvent {
  uid: string; title: string; start: string; end: string; all_day: boolean;
  call_link: string | null; attendees: string[]; calendar: string;
}
export interface CalendarSource { id: string; kind: "link" | "file"; label: string; location: string }
export interface CalendarSettings { sources: CalendarSource[]; only_with_call_link: boolean }
export interface Upcoming { events: CalendarEvent[]; errors: string[] }

export interface ConfigView {
  path: string;
  meetings_dir: string;
  whisper_model: string;
  llm_model: string;
  config: {
    language: string;
    chunk_seconds: number;
    whisper: { server: string; model: string; device: number; threads: number; vad_model: string | null };
    llm: { server: string; model: string; device: string; context: number; threads: number };
  };
}
