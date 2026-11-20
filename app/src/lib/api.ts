// Backend bridge. Inside Tauri it calls the Rust commands; in a plain browser (`npm run dev`
// without Tauri) it falls back to a scripted fake so the UI can be developed and screenshotted.

import type { AudioConfig, JobEvent, StorageView, CalendarSettings, CalendarSource, ConfigView, LabelCount, Meeting, MeetingMeta, SessionEvent, Upcoming } from "./types";
import * as fake from "./fake";
import type { TrackUrls } from "./player.svelte";

const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/** True in a plain browser, where the fake backend is used (UI development / screenshots). */
export const preview = !inTauri;

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!inTauri) return fake.call<T>(cmd, args);
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(cmd, args);
}

export const api = {
  config: () => call<ConfigView>("get_config"),
  meetings: () => call<MeetingMeta[]>("list_meetings"),
  meeting: (id: string) => call<Meeting>("get_meeting", { id }),
  renameSpeaker: (id: string, speaker: string, name: string) => call<Meeting>("rename_speaker", { id, speaker, name }),
  separateSpeakers: (id: string) => call<Meeting>("separate_speakers", { id }),
  writeNotes: (id: string) => call<Meeting>("write_notes", { id }),
  setKeepAudio: (id: string, keep: boolean) => call<Meeting>("set_keep_audio", { id, keep }),
  deleteAudio: (id: string) => call<Meeting>("delete_audio", { id }),
  /** Playable URLs of the meeting's audio files that still exist (spec 014). */
  audioTracks: async (id: string): Promise<TrackUrls> => {
    if (!inTauri) return fake.audioTracks(id);
    // Loopback http:// URLs — WebKitGTK's GStreamer can't stream from asset:// (spec 014 fix).
    return call<TrackUrls>("audio_tracks", { id });
  },
  storage: () => call<StorageView>("storage"),
  pickRecording: () => call<string | null>("pick_recording"),
  importRecording: (path: string, title: string | null = null, labels: string[] = []) => call<MeetingMeta>("import_recording", { path, title, labels }),
  setAudioPolicy: (audio: AudioConfig) => call<StorageView>("set_audio_policy", { audio }),
  activeSession: () => call<MeetingMeta | null>("active_session"),
  start: (title: string, attendees: string[] = [], labels: string[] = [], calendarUid: string | null = null) =>
    call<MeetingMeta>("start_session", { title, attendees, labels, calendarUid }),
  knownLabels: () => call<LabelCount[]>("known_labels"),
  setMeetingLabels: (id: string, labels: string[]) => call<MeetingMeta>("set_meeting_labels", { id, labels }),
  summarize: () => call<void>("summarize_now"),
  stop: (summarize: boolean) => call<void>("stop_session", { summarize }),
  discard: () => call<void>("discard_session"),
  pause: () => call<void>("pause_session"),
  resume: () => call<void>("resume_session"),
  calendars: () => call<CalendarSettings>("calendar_settings"),
  addCalendar: (input: string) => call<CalendarSource>("add_calendar", { input }),
  removeCalendar: (id: string) => call<void>("remove_calendar", { id }),
  setCalendarDefaultLabels: (id: string, labels: string[]) => call<CalendarSettings>("set_calendar_default_labels", { id, labels }),
  setCalendarOptions: (o: { onlyWithCallLink: boolean; notifyBefore: boolean; autoRecord: boolean }) =>
    call<CalendarSettings>("set_calendar_options", o),
  notify: (title: string, body: string) => call<void>("notify", { title, body }),
  upcoming: (force = false) => call<Upcoming>("upcoming_events", { force }),
};

export async function onSession(cb: (e: SessionEvent) => void): Promise<() => void> {
  if (!inTauri) return fake.onSession(cb);
  const { listen } = await import("@tauri-apps/api/event");
  return listen<SessionEvent>("session", (e) => cb(e.payload));
}

/** Background job progress (imports, spec 012). */
export async function onJob(cb: (e: JobEvent) => void): Promise<() => void> {
  if (!inTauri) return fake.onJob(cb);
  const { listen } = await import("@tauri-apps/api/event");
  return listen<JobEvent>("job", (e) => cb(e.payload));
}

export const JOB_STEPS: Record<string, string> = {
  queued: "Waiting", converting: "Converting", transcribing: "Transcribing", speakers: "Separating speakers", notes: "Writing notes",
};
