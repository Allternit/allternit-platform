# Allternit CLI Contract (v1)

This file defines **commands → gate checks → required emitted events**.
Implementations may vary, but the semantic contract must hold.

## Planning / Intent

### `allternit plan new "<text>" [--project <id>]`
Gate: Gate 0  
Required events:
- PromptCreated
- PromptDeltaAppended (initial baseline)
- DagCreated
- DagNodeCreated (root + initial structure)
- PromptLinkedToWork

### `allternit plan new ["<text>"] --template <id|path> [--param <name>=<value> ...]`
Gate: Gate 0 (same path as a normal plan: `plan new`, then one `plan refine` delta).  
Instantiates a workflow template into the WIH DAG. `<id>` resolves in
`.allternit/rails/templates/` (`<id>.json`, else `<id>.md`); an existing file path
also works. `<text>` defaults to `Template <name>: <description> (params)`.
Required events (in addition to `plan new`'s):
- PromptDeltaAppended (`instantiate template <id> (...) with k=v`) linking every mutation
- DagNodeCreated per step (child of the plan root; node id `<step_id>-<4 hex>`, one
  suffix per instantiation; `description`, `executor` carried over)
- DagEdgeAdded (blocked_by) per step `blocked_by` entry
- DagNodeWaitGateAdded per step `wait_gate`

Validation happens before the plan exists (a bad template leaves no orphan DAG):
unknown/duplicate step ids, blocked_by cycles, invalid `executor`, `{{ <step>.output }}`
refs to steps that are not transitive blocked_by predecessors, unknown `--param`
names, and missing required params (declared without `default`, or referenced as
`{{ params.<name> }}` without a declaration) are all rejected.

Prints `prompt_id`, `dag_id`, `node_id` (root), `delta_id`, `template`, then
`step <step_id> -> <node_id>` per step.

Markdown template format:
````markdown
---
name: Motion promo
description: capture -> cut -> gate-2 review -> re-record
---

Free prose is ignored. Exactly one spec block:

```yaml template-spec
params:
  - name: topic            # no default = required
  - name: length
    default: "30s"
steps:
  - id: capture
    title: "Capture {{ params.topic }}"
    description: "Screen-record the {{ params.topic }} flow"
    executor: "ao:claude"  # optional: bot:<slug> | ao:<harness> (recorded only)
  - id: cut
    title: Cut
    description: "Cut {{ params.length }} from:\n{{ capture.output }}"
    blocked_by: [capture]
  - id: review
    title: Gate-2 review
    blocked_by: [cut]
    wait_gate:             # optional: kind timer|github_run|github_pr|manual
      kind: manual
      description: "Eoj reviews the cut"
      # params: { until: "2026-10-01T09:00:00Z" }   (timer)
```
````
Existing JSON templates (`commrails template new`) load unchanged; `kind`/`priority`
(ticket-only) default when absent. Ticket instantiation (`commrails template
instantiate`) is unchanged.

### `allternit plan refine <dag_id> --delta "<text>" [--mutations <file>|--mutations-json <json>]`
Required events:
- PromptDeltaAppended
- (0..N) DagNodeCreated / DagNodeUpdated / DagNodeRemoved / DagEdgeAdded / DagRelationAdded

Notes:
- In strict provenance mode, every mutation must carry `prompt_id + delta_id` or `agent_decision_id`.
- Prompt deltas and agent decisions must list linked mutation event IDs.

Mutations JSON format:
```json
[
  {
    "op": "create_node",
    "node_id": "n_0002",
    "node_kind": "subtask",
    "title": "Draft ledger schema",
    "parent_node_id": "n_0001",
    "execution_mode": "shared"
  },
  {
    "op": "update_node",
    "node_id": "n_0002",
    "patch": {
      "title": "Draft ledger schema v2",
      "priority": 1
    }
  },
  {
    "op": "add_blocked_by",
    "from_node_id": "n_0002",
    "to_node_id": "n_0003"
  },
  {
    "op": "add_relation",
    "a": "n_0002",
    "b": "n_0004",
    "note": "related but not blocked",
    "context_share": true
  },
  {
    "op": "delete_node",
    "node_id": "n_0002"
  },
  {
    "op": "reparent_node",
    "node_id": "n_0003",
    "new_parent_id": "n_0001"
  }
]
```

### `allternit plan show <dag_id>`
Reads projection (no events). Nodes show `description`, `executor`, `output`
(receipt/blob/output_path of the latest recorded output) and `wait_gates`
(with `outcome`, `resolved_by`) when set.

### `allternit node add --dag <dag_id> --parent <node_id> --title <t> [--description <d>] [--executor bot:<slug>|ao:<harness>]`
Gate 0 refine with one CreateNode. `--description` may contain
`{{ <node_id>.output }}` / `{{ <node_id>.output_path }}`.

### `allternit dag render <dag_id> [--format md|json]`
Reads projection; may emit no-op events only if you choose to log reads (optional).

## Work / WIH

### `allternit wih list --ready [--dag <dag_id>]`
Uses DAG projection readiness derivation: blocked_by predecessors DONE and no
unsatisfied node wait-gate. Elapsed timer gates are resolved lazily first
(emits DagNodeWaitGateResolved, actor gate); otherwise no events.

### `allternit wih pickup <node_id> --dag <dag_id> --agent <agent_id> [--role <role>] [--fresh]`
Gate: Gate 1  
Required events:
- WIHCreated (if absent)
- WIHPickedUp
- (must be completed before tool execution) WIHOpenSigned

Notes:
- `--fresh` forces `execution_mode: fresh` and writes a ContextPack for the WIH
  (includes `dependency_outputs` and `resolved_description`).
- `--role` must match `owner_role` when set on the node.
- Refusals are structured (`GateError`, exit code 2, JSON on stderr):
  `wait_gate_unresolved`, `blocked_by_unmet`, `node_not_ready`,
  `template_ref_not_predecessor`, `template_ref_output_missing`.
- When the node description has output placeholders, prints
  `resolved_prompt_path: <path>` and the resolved text after
  `--- resolved prompt ---`.

### `allternit wih sign-open <wih_id>`
Required events:
- WIHOpenSigned

### `allternit wih context <wih_id>`
Reads ContextPack if available, and the resolved prompt if the WIH has one (no events).

### `allternit wih close <wih_id> DONE|FAILED [<evidence ref>...] [--output <file>]`
Gate: Gate 4 → Gate 5  
`--output` stores the file's text as the node output (immutable blob +
`node.output` receipt, derived view `nodes/<node_id>.out.md`) and counts as
evidence. HTTP: `POST /v1/wihs/:wih_id/close` (service) and the API close route
accept `"output": "<text>"`.
Required events:
- (with output) ReceiptWritten + DagNodeOutputRecorded
- WIHCloseRequested
- WIHClosedSigned (gate attestation)
- DagNodeStatusChanged
- WIHArchived
- VaultJobCreated → VaultJobCompleted

