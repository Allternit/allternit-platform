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

### Spawn gate (third-party harnesses)
- HarnessToolGated (payload: wih_id|null, harness, harness_session_id, tool, decision allow|deny, reason, paths, command) — written by `allternit-commrails hook claude-pretool` for every WIH-bound tool call and every denial (hard floor included).
- HarnessSpawnRefused (payload: wih_id, harness, reason) — an unhooked harness refused on a WIH whose policy requires leased writes.

### Drive runner (spec/DRIVE.md)
- DriveAttemptStarted (payload: dag_id, node_id, wih_id, attempt_id, attempt, executor, harness, slug, run_dir, timeout_seconds, restart_of, pid) — written before the harness spawns; an attempt without a finish whose session is gone reads as interrupted.
- DriveAttemptFinished (payload: dag_id, node_id, wih_id, attempt_id, outcome done|failed|dead|timeout|interrupted|spawn_refused|spawn_failed|close_failed|closed, exit_code, receipt_id, reason)
- DriveSpawnDeferred (payload: dag_id, node_id, reason max_concurrent|max_spawns_per_hour|global_max_concurrent|global_max_spawns_per_hour|capacity, limit, current, detail) — once per node and reason per drive process.
- DriveNeedsYou (payload: dag_id, node_id, reason harness_refused|harness_unconfigured|interrupted|attempt_failed|pickup_refused, gate_id, executor, attempt_id, detail) — paired with a manual DagNodeWaitGateAdded whose params carry `source: drive`.
- DriveBotNotified (payload: dag_id, node_id, executor, thread_id, message_id) — one typed mail per bot node.
- DriveCapacityRefused (payload: dag_id, reason) — drive refused to start (load / memory below thresholds).

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
