// Tasks screen (spec 016): the pure parts — label filter, grouping, due-date order.
// Type-only imports, so `npm test` can run it under plain Node.

import type { TaskItem } from "./types";

/** Labels across the tasks' meetings, most used first (counted per meeting, like the Meetings list). */
export function labelCounts(tasks: TaskItem[]): [string, number][] {
  const perMeeting = new Map<string, string[]>();
  for (const t of tasks) perMeeting.set(t.meeting_id, t.labels);
  const c = new Map<string, number>();
  for (const labels of perMeeting.values()) for (const l of labels) c.set(l, (c.get(l) ?? 0) + 1);
  return [...c].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]));
}

/** Tasks whose meeting carries every selected label. */
export function filterTasks(tasks: TaskItem[], labels: string[]): TaskItem[] {
  return labels.length ? tasks.filter((t) => labels.every((l) => t.labels.includes(l))) : tasks;
}

export interface MeetingGroup { meeting_id: string; meeting_title: string; started_at: string; labels: string[]; tasks: TaskItem[] }

/** Grouped by meeting, keeping the given order (the backend sends newest meeting first). */
export function byMeeting(tasks: TaskItem[]): MeetingGroup[] {
  const out: MeetingGroup[] = [];
  for (const t of tasks) {
    let g = out.find((x) => x.meeting_id === t.meeting_id);
    if (!g) out.push((g = { meeting_id: t.meeting_id, meeting_title: t.meeting_title, started_at: t.started_at, labels: t.labels, tasks: [] }));
    g.tasks.push(t);
  }
  return out;
}

/** `YYYY-MM-DD` of a date in local time. */
export const isoDay = (d: Date) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;

export const isOverdue = (t: TaskItem, today: string) => !!t.due && t.due < today;

/** Overdue first, then by date, then tasks without a date (newest meeting first among those). */
export function byDue(tasks: TaskItem[]): TaskItem[] {
  return tasks
    .map((t, i) => ({ t, i }))
    .sort((a, b) => {
      const [x, y] = [a.t.due, b.t.due];
      if (x && y) return x.localeCompare(y) || a.i - b.i;
      if (x) return -1;
      if (y) return 1;
      return a.i - b.i;
    })
    .map((e) => e.t);
}