## Node wait-gates

### `allternit wait-gate add --node <dag_id>/<node_id> timer|github-run|github-pr|manual [--description <d>] [--until <rfc3339>] [--repo <owner/repo>] [--run-id <id>] [--pr <n>]`
Gate 0 refine (`add_wait_gate` mutation, prompt delta provenance). Prints `gate_id`.
Required events:
- PromptDeltaAppended
- DagNodeWaitGateAdded

### `allternit wait-gate resolve --node <dag_id>/<node_id> <gate_id> [--outcome ok|failed|skipped] [--actor user:<id>|agent:<id>] [--reason <text>]`
Manual gates require `--actor` (bare id = user). Required events:
- DagNodeWaitGateResolved

### `allternit wait-gate list --node <dag_id>/<node_id> | --dag <dag_id>`
Reads projection (no events).

### `allternit wait-gate pending [--json]`
Unresolved Manual node gates across all dags ("needs you"; no events). The API
visibility DTO (`GET /api/commrails/visibility`) appends the ones whose upstream is
DONE to `needsYou` with `reason: "manual_gate"` and a `node` join.

Timer gates also register a keyed wake (`node:<dag_id>/<node_id>`, event
WakeScheduled), so `wake run-due` flips readiness without polling.

## Campaigns, wakes, attention (spec/CAMPAIGNS.md)

Config: optional `.allternit/rails/automation.yaml`. Defaults: no executor
runs, 7-day check ceiling, UTC, no quiet hours, 24h dedupe, 6/hour cap.

### `allternit campaign declare --file <yaml|json> | --id <id> --objective <text> --executor bot:<slug>|ao:<harness>|command [--command <sh>] [--owner <o>] [--budget-unit <u> --budget-limit <n> [--budget-mode shared|additive] [--per-wake <n>]] [--dag <dag_id>] [--rearm-every-secs <n>] [--paused]`
Required events: CampaignDeclared (+ WakeScheduled when active with `rearm`).
Examples: `docs/examples/campaigns/*.yaml` (both ship paused).

