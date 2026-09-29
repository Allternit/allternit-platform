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
- DagNodeCreated
- DagNodeUpdated
- DagNodeRemoved (payload: dag_id, node_id, title, parent_node_id)
- DagNodeReparented (payload: dag_id, node_id, new_parent_id, old_parent_id)
- DagNodeStatusChanged
- DagEdgeAdded (blocked_by)
- DagRelationAdded (related_to)

### WIH lifecycle
- WIHCreated
- WIHPickedUp
- WIHOpenSigned
- WIHHeartbeat
- WIHCloseRequested
- WIHClosedSigned
- WIHArchived

### Runs and receipts
- RunStarted
- ReceiptWritten
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
