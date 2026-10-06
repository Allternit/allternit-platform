# Vault Layer

## Purpose
Acts as the compaction and learning storage for closed WIHs. The vault keeps finalized snapshots, archived receipts, and extracted learnings/memories.

## Structure
- `.allternit/vault/<year>/<dag_id>/dag.snapshot.json` — frozen DAG graph at closure. Contains linked prompt history and loop outcomes.
- `.allternit/vault/wih.closed/<wih_id>.snapshot.json` — WIH metadata plus policy rules, receipts, and completion reasoning.
- `.allternit/vault/receipt_bundle/<bundle_id>/` — copies or hashes of receipts referenced in `WIHClosedSigned`. Vault stores the manifest (receipt IDs + hashes) plus optionally zipped artifacts.
- `.allternit/vault/learning/*.json` and `.allternit/vault/memory_candidates/*.json` — extracted lessons for long-term memory; gated by configured heuristics (avoid storing noise).

## Pipeline
1. `WIHClosedSigned` triggers `VaultArchiveStarted` events via the runner. The runner verifies receipts, copies necessary blobs into the vault, takes snapshots, and writes `VaultArchiveCompleted`.
2. Learning extraction runs after archiving: generates `LearningExtracted` events plus `vault/learning/*.json` records describing what to remember next time.
3. Optionally `WorkspaceCompacted` events run to remove temporary projections or release TTL-limited indexes from `.allternit/`, keeping the active workspace nimble.

## Testing hints
- Simulate closing a WIH and assert the runner emits the full `VaultArchive…` sequence plus writes vault files listed above.
- Validate that `allternit rails vault list <dag_id>` (if implemented) reads the directories correctly and rejects missing `dag_id` entries.
