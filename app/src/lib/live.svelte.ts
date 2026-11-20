// Live-session state, fed by backend events. Lives outside the screens so leaving the Live
// screen (e.g. to check Settings) doesn't lose the transcript.

import type { MeetingMeta, Segment, Status, Summary } from "./types";

export const live = $state({
  meeting: null as MeetingMeta | null,
  segments: [] as Segment[],
  /** Meeting times (s) where the recording was paused. */
  pauses: [] as number[],
  partial: null as Summary | null,
  final: null as Summary | null,
  status: null as Status | null,
  summarizing: false,
  stopping: false,
  steps: [] as string[],
  /** How far the running post-call step is, 0..1 (the per-track rebuild, spec 018). */
  progress: null as { step: string; progress: number } | null,
});

export function resetLive(meeting: MeetingMeta | null) {
  live.meeting = meeting;
  live.segments = [];
  live.pauses = [];
  live.partial = null;
  live.final = null;
  live.status = null;
  live.summarizing = false;
  live.stopping = false;
  live.steps = [];
  live.progress = null;
}
