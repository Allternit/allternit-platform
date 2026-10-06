# Gate Layer

## Purpose
Gate is the WIH policy enforcer that runs before any tool use, mutation, or loop iteration. It ensures AGENTS.md bundles are injected, leases are honored, and provenance/signatures are recorded.

## Responsibilities
- Validate `AgentsPolicyInjected` markers scoped to WIH, iteration, or loop context.
- Enforce `GateTurnCloseout` after every action so leases/receipts are reviewed and heartbeats updated.
- Guard all DAG mutations with `MutationProvenance` referencing the responsible prompt delta or agent decision.
- Reject commands if the required signatures, leases, or evidence are missing.

## Implementation notes
- `src/gate/gate.rs` implements the gate checks and turn closeout hooks.
- `src/policy.rs` builds the policy bundle from `AGENTS.md`, `.allternit/agents/**`, and `.allternit/spec/**`, then emits `AgentsPolicyInjected` events and stores the marker per scope.
- `allternit rails gate` commands (`pre-tool`, `mutate`, `turn-closeout`, `rules`) expose CLI hooks for policy enforcement and introspection.
- Gate uses `.allternit/meta/agents_policy_history.jsonl` (or ledger events) to prove injection happened before every mutation.

## Key files
- `src/policy.rs` – hashing, ULID generation, injection markers.
- `src/gate/turn_closeout.rs` (or functions in `gate.rs`) – lease verification, receipt validation, emission of `GateTurnCloseout` events.
- `.allternit/meta/gate_cursor.json` – tracks the last ledger event the gate processed for turn closeout enforcement.
- `.allternit/ledgers/events/` – gate reads ledger events to validate constraints; `GateTurnCloseout` is stored here.
