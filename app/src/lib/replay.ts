// Replay (spec 014): the pure parts — clock sync, line/word lookup, lane geometry. No imports
// besides types, so `npm test` can run this file under plain Node.

import type { Segment } from "./types";

/** How far the mic element may drift from the call element (the master clock) before it's reset. */
export const DRIFT_S = 0.08;

/** The time to set on the follower element, or null when it's close enough to the master. */
export function resync(master: number, follower: number, tolerance = DRIFT_S): number | null {
  return Math.abs(master - follower) > tolerance ? master : null;
}

export const RATES = [1, 1.25, 1.5, 2];
/** 1× → 1.25× → 1.5× → 2× → 1×. */
export function nextRate(r: number): number {
  const i = RATES.indexOf(r);
  return RATES[(i + 1) % RATES.length];
}

/** Index of the line playing at `t`: the last one starting at or before `t` (binary search over
 *  line starts, which are sorted), or -1 before the first line. */
export function lineAt(starts: number[], t: number): number {
  let lo = 0, hi = starts.length - 1, found = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (starts[mid] <= t) { found = mid; lo = mid + 1; } else hi = mid - 1;
  }
  return found;
}

export interface TimedWord { start: number; end: number; text: string }

/** A line's words with times: the stored ones when present, else the line's span spread over its
 *  words in proportion to their length (as fofoca-core's `approx_words` does). */
export function wordTimes(seg: Pick<Segment, "start" | "end" | "text" | "words">): TimedWord[] {
  if (seg.words?.length) return seg.words.map((w) => ({ start: w.start, end: w.end, text: w.text.trim() })).filter((w) => w.text);
  const parts = seg.text.split(/\s+/).filter(Boolean);
  const total = parts.reduce((n, p) => n + p.length, 0) || 1;
  const span = Math.max(0, seg.end - seg.start);
  let at = seg.start;
  return parts.map((text) => {
    const start = at;
    at += (span * text.length) / total;
    return { start, end: at, text };
  });
}

/** Index of the word being said at `t`: the last one starting at or before it (-1 before the first). */
export function wordAt(words: TimedWord[], t: number): number {
  return lineAt(words.map((w) => w.start), t);
}

/** Playback can't run past the audio, and the transcript may outlast the audio by a little. */
export function clamp(t: number, duration: number): number {
  return Math.min(Math.max(0, t), Math.max(0, duration));
}

/** Pointer x (px from the lane's left edge) → meeting time. */
export function timeAtX(x: number, width: number, duration: number): number {
  if (width <= 0) return 0;
  return clamp((x / width) * duration, duration);
}

export interface LaneBlock { left: number; width: number; seg: number }
/** Lane blocks as percentages of the lane; lines past the end are cut, empty ones dropped. */
export function laneBlocks(segs: { start: number; end: number }[], duration: number): LaneBlock[] {
  if (duration <= 0) return [];
  const out: LaneBlock[] = [];
  segs.forEach((s, i) => {
    const a = clamp(s.start, duration), b = clamp(s.end, duration);
    if (b > a) out.push({ left: (a / duration) * 100, width: ((b - a) / duration) * 100, seg: i });
  });
  return out;
}
