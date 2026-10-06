# Vault Pipeline + Compaction

## Trigger
- Gate initiates VaultJob on WIHClosedSigned

## Execution model (v1)
- Vault is synchronous: VaultJobCreated is emitted, archive runs inline, then VaultJobCompleted is emitted.
- No asynchronous job runner in v1 (upgrade path: queue + worker).

## Inputs
- DAG snapshot (affected node + ancestor chain + relevant edges/relations)
- Closed WIH snapshot
- Closure summary (structured, evidence-linked)
- Receipt refs or bundled receipts (configurable)

## Outputs (Vault)
- snapshots/dag.snapshot.json
- snapshots/wih.closed.json
- closure/closure.summary.json
- receipts/(refs or bundle)
- learning/*.json
- memory_candidates/*.json

## Compaction rules (Hot vs Cold)
Hot (Work/Logistics/Ledger views):
- Keep open WIHs
- Keep active DAG projections (status != CLOSED)
- Keep last N days of events locally for convenience (truth remains in ledger)

Cold (Vault):
- Store full history of closed WIHs + snapshots

## Memory extraction
- Only promote durable user facts/preferences or durable process learnings
- Produce MemoryCandidateExtracted events; gate decides MemoryCommitted

### Candidates (v1)
- `archive_wih` builds one candidate per closed WIH (`mc_<wih_id>`,
  `crate::lessons::extract_candidate`): node id/title/description, final status,
  attempts and failed attempts on the node, evidence refs, receipt ids, counts of
  the ledger event types touching the WIH/node, and a 2 KiB excerpt of the output
  this WIH recorded. No lesson text.
- It is submitted through the one `MemorySink` contract
  (`crate::lessons::sink`, S13): `VaultCandidateSink` stores it at
  `.allternit/vault/<year>/<dag_id>/memory_candidates/<candidate_id>.json`.
  Sinks accept only `pending` candidates, are idempotent and immutable per id,
  and have no commit operation. `run_memory_sink_contract` is the contract test
  every sink runs (vault + in-memory today).
- `MemoryCandidateExtracted` carries `{wih_id, dag_id, node_id, candidate_id, path, sink}`.
- Nothing emits `MemoryCommitted` in v1: a candidate reaches memory only when a
  human approves the Brain draft below.

## Lesson triage (Beacon pattern → Brain close-out)

`allternit-factory internal core lessons triage --dag <id>`:

1. For each candidate of the DAG without a `LessonTriaged` event (or all with
   `--force`), ask the local System One server
   (`POST http://127.0.0.1:7717/v1/decision`, bank `bank.lesson_worthiness`, CONFIDENCE_GATE, backend `ALLTERNIT_S1_BACKEND`) three Nouls
   over the candidate as `state` (output excerpt fenced):
   `task_success` (task completed?), `reusable_pattern` (reusable
   correction/debug pattern?), `supported_by_events` (supported by concrete
   events?). The scorer writes no lesson text.
2. Promote when `task_success >= 0.50` and the mean `>= 0.60`
   (`--task-min`/`--mean-min`).
3. Promoted → a draft in the allternit-ops `brain_update_draft` file format with
   `confirm: false`: `<brain_root>/.incoming/draft-<ms>.json`
   `{source, date, auto_apply: false, updates: [{doc: "Sessions/lessons/commrails-<dag>-<candidate>.md", action: "create-or-replace", content}], x_commrails: {…scores}}`. <!-- old-names: keep (Brain draft data key and lesson file names) -->
   The content has frontmatter (`status: draft`), an empty "Lesson" section for the
   human, the scores, and the ledger evidence. Never applied here.
4. Server unreachable → scoring is skipped and the draft is written marked
   `unscored` (humans gate every draft anyway). Rejected → no draft.
5. Every candidate gets a `LessonTriaged` event
   `{dag_id, node_id, wih_id, candidate_id, verdict promoted|rejected|unscored, scores, mean, task_min, mean_min, model, unscored_reason, draft_path}`.
6. Human review → outcome labels (`lessons outcomes`): an applied draft labels
   `reusable_pattern`/`supported_by_events` true; a draft rejected with
   `apply-brain-updates.js --reject --why …` (moved to `.incoming/rejected/`, never
   deleted) labels the named question(s) false.
