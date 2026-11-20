// Live transcript order (spec 019). Lines arrive out of order — a long call turn can finish after
// your later reply — so each one is placed by its start time instead of appended.

/** Put `line` into `lines` (sorted by start) after every line starting at or before it, and
 * return where it went. Lines with equal starts keep their arrival order. */
export function insertByStart<T extends { start: number }>(lines: T[], line: T): number {
  let i = lines.length;
  while (i > 0 && lines[i - 1].start > line.start) i--;
  lines.splice(i, 0, line);
  return i;
}
