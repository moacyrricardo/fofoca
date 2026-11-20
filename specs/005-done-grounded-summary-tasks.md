Status: done
Branch: moacyrricardo/spec-005-grounded-notes

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

## Implementation Notes

- **Schema**: `summary`, `decisions`, `action_items` (owner/due nullable), each with 1–4 `refs`
  (`mm:ss`). The separate `facts` list was dropped: numbers are checked inside every item's text
  instead, which covers the same failure (invented figures) with a simpler schema for a 2B model.
  Requested via `response_format: json_schema`; sampling temperature 0.2 for extraction.
- **Validation** (`grounding.rs`, pure, unit-tested): refs resolve to a line starting within ±2 s;
  numbers must appear in a cited line **or the line right before/after one** (measured: the model
  cited "R$120 mil" at 00:34 for a point whose "R$87.500" is at 00:39). Number reading handles
  `87.500`, `87,5 mil`, `R$87.500,00`, `1,234.5`, "cento e vinte mil", "oitenta e sete mil e
  quinhentos", "dois mil e vinte e seis", safe ordinals (primeiro/terceiro). Owners match a speaker
  display name or invitee (exact, or an unambiguous first name). Due dates are computed from the
  cited words (explicit date, weekday → next one, hoje/amanhã); the model's date is used only for
  "semana que vem"/"next week". Summary/decision items that repeat an action item are dropped.
- **Eval** (`fofoca-cli eval-notes <wav> eval/ptbr-facts.json`, 75 s pt-BR recording, Qwen3.5-2B
  on the MX230, same transcript):

  | | numbers not in transcript | reference facts covered | time |
  |---|---:|---:|---:|
  | free-text summary | 1 ("28.75") | 9/9 | 28.8 s |
  | grounded notes | 0 | 8/9 | 32.1 s |

  Number checks don't catch reworded meaning: one kept point said the client "perdeu o contrato"
  (not said). The time chips are the answer for that. An earlier run's free text also claimed
  "cancelou o contrato" and a "-10.000" variance.
- **Larger CPU model not evaluated**: no Qwen3.5-4B on this machine; the 2B extraction is the
  default. Revisit if flagged/missed rates are high on real calls.
- The final step still runs inside the session (it owns the LLM swap); `pipeline::mark_done` records
  `notes` in `steps_done`. If extraction fails, the old free-text summary is written with a notice.
  The notes are also stored as Markdown in `summaries.json` so older views keep working.
- UI: Summary renders notes (time chips → Transcript tab, line highlighted), "Couldn't verify"
  collapsed, Tasks tab with owner/due and the quoted line; "Write checked notes" button for
  meetings without notes (Tauri `write_notes`, refused while recording). Checked in the browser
  preview; not yet on a real call.
- Found while measuring: a `whisper-server` orphaned since a recording the day before still held
  813 MiB of VRAM (its app had died), which made llama-server OOM. Model servers don't die with
  their parent — follow-up needed (e.g. `PR_SET_PDEATHSIG` on spawn).
