// Replay (spec 014): one player shared by the Transcript tab, the mini player and the time chips.
// Two streaming <audio> elements (mic + call) play together; the call element is the master clock
// and the mic element is nudged back whenever it drifts.

import { clamp, nextRate, resync } from "./replay";

export interface TrackUrls { mic: string | null; call: string | null }

export const player = $state({
  /** Meeting whose audio is loaded, or null. */
  meetingId: null as string | null,
  hasMic: false,
  hasCall: false,
  time: 0,
  duration: 0,
  playing: false,
  rate: 1,
  muted: { you: false, call: false },
  follow: true,
  /** Last playback error, shown in the bar. */
  error: null as string | null,
});

let mic: HTMLAudioElement | null = null;
let call: HTMLAudioElement | null = null;
let syncTimer: ReturnType<typeof setInterval> | undefined;
let frame = 0;

const master = () => call ?? mic;
const elements = () => [call, mic].filter((e): e is HTMLAudioElement => !!e);

function element(url: string): HTMLAudioElement {
  const a = new Audio();
  a.preload = "metadata";
  a.src = url;
  a.onerror = () => {
    player.error = a.error?.message || "the audio file couldn't be read";
    pause();
  };
  return a;
}

/** Load a meeting's tracks (no-op when they're already loaded). `fallback` is the duration to show
 *  until the audio reports its own. */
export function load(meetingId: string, urls: TrackUrls, fallback: number) {
  if (player.meetingId === meetingId) return;
  unload();
  if (!urls.mic && !urls.call) return;
  player.meetingId = meetingId;
  player.hasMic = !!urls.mic;
  player.hasCall = !!urls.call;
  player.duration = fallback;
  mic = urls.mic ? element(urls.mic) : null;
  call = urls.call ? element(urls.call) : null;
  const m = master()!;
  m.onloadedmetadata = () => {
    if (Number.isFinite(m.duration) && m.duration > 0) player.duration = m.duration;
  };
  m.onended = () => {
    pause();
    player.time = player.duration;
  };
  for (const e of elements()) e.playbackRate = player.rate;
  applyMute();
}

/** Stop and forget the audio (leaving the meeting page). */
export function unload() {
  pause();
  for (const e of elements()) {
    e.removeAttribute("src");
    e.load();
  }
  mic = call = null;
  player.meetingId = null;
  player.hasMic = player.hasCall = false;
  player.time = player.duration = 0;
  player.error = null;
}

export async function play() {
  const m = master();
  if (!m) return;
  if (player.time >= player.duration - 0.05) seek(0);
  player.error = null;
  try {
    await Promise.all(elements().map((e) => e.play()));
  } catch (e) {
    player.error = String(e);
    pause();
    return;
  }
  player.playing = true;
  clearInterval(syncTimer);
  syncTimer = setInterval(() => {
    if (mic && call) {
      const t = resync(call.currentTime, mic.currentTime);
      if (t !== null) mic.currentTime = t;
    }
  }, 250);
  const tick = () => {
    if (!player.playing) return;
    player.time = master()?.currentTime ?? player.time;
    frame = requestAnimationFrame(tick);
  };
  frame = requestAnimationFrame(tick);
}

export function pause() {
  for (const e of elements()) e.pause();
  player.playing = false;
  clearInterval(syncTimer);
  cancelAnimationFrame(frame);
  const m = master();
  if (m) player.time = m.currentTime;
}

export function toggle() {
  if (player.playing) pause();
  else void play();
}

export function seek(t: number) {
  const to = clamp(t, player.duration);
  for (const e of elements()) e.currentTime = to;
  player.time = to;
}

export const skip = (by: number) => seek(player.time + by);

export function cycleRate() {
  player.rate = nextRate(player.rate);
  for (const e of elements()) e.playbackRate = player.rate;
}

function applyMute() {
  if (mic) mic.muted = player.muted.you;
  if (call) call.muted = player.muted.call;
}

export function setMuted(track: "you" | "call", muted: boolean) {
  player.muted[track] = muted;
  applyMute();
}
