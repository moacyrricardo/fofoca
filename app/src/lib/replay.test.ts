// Run with `npm test`: esbuild (shipped with Vite) bundles this file, Node's test runner runs it.
import { test } from "node:test";
import assert from "node:assert/strict";
import { clamp, laneBlocks, lineAt, nextRate, resync, timeAtX, wordAt, wordTimes } from "./replay";

test("resync only corrects drift beyond the tolerance", () => {
  assert.equal(resync(10, 10.05), null);
  assert.equal(resync(10, 9.93), null);
  assert.equal(resync(10, 10.2), 10);
  assert.equal(resync(10, 9.5), 10);
});

test("speed cycles 1 → 1.25 → 1.5 → 2 → 1", () => {
  assert.deepEqual([1, 1.25, 1.5, 2].map(nextRate), [1.25, 1.5, 2, 1]);
  assert.equal(nextRate(3), 1); // unknown rate starts over
});

test("lineAt finds the last line starting at or before t", () => {
  const starts = [0, 8, 16, 24];
  assert.equal(lineAt(starts, -1), -1);
  assert.equal(lineAt(starts, 0), 0);
  assert.equal(lineAt(starts, 7.9), 0);
  assert.equal(lineAt(starts, 8), 1);
  assert.equal(lineAt(starts, 100), 3);
  assert.equal(lineAt([], 5), -1);
  assert.equal(lineAt([2], 1), -1);
});

test("wordTimes uses stored words, else spreads the line by word length", () => {
  const stored = wordTimes({ start: 0, end: 2, text: "oi tudo", words: [{ start: 0, end: 0.5, text: " oi", p: 1 }, { start: 0.5, end: 2, text: " tudo", p: 1 }] });
  assert.deepEqual(stored, [{ start: 0, end: 0.5, text: "oi" }, { start: 0.5, end: 2, text: "tudo" }]);
  const approx = wordTimes({ start: 10, end: 16, text: "ab abcd", words: [] });
  assert.deepEqual(approx.map((w) => w.text), ["ab", "abcd"]);
  assert.equal(approx[0].start, 10);
  assert.equal(approx[1].start, 12);
  assert.equal(approx[1].end, 16);
  assert.deepEqual(wordTimes({ start: 0, end: 1, text: "  ", words: [] }), []);
});

test("wordAt picks the word being said", () => {
  const w = wordTimes({ start: 10, end: 16, text: "ab abcd", words: [] });
  assert.equal(wordAt(w, 9), -1);
  assert.equal(wordAt(w, 11), 0);
  assert.equal(wordAt(w, 12.5), 1);
});

test("lane geometry", () => {
  assert.equal(clamp(-3, 60), 0);
  assert.equal(clamp(70, 60), 60);
  assert.equal(timeAtX(50, 200, 80), 20);
  assert.equal(timeAtX(-10, 200, 80), 0);
  assert.equal(timeAtX(500, 200, 80), 80);
  assert.equal(timeAtX(10, 0, 80), 0);
  assert.deepEqual(laneBlocks([{ start: 0, end: 10 }, { start: 30, end: 50 }, { start: 45, end: 45 }], 40), [
    { left: 0, width: 25, seg: 0 },
    { left: 75, width: 25, seg: 1 },
  ]);
  assert.deepEqual(laneBlocks([{ start: 0, end: 1 }], 0), []);
});
