# Allternit Agent System Rails — Spec Overview (Locked Invariants)

## Layers (Unified System)

1) **Work Surface (Tickets/DAG/WIH)**
- Hierarchical DAG plan (tasks/subtasks/sub-subtasks)
- Hard dependencies: `blocked_by` edges (must be acyclic)
- Soft links: `related_to` relations (context-sharing only)
- Work identity law: `dag_id` is the canonical work ID (no separate ticket/work entity)
- Fresh execution: `execution_mode: fresh` writes a ContextPack and records `context_pack_path` on WIHCreated
- Strict provenance: DAG mutations must carry `prompt_id + delta_id` or `agent_decision_id`
- Nodes may declare `execution_mode: fresh|shared`

2) **Logistics (Mail/Envelope/Delivery)**
- Threads keyed by DAG, node, WIH, or run
- Messages may be informational or actionable
- If actionable (lease/review/approval/handoff), it **must** emit Ledger events

3) **Ledger (Authoritative)**
- Append-only events (JSONL segmented by date)
- Immutable receipts (content-addressed blobs)
- Atomic leases/reservations (durable store)
- Projections generate current views for Work + Logistics

4) **Vault (End-of-Line)**
- Triggered by WIH close
- Bundles DAG snapshot + closed WIH snapshot + closure summary + receipt refs/bundles
- Extracts learning records + memory candidates to agent memory layer
- Compacts hot workspace (keeps only open WIHs + active DAG views + recent event window)

## Gate (Policy Enforcer)

**Gate = the WIH policy enforcer for dag/node/run transitions and tool execution.**

Gate owns:
- Preconditions/postconditions
- WIH open/close signatures
- Lease checks
- Required evidence checks

No other responsibilities implied.

## Bitemporal provenance

- **Valid time**: when user said it (PromptDelta timestamp)
- **Transaction time**: when system applied it (Ledger event ordering)

Every DAG mutation event may include `provenance.prompt_id` and optional `provenance.delta_id`.
