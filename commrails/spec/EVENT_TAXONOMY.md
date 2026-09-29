# Event Taxonomy (v1)

## Envelope
All events are appended to the Ledger as JSON objects with:
- event_id (sortable)
- ts (transaction time)
- actor (user|agent|gate)
- scope (project_id, dag_id, node_id, wih_id, run_id)
- type
- payload
- provenance (optional): prompt_id, delta_id, agent_decision_id, parent_event_id

## Core event groups

### Prompt provenance
- PromptCreated
- PromptDeltaAppended
- PromptLinkedToWork
- AgentDecisionRecorded

### DAG planning and mutation
- DagCreated
- DagNodeCreated (payload adds optional `description`, `executor` `bot:<slug>`|`ao:<harness>`)
- DagNodeRemoved (payload: dag_id, node_id, title, parent_node_id)
- DagNodeReparented (payload: dag_id, node_id, new_parent_id, old_parent_id)
- DagNodeStatusChanged
- DagNodeUpdated (payload: dag_id, node_id, patch — dag_id added 2026-09-29; older events have none)
- DagEdgeAdded (blocked_by)
- DagRelationAdded (related_to)
- DagNodeOutputRecorded (payload: dag_id, node_id, wih_id, receipt_id, blob_id, sha256, size_bytes, output_path) — emitted by `wih close --output`; the projection sets `node.output`
- DagNodeWaitGateAdded (payload: dag_id, node_id, gate_id, kind timer|github_run|github_pr|manual, description, params) — Gate 0 mutation with provenance
- DagNodeWaitGateResolved (payload: dag_id, node_id, gate_id, kind, outcome ok|failed|skipped, resolved_by `<actor_type>:<id>`, reason) — actor is the resolver (Manual: explicit user/agent; Timer: gate)

### WIH lifecycle
- WIHCreated (payload adds `resolved_prompt_path`, `template_refs` [{node_id, field, receipt_id}] when the node description had output placeholders)
- WIHPickedUp
- WIHOpenSigned
- WIHHeartbeat
- WIHCloseRequested
- WIHClosedSigned
- WIHArchived

### Judge (`spec/JUDGE.md`; actor `gate:judge` unless noted)
- JudgePolicySet (payload: dag_id, node_id|null, policy {verify off|judge, close_by any|verifier, tool_judge, max_continuations}, set_by) — actor is the setter
- JudgeVerdictRecorded (payload: dag_id, node_id, wih_id, requested_status, outcome accomplished|not_accomplished|needs_human, category, reason, backend, source command|stub|system_one|human, failure {kind timeout|error|invalid, detail}|null, node_status, note, continuations_used, max_continuations, closer, attempt)
- JudgeContinuationGranted (payload: dag_id, node_id, continuation, max_continuations, counted, by, reason, last_verdict) — actor is the continuer
- JudgeHumanResolved (payload: dag_id, node_id, decision accomplished|continue|abandon, from, to, by, reason) — actor is the user
- JudgeToolDecision (payload: wih_id, dag_id, node_id, tool, command_preview (200 chars), paths, decision allow|ask|deny, source gate2|hard_rule|judge|judge_failed, reason, backend, failure)
- WIHCloseDenied (payload: wih_id, dag_id, node_id, status, code close_by_verifier, closer, reason) — actor gate

### Runs and receipts
- RunStarted
- ReceiptWritten (node outputs: `tool: "node.output"`, payload.payload = {blob_id, sha256, size_bytes, output_path})
- RunEnded

### Leases
- LeaseRequested
- LeaseGranted
- LeaseDenied
- LeaseRenewed
- LeaseReleased
- LeaseAutoRenewed (payload: lease_id, wih_id, granted_until, extend_seconds)
- LeaseHolderHeartbeat (payload: wih_id, dag_id, node_id, agent_id, pid, host, beat_at, first) — only when the holder changes; beats themselves live in `.allternit/leases/heartbeats/<wih_id>.json`
- LeaseReclaimed (payload: lease_id, wih_id, agent_id, paths, reason, last_beat_at, holder_pid, holder_host) — after LeaseReleased
- WIHReclaimed (payload: wih_id, dag_id, node_id, reason) — followed by WIHClosedSigned with final_status RECLAIMED

### Mail logistics
- ThreadCreated
- MessageSent
- ReviewRequested
- ReviewDecision

### Vault + learning + memory
- VaultJobCreated
- VaultJobCompleted
- LearningRecorded
- MemoryCandidateExtracted
- MemoryCommitted