### `allternit campaign list [--json]` / `campaign status <id> [--json]`
Projection from the ledger (no events).

### `allternit campaign check-later <id> <delay_secs> [--message <m>]`
Arms the campaign's one pending check and replaces any pending one. Delay is
clamped to [60s, ceiling]. Required events: WakeScheduled.

### `allternit campaign note <id> <text>`
Required events: CampaignNoteAdded.

### `allternit campaign spend <id> <amount> [--start <rfc3339> --end <rfc3339> --resource <r>] [--note <n>]`
Required events: CampaignSpendRecorded. When the budget is exhausted, also
CampaignStatusChanged(paused, budget_exhausted) and an attention item.
Budgets do not cap provider bills.

### `allternit campaign pause <id> [--reason <r>]` / `resume <id> [--limit <n>]` / `kill <id> [--reason <r>]` / `finish <id> [--reason <r>]`
Required events: CampaignStatusChanged. `resume --limit` emits
CampaignBudgetChanged first. Resume with `rearm` emits WakeScheduled.
Kill/finish emit WakeCancelled for the pending check.

### `allternit wake list [--json]` / `wake due [--at <rfc3339>] [--json]`
Pending wakes (one per key), plus `unfinished` (claimed by a sweep that never
completed). No events.

### `allternit wake run-due [--at <rfc3339>] [--json]`
One sweep under `.allternit/rails/wakes/sweep.lock`. A concurrent sweep prints
`locked` and exits 0. Required events per fired wake: WakeFired, then
WakeCompleted. `bot:`/`ao:` executors and non-allowlisted commands raise an
attention item instead of running. The sweep also releases due attention items.

### `allternit wake cancel <key> [--reason <r>]`
Required events: WakeCancelled.

### `allternit attention list [--open] [--json]`
Projection (no events). `--open` = delivered, not acked (the needs-you set).

### `allternit attention submit --key <k> --title <t> --body <b> [--channel needs-you|mail] [--source <s>]`
Required events: AttentionItemSubmitted, then one of AttentionItemDelivered,
AttentionItemDeferred or AttentionItemCoalesced (`mail` delivery also emits
MessageSent on `mail:attention`).

### `allternit attention release [--at <rfc3339>]` / `attention ack <item_id> [--by user:<id>]`
Required events: AttentionItemDelivered/Deferred; AttentionItemAcked.

## Leases / Reservations

### `allternit lease request <wih_id> --paths "<glob>" [--ttl <sec>]`
Required events:
- LeaseRequested
Followed by:
- LeaseGranted or LeaseDenied

### `allternit lease release <lease_id>`
Required events:
- LeaseReleased

## Mail / Logistics

### `allternit mail thread ensure --topic dag:<dag_id>|wih:<wih_id>`
Required events:
- ThreadCreated (if missing)

Notes:
- `thread_id` is deterministic from topic: `dag:<dag_id>` or `wih:<wih_id>`.

### `allternit mail send <thread_id> --body <file> [--attach <ref...>]`
Required events:
- MessageSent

### `allternit mail request-review <thread_id> --wih <wih_id> --diff <ref>`
Required events:
- ReviewRequested

### `allternit mail decide <thread_id> --approve|--reject --notes <file>`
Required events:
- ReviewDecision

## Ledger / Audit

### `allternit ledger tail [--n 50]`
Reads events.

### `allternit ledger trace --node <node_id>|--wih <wih_id>|--prompt <prompt_id>`
Reads events and correlates provenance.

## Vault

### `allternit vault status`
Reads vault jobs projection.

Flags:
- `--json` outputs a machine-readable summary.

## Gate

### `allternit gate status`
Returns gate configuration and active policy constraints.

### `allternit gate check <wih|run>`
Runs gate checks and returns pass/fail with reasons.

### `allternit gate rules`
Lists active gate rules.

### `allternit gate verify`
Runs invariant verification (optional).

Flags:
- `--json` outputs a machine-readable summary.

### `allternit gate decision "<note>" [--reason <text>] --link <event_id>...`
Records an explicit agent decision with linked mutation event IDs and returns a `decision_id`.

### `allternit gate mutate --dag <dag_id> "<note>" [--reason <text>] --mutations <file>|--mutations-json <json>`
Creates an AgentDecisionRecorded event (with linked mutation IDs) and then emits the
mutations using that decision as provenance.
