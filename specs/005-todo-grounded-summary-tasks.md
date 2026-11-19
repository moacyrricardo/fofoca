# 005 — Grounded final summary and task extraction

> **Evidence:** [`001-assets/benchmarks-mx230.html`](001-assets/benchmarks-mx230.html) §5 — every
> Qwen3.5-2B final summary contained at least one invented or wrong fact.

## Context

Small local models keep names, numbers and dates but misread slang and invent details. Notes are
only trustworthy if every claim can be checked against what was said.

## Decision

- Ask the model to **extract** (decisions, action items with owner/due, numbers & dates, short
  summary) as structured JSON, each item carrying the **transcript timestamp(s)** it came from.
- Validate references (timestamps must exist; quoted numbers must appear in the cited segment);
  drop or flag items that fail.
- UI: time chips on every claim jump to the transcript line; Tasks tab quotes the source sentence.
- Evaluate a larger model on CPU for the post-call pass only (latency is acceptable there).

## Implementation

To be detailed when picked up. Build a small pt-BR evaluation set (recording + reference facts) to
measure invented-fact rate per model/prompt.

## Known Gaps

- No write-back to task managers.
