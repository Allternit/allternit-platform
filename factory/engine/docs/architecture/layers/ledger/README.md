# Ledger Layer

## Purpose
The ledger is the append-only store of truth. Every event relating to DAG nodes, WIHs, gate actions, bus messages, iterations, and vault actions is persisted here so projections (mail, work views) are rebuildable.

## Stores
| Store | Path | Responsibility |
| --- | --- | --- |
| Ledger events | `.allternit/ledger/events/YYYY-MM-DD.jsonl` | The canonical log for DAG/WIH/loop/bus/vault events. Reader tools tail this file. |
| Cursor state | `.allternit/meta/rails_cursor.json` | Ensures the runner processes each event exactly once; includes handler checkpoints. |
| Processed registry | `.allternit/meta/processed.jsonl` or `.allternit/meta/processed.sqlite` | Stores `(event_id, handler)` pairs to avoid duplicate handling. |
| Prompt timeline | `.allternit/prompts/*` | Prompt records/deltas referencing `prompt_id`, with links to DAG mutation events. |

## Guidelines
- Store events with ULID-based IDs and include both valid-time and transaction-time stamps to reconstruct concurrency order.
- Every mutation event (dag node create/edit, dependency change) must include a `provenance` block linking to `prompt_id`/`prompt_delta_id` or `agent_decision_id`. Strict mode forbids missing provenance.
- Readers like the runner, index builder, and mail projection replay these logs for state and so must handle out-of-order files via the cursor.
