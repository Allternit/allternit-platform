# Gate Rules (Enforcement)

## Gate 0 — Plan creation
Trigger: `allternit plan new` or equivalent.
Checks:
- PromptCreated exists (raw intent immutable)
- Create DagCreated + root node
- Link prompt → dag
- blocked_by edges remain acyclic
- mutations must include provenance (prompt delta or agent decision)
- prompt deltas and agent decisions must list linked mutation IDs (strict-mode enforcement ensures bidirectional traceability)
- the whole mutation batch is validated before anything is emitted (`plan refine`,
  `gate mutate`, template instantiation):
  - every `{{ <node_id>.output }}` / `{{ <node_id>.output_path }}` placeholder in a
    node `description` names a node that exists in the dag or is created in the same
    batch (`template_ref_unknown_node`)
  - `executor`, when set, is `bot:<slug>` or `ao:<harness>` (`invalid_executor`)
  - `add_wait_gate` targets a node that exists and is not DONE/FAILED
    (`wait_gate_unknown_node`, `wait_gate_terminal_node`); timer gates carry an
    RFC 3339 `params.until` (`wait_gate_invalid_params`)
  - rejections are `GateError { gate: "gate0.plan", code, reason, dag_id, node_id,
    details }` with `details.provenance` = the prompt/delta the batch was submitted
    under; the ledger is left untouched

Emits:
- PromptCreated
- PromptDeltaAppended (initial baseline)
- DagCreated
- DagNodeCreated (root + initial structure)
- PromptLinkedToWork

## Gate 1 — WIH pickup/open
Trigger: `allternit wih pickup <node>`
Checks (in order; each refusal is a structured `GateError { gate: "gate1.pickup", code, ... }`,
CLI exit 2 with the JSON on stderr, HTTP 409 with `gate_error`):
- elapsed timer wait-gates on the dag are resolved first (lazy resolution; emits
  `DagNodeWaitGateResolved` with actor `gate`)
- role matches owner_role (unless override policy)
- no unsatisfied wait-gate on the node (`wait_gate_unresolved`; Manual/GitHub gates block
  until resolved ok/skipped, a `failed` outcome keeps blocking)
- every blocked_by predecessor is DONE (`blocked_by_unmet`) — enforced explicitly; before
  2026-09-29 a blocked node (projected status NEW) could be picked up
- target node status is READY (`node_not_ready`)
- no active WIH already bound to node (exclusive pickup)
- output placeholders in the node `description` resolve: each `{{ <ref>.output }}` /
  `{{ <ref>.output_path }}` must name a blocked_by predecessor, transitively
  (`template_ref_not_predecessor`), with a recorded output (`template_ref_output_missing`).
  Resolution is written to the WIH context (`wih/context/<wih_id>.prompt.md`,
  `resolved_prompt_path` on WIHCreated, `resolved_description` in the ContextPack); the DAG
  is never rewritten
- emit WIHPickedUp then require WIHOpenSigned before any tool/action
- if execution_mode is fresh, write ContextPack for the WIH (includes predecessor outputs,
  see `FRESH_CONTEXT_ISOLATION.md`)
- record context_pack_path on WIHCreated for discovery

Emits:
- DagNodeWaitGateResolved (only for elapsed timers)
- WIHCreated (if absent; payload adds `resolved_prompt_path`, `template_refs`)
- WIHPickedUp
- (requires) WIHOpenSigned

## Gate 2 — PreToolUse
Trigger: any tool/action execution request
Checks:
- WIHOpenSigned is true
- tool is allowed by WIH policy
- if tool can write: lease must cover path(s)
- if merge/release: review approved if required
On denial: return structured error with gate id + reason.

## Gate 3 — PostToolUse
Trigger: tool/action completion
Checks:
- ReceiptWritten appended with content-addressed refs
- update derived status/evidence flags

## Gate 4 — WIH close
Trigger: `allternit wih close [--output <file>]`
Checks:
- required evidence satisfied (a recorded `--output` counts: its `receipt:<id>` is
  appended to `evidence_refs`)
- leases released or compatible with close policy
- node transition legal (RUNNING → DONE/FAILED)
Emits:
- (with output) ReceiptWritten (`tool: node.output`) + DagNodeOutputRecorded
- WIHCloseRequested
- WIHClosedSigned (gate attestation)
- DagNodeStatusChanged

## Gate 5 — Vault pipeline
Trigger: WIHClosedSigned
Checks:
- receipt bundle integrity
- snapshots captured
Emits:
- WIHArchived
- VaultJobCreated → VaultJobCompleted

## Gate 6 — Node removal
Trigger: `DagMutation::DeleteNode` (e.g. API `DELETE /dags/:dag_id/nodes/:node_id`).
Checks (enforced at the API surface, not the library):
- node has no active WIH (status not CLOSED/FAILED/VAULTED)
- node has no children with status other than DONE
Emits:
- DagNodeRemoved (payload: dag_id, node_id, title, parent_node_id)

## Gate 7 — Node reparent
Trigger: `DagMutation::ReparentNode` (wire op `reparent_node`; `new_parent_id: null` moves the node to top level).
Checks (enforced in the library, unlike Gate 6):
- node exists in the dag
- new parent (if any) exists in the dag
- reparent would not create a parent-chain cycle (node must not be an ancestor of the new parent)
Emits:
- DagNodeReparented (payload: dag_id, node_id, new_parent_id, old_parent_id)

Invariant: the parent chain is always acyclic. Reparenting goes exclusively through
this mutation; the node patch path (`DagMutation::UpdateNode`) must not accept
`parent_node_id`, since that would bypass the cycle check.

## Wait-gate resolution (node-scoped)
Trigger: `allternit wait-gate resolve --node <dag_id>/<node_id> <gate_id> [--outcome ok|failed|skipped] [--actor user:<id>|agent:<id>]`
(adding a gate goes through Gate 0 as the `add_wait_gate` mutation: `wait-gate add`).
Checks:
- node and gate exist (`node_not_found`, `gate_not_found`)
- gate not already resolved ok/skipped (`already_resolved`); a `failed` gate may be re-resolved
- Manual gates require an explicit non-gate resolver (`manual_resolve_requires_actor`);
  the event's `actor` is that resolver
- Timer gates resolve lazily on readiness checks (Gate 1 pickup, `wih list --ready`) once
  `params.until` has passed; GitHub gates are resolved explicitly (no poller yet)
Emits:
- DagNodeWaitGateResolved (payload: dag_id, node_id, gate_id, kind, outcome, resolved_by, reason)

Readiness: a node with any unsatisfied gate stays projected `NEW` and is excluded from
`ready_nodes` / `wih list --ready`. Ticket wait-gates (`commrails gate ...`,
`.allternit/rails/wait_gates/`) are unchanged and separate.
