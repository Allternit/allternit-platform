# Rails Runner Layer

## Purpose
`allternit rails runner` is the deterministic autopipeline orchestrating WIHs, loop iterations, vaulting, and learning extraction. It tails the ledger, processes events with idempotency, and emits the derived events that keep the system autonomous.

## Commands
- `allternit rails runner start` — long-running mode that watches `.allternit/ledger/events/*.jsonl`, updates `.allternit/meta/rails_runner_state.json`, and dispatches handlers.
- `allternit rails runner once` — single-shot run useful for testing or when daemonizing is not available. It replays the ledger up to the latest event and runs the same handlers.
- `allternit rails runner cursor` — inspect or reset the cursor state used to avoid reprocessing events.

## Loop control
- Handles `RailsLoopIterationStarted/Completed/SpawnRequested/Escalated` events emitted during each WIH execution.
- Uses `LoopPolicy` data (max iterations, fresh context cadence, escalation conditions) stored in each WIH to decide whether to spawn a new iteration or escalate with a prompt delta.
- Keeps `LoopProgress` state per WIH so it can continue even after restarts; state persisted under `.allternit/meta/rails_loop_progress.json`.

## Vault handoff
- `handle_wih_closed` emits `VaultArchiveStarted`, `VaultArchiveCompleted`, `LearningExtracted`, and optional `WorkspaceCompacted` events.
- Releases leases (`Leases::release_for_wih`) and rebuilds indexes (`Index::rebuild_from_ledger`) once closure is confirmed.
- Ensures receipts referenced in `WIHClosedSigned` exist before vaulting.
