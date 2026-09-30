# Storage Layout (Locked)

Authoritative truth (append-only):
- `.allternit/ledger/events/YYYY-MM-DD.jsonl`

Atomic correctness (transactional):
- `.allternit/leases/leases.db`

Immutable evidence blobs (generated IDs):
- `.allternit/receipts/<receipt_id>/receipt.json`
- `.allternit/blobs/<blob_id>`

Fast retrieval (derived, rebuildable):
- `.allternit/index/index.db` (SQLite FTS)

Mail threads (derived view):
- `.allternit/mail/threads/<thread_id>.jsonl` (projection from ledger events)

Context packs (derived view):
- `.allternit/work/dags/<dag_id>/wih/context/<wih_id>.context.json`

Resolved node prompts (derived view, written at Gate 1 pickup when the node
description has `{{ <node_id>.output }}` / `{{ <node_id>.output_path }}` placeholders):
- `.allternit/work/dags/<dag_id>/wih/context/<wih_id>.prompt.md`

Node outputs:
- authoritative: the immutable blob `.allternit/blobs/<blob_id>` behind a
  `node.output` receipt `.allternit/receipts/<receipt_id>/receipt.json`
  (`outputs_ref: "blob:<blob_id>"`), referenced by `DagNodeOutputRecorded`
- derived view (stable path, rebuildable from the blob; rewritten from the blob if
  missing or stale when an `output_path` placeholder is resolved):
  `.allternit/work/dags/<dag_id>/nodes/<node_id>.out.md`
- re-closing a node with a new output appends a new blob/receipt; the latest
  `DagNodeOutputRecorded` wins and the view is overwritten

Workflow templates (JSON `<id>.json` or markdown `<id>.md`):
- `.allternit/rails/templates/`

Notes:
- Ledger is the single source of truth for state transitions.
- Leases are authoritative for locks only.
- Receipts and blobs are immutable evidence referenced by events.
- Index and mail thread files are projections and can be rebuilt.
- Tests: `tests/invariants.rs::authoritative_stores_are_created` asserts that ledger JSONL files, `leases.db`, receipt directories, and CAS blobs exist once the stores are initialized.
