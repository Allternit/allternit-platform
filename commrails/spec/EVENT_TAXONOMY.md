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
- PromptCreated (payload: prompt_id, source `cli`|`bridge`, raw_text; remote-origin prompts add `submitted_by`, `decision_ref`, `request_id` and the event actor is the remote identity, e.g. agent `bot:chief` — see `BRIDGE.md`)
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

### Mail logistics
- ThreadCreated
- MessageSent
- ReviewRequested
- ReviewDecision

### Bridge audit (`BRIDGE.md`)
- BridgeRequest (actor: the remote identity; payload: request_id, identity_id, method, path, status, scope, peer, optional target_dag, prompt_id, mail_thread, message_id, template_id; scope.dag_id set for plan requests). Payload never carries `thread_id`/`dag_id` keys so mail and DAG projections ignore it.
- BridgeRequestDenied (actor: gate `bridge`; 401s: request_id, method, path, status, reason, peer)

### Vault + learning + memory
- VaultJobCreated
- VaultJobCompleted
- LearningRecorded
- MemoryCandidateExtracted
- MemoryCommitted
