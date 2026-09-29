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

### Campaigns (spec/CAMPAIGNS.md)
- CampaignDeclared (payload: campaign_id, definition {id, objective, owner, status active|paused, executor, command?, budget? {unit, limit, mode shared|additive, per_wake?}, dag_id?, rearm?})
- CampaignNoteAdded (payload: campaign_id, text, by)
- CampaignSpendRecorded (payload: campaign_id, entry {amount, start?, end?, resource?, note?}); the projection recomputes `budget.spent` per mode
- CampaignStatusChanged (payload: campaign_id, from, to, reason; `budget_exhausted` when spend pauses it)
- CampaignBudgetChanged (payload: campaign_id, limit, previous_limit), from `campaign resume --limit`

### Wakes (keyed queue; a new WakeScheduled for a key replaces the pending one)
- WakeScheduled (payload: wake_id, key `campaign:<id>`|`node:<dag>/<node>`, due_at, target {kind campaign|node_timer, …}, message, source, replaces). The Gate appends one after every timer `DagNodeWaitGateAdded`.
- WakeCancelled (payload: wake_id, key, reason); removes the key only if it names the pending wake
- WakeFired (payload: wake_id, key, fired_at): a sweep's claim, written before dispatch (at-most-once)
- WakeCompleted (payload: wake_id, key, outcome ran|failed|needs_you|resolved_timers|skipped|skipped_<status>, detail)

### Attention gate (agent→human notifications)
- AttentionItemSubmitted (payload: item_id, key, channel needs_you|mail, title, body, content_hash, source, submitted_at)
- AttentionItemDeferred (payload: item_id, release_at, reason quiet_hours|hourly_cap): queued, never dropped
- AttentionItemCoalesced (payload: item_id, into): same key + content hash already queued or delivered inside the dedupe window
- AttentionItemDelivered (payload: item_id, channel, delivered_at): open in needs-you; `mail` also emits MessageSent on `mail:attention`
- AttentionItemAcked (payload: item_id, acked_at, acked_by)
