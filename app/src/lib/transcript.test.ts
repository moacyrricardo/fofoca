// Run with `npm test`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { insertByStart } from "./transcript";

const line = (start: number, text: string) => ({ start, text });

test("in-order lines are appended", () => {
  const lines = [line(0, "a")];
  assert.equal(insertByStart(lines, line(4, "b")), 1);
  assert.deepEqual(lines.map((l) => l.text), ["a", "b"]);
});

test("a call turn finishing after your later reply goes back to its place", () => {
  const lines = [line(0, "oi"), line(12, "sua resposta")];
  assert.equal(insertByStart(lines, line(5, "a fala longa")), 1);
  assert.deepEqual(lines.map((l) => l.text), ["oi", "a fala longa", "sua resposta"]);
  assert.equal(insertByStart(lines, line(-1, "antes de tudo")), 0);
  assert.equal(lines[0].text, "antes de tudo");
});

test("equal starts keep arrival order", () => {
  const lines = [line(3, "first")];
  insertByStart(lines, line(3, "second"));
  insertByStart(lines, line(3, "third"));
  assert.deepEqual(lines.map((l) => l.text), ["first", "second", "third"]);
  assert.equal(insertByStart([], line(1, "x")), 0);
});
