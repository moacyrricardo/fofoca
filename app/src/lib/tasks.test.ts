// Run with `npm test`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { byDue, byMeeting, filterTasks, isOverdue, isoDay, labelCounts } from "./tasks";
import type { TaskItem } from "./types";

const t = (meeting: string, text: string, labels: string[], due: string | null = null): TaskItem =>
  ({ meeting_id: meeting, meeting_title: `M ${meeting}`, started_at: "2026-11-19T10:00:00-03:00", labels, text, owner: null, due, refs: [] });
const tasks = [
  t("b", "b1", ["trabalho", "diretoria"], "2026-11-25"),
  t("b", "b2", ["trabalho", "diretoria"]),
  t("a", "a1", ["trabalho", "1:1"], "2026-11-10"),
  t("c", "c1", [], "2026-11-21"),
];

test("label filter uses the meetings' labels, all selected must match", () => {
  assert.deepEqual(filterTasks(tasks, []).length, 4);
  assert.deepEqual(filterTasks(tasks, ["trabalho"]).map((x) => x.text), ["b1", "b2", "a1"]);
  assert.deepEqual(filterTasks(tasks, ["trabalho", "1:1"]).map((x) => x.text), ["a1"]);
  assert.deepEqual(labelCounts(tasks), [["trabalho", 2], ["1:1", 1], ["diretoria", 1]]); // per meeting, not per task
});

test("grouping by meeting keeps the newest-first order", () => {
  const g = byMeeting(tasks);
  assert.deepEqual(g.map((x) => [x.meeting_id, x.tasks.length]), [["b", 2], ["a", 1], ["c", 1]]);
});

test("due order: by date, undated last; overdue against today", () => {
  assert.deepEqual(byDue(tasks).map((x) => x.text), ["a1", "c1", "b1", "b2"]);
  assert.equal(isOverdue(tasks[2], "2026-11-20"), true);
  assert.equal(isOverdue(tasks[0], "2026-11-20"), false);
  assert.equal(isOverdue(tasks[1], "2026-11-20"), false);
  assert.equal(isoDay(new Date(2026, 0, 5)), "2026-01-05");
});
