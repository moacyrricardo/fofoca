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

- **Input**: the transcript as numbered lines `[mm:ss] Speaker: text` (speaker names from 003,
  else You/Call), one line per segment. The line timestamp is the citation unit.
- **Output schema** (llama-server `json_schema` constrained decoding, so the 2B model can't emit
  broken JSON):
  ```json
  { "summary": [ { "text": "...", "refs": ["12:04", "12:31"] } ],
    "decisions": [ { "text": "...", "refs": ["..."] } ],
    "action_items": [ { "text": "...", "owner": "Fernanda|null", "due": "2026-11-28|null", "refs": ["..."] } ],
    "facts": [ { "kind": "number|date|money", "value": "87.500", "text": "...", "refs": ["..."] } ] }
  ```
- **Validation** (`grounding.rs`, pure functions, unit-tested):
  - every `ref` must match a transcript line start (±2 s); unknown refs are dropped, and an item
    with no valid ref is **flagged**, not silently kept;
  - `facts.value` and any number in `text` must appear in a cited line after normalization
    (`87.500`, `87500`, `87,5 mil`, spelled-out numbers via a small pt/en number parser);
  - `owner` must be a speaker or attendee name, else cleared;
  - `due` must be derivable from a cited line (absolute date or weekday/relative phrase resolved
    against `started_at`), else cleared and the phrase kept in `text`.
- **Long meetings**: if the transcript exceeds ~70 % of the context, extract per 20-minute window
  and merge (concatenate lists, dedupe by text similarity, keep all refs).
- **Storage**: `notes.json` per meeting `{ model, items…, flagged… }`; the free-text final summary
  remains as a fallback when extraction fails.
- **UI**: Summary tab renders summary bullets, Decisions, Action items, each with time chips that
  scroll to and highlight the transcript line; flagged items in a collapsed "Couldn't verify"
  section. New **Tasks** tab: action items with owner/due and the quoted source line.
- **Evaluation**: `eval/` with the pt-BR test recording's reference facts
  (`ptbr_ref` transcript + a hand-written list of facts) and the MLS file; `fofoca-cli eval-notes
  <meeting-id> <facts.json>` reports invented / wrong / missed facts. Compare: today's free-text
  prompt vs extraction, Qwen3.5-2B (GPU) vs a larger model on CPU (e.g. Qwen3.5-4B Q4_K_M, if the
  post-call wait stays under ~3 min for a 30 min meeting). Pick the default from the numbers.
- Runs as the `summary` step of the post-call pipeline (003); partial summaries during the call
  keep the light prompt.

## Known Gaps

- No write-back to task managers.
