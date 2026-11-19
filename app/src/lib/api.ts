// Backend bridge. Inside Tauri it calls the Rust commands; in a plain browser (`npm run dev`
// without Tauri) it falls back to a scripted fake so the UI can be developed and screenshotted.

import type { ConfigView, Meeting, MeetingMeta, SessionEvent } from "./types";
import * as fake from "./fake";

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
  activeSession: () => call<MeetingMeta | null>("active_session"),
  start: (title: string) => call<MeetingMeta>("start_session", { title }),
  summarize: () => call<void>("summarize_now"),
  stop: (summarize: boolean) => call<void>("stop_session", { summarize }),
  discard: () => call<void>("discard_session"),
};

export async function onSession(cb: (e: SessionEvent) => void): Promise<() => void> {
  if (!inTauri) return fake.onSession(cb);
  const { listen } = await import("@tauri-apps/api/event");
  return listen<SessionEvent>("session", (e) => cb(e.payload));
}
