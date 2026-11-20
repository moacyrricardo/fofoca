// Run with `npm test`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { afterSteps, peakMib, setupTitle, unavailable } from "./workflow";
import type { Workflow } from "./types";

const base: Workflow = { live_transcript: true, live_summary: true, layout: "swap", live_device: "gpu", updates: "balanced", summary_every_s: 300, speakers_after: true, notes_after: true, notes_device: "gpu" };

test("GPU memory at peak per layout", () => {
  assert.deepEqual(peakMib(base, 824, 1414), { mib: 1414, whisper: 0, llm: 1414 }); // one at a time
  assert.equal(peakMib({ ...base, layout: "gpu_both" }, 824, 1414).mib, 2238);
  assert.equal(peakMib({ ...base, layout: "gpu_transcript" }, 824, 1414).mib, 824);
  assert.equal(peakMib({ ...base, layout: "gpu_summary" }, 824, 1414).mib, 1414);
  assert.equal(peakMib({ ...base, layout: "cpu" }, 824, 1414).mib, 0);
  assert.equal(peakMib({ ...base, live_summary: false }, 824, 1414).mib, 824);
  assert.equal(peakMib({ ...base, live_summary: false, live_device: "cpu" }, 824, 1414).mib, 0);
  assert.equal(peakMib({ ...base, live_transcript: false }, 824, 1414).mib, 0); // record only
  assert.equal(peakMib(base, null, 1414).mib, null);
});

test("which layouts are available", () => {
  assert.equal(unavailable("swap", 2294, 824, 1414), null);
  assert.match(unavailable("gpu_both", 2294, 824, 1414)!, /Doesn't fit: 824 \+ 1,414 MiB/);
  assert.equal(unavailable("gpu_both", 8192, 824, 1414), null);
  assert.match(unavailable("swap", null, 824, 1414)!, /Needs a GPU/);
  assert.equal(unavailable("cpu", null, 824, 1414), null);
  assert.equal(unavailable("gpu_both", 8192, null, 1414), null); // unknown size: let the user try
});

test("banner titles and after-call steps", () => {
  assert.equal(setupTitle(base), "Swap layout · Balanced updates");
  assert.equal(setupTitle({ ...base, live_summary: false, updates: "fast" }), "Live transcript on the GPU · Fast updates");
  assert.equal(setupTitle({ ...base, live_transcript: false }), "Record only");
  assert.equal(setupTitle({ ...base, updates: "custom", layout: "cpu" }), "Everything on the CPU · Custom updates");
  assert.deepEqual(afterSteps(base).map((s) => s.name), ["Speakers", "Checked notes", "Storage"]);
  assert.deepEqual(afterSteps({ ...base, live_transcript: false, speakers_after: false }).map((s) => s.name), ["Transcribe", "Checked notes", "Storage"]);
});
