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

### Bridge audit (`BRIDGE.md`)
- BridgeRequest (actor: the remote identity; payload: request_id, identity_id, method, path, status, scope, peer, optional target_dag, prompt_id, mail_thread, message_id, template_id; scope.dag_id set for plan requests). Payload never carries `thread_id`/`dag_id` keys so mail and DAG projections ignore it.
- BridgeRequestDenied (actor: gate `bridge`; 401s: request_id, method, path, status, reason, peer)

### Vault + learning + memory
- VaultJobCreated
- VaultJobCompleted
- LearningRecorded
- MemoryCandidateExtracted (payload: wih_id, dag_id, node_id, candidate_id, path, sink — candidate stored pending via `MemorySink`)
- MemoryCommitted (not emitted in v1: commit = human-approved Brain draft)
- LessonTriaged (payload: dag_id, node_id, wih_id, candidate_id, verdict promoted|rejected|unscored, scores, mean, task_min, mean_min, model, unscored_reason, draft_path)

### Observer
- No observer-specific events: the read-only observer writes only mail
  (`ThreadCreated` if new, `MessageSent` with `from_agent: "observer"`, subject
  `observer <trigger> dag:<id> [wih:<id>] [sig:<failure signature>]`)

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
