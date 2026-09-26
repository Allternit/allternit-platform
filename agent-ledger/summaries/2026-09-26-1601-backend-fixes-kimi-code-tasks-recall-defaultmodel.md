# Attestation — session/backend-fixes (Kimi Code)

- **Date:** 2026-09-26 (attestation written 16:01 local)
- **Agent:** Kimi Code (subagent of the orchestrating session)
- **Branch / PR:** `session/backend-fixes` → **PR #765**, merge commit **0c00be16b** (merge, not squash)
- **Scope:** three gateway (`allternit-api`) backend fixes — stale defaultModel repair (B1), tasks client-id/flexible-metadata/idempotent-create (B2), memory recall 500 (B3).

## What was done

### B1 — stale defaultModel repair
`cmd/allternit-api/src/config.rs:253-300`. The live user config
(`~/Library/Application Support/allternit/config.json`) held
`defaultModel: "ollama/llama3.2:3b"` (verified directly) — a local-provider
lane unservable through the cowork bridge, so sessions died with
`ProviderModelNotFoundError`. `AppConfig::load` now treats `ollama/` /
`lmstudio/` defaults as stale: replaces with `claude-cli/claude-sonnet-4-6`
(matches the desktop CoworkRoot fallback; deliberate decision — gizzi's local
model is NOT mirrored) and persists via the existing `save_user_config`. The
gizzi-mirror behavior for the `None` case is unchanged. (Completed a partial
uncommitted diff left by the prior session rather than redoing it.)

### B2 — tasks: client id + flexible metadata + idempotent create
`cmd/allternit-api/src/task_routes.rs:44-71` (`CreateTaskRequest`),
`:207-301` (`create_task`).

- `#[serde(default)] id: Option<String>` added; used when present, UUID
  fallback otherwise.
- `metadata` widened `Option<String>` → `Option<serde_json::Value>`;
  object/array payloads (previously 422) are stringified into the existing
  `metadata TEXT` column (present since V1 — no migration needed).
- INSERT is now `ON CONFLICT(id) DO NOTHING` followed by SELECT: **201** with
  the created task on insert, **200** with the existing row on conflict;
  audit log only on a real insert.
- **cloud-api tolerance finding:** `services/cloud-api` does not exist in
  this repo; the cloud API is `cmd/allternit-cloud-api`. Its task DTO
  (`src/db/cowork_models.rs:913`) is a different struct with no `id`/`metadata`
  fields, and there is **no `deny_unknown_fields`** anywhere in
  `cmd/allternit-cloud-api` — serde ignores unknown fields by default, so the
  widened gateway contract cannot break it. No cloud-api change made.

### B3 — memory recall 500
Root cause: `memory_entities.summary` missing on existing DBs — V1 created
the table without it, V86's `CREATE TABLE IF NOT EXISTS` is a no-op on
existing DBs → `no such column: summary` on every recall/entity list.

- **Cherry-picked `fa2160e47`** from `session/memory-entities-summary`
  (parallel Claude session) so both branches add the identical file
  `cmd/allternit-api/migrations/V183__memory_entities_summary.sql` → clean
  merge. **V183 was free** on `origin/main` (previous max V182), re-verified
  after main moved mid-session.
- `src/memory_kernel_service.rs:429-483` — `recall()`'s entity section is now
  a fallible closure: on error it `warn!`-logs and skips entities instead of
  500ing the whole recall (facts/observations still return).
- New migration-stack test
  `memory_kernel_service::tests::migration_stack_adds_memory_entities_summary`
  boots the full embedded chain via `DbHandle::new_memory()` and asserts
  `memory_entities.summary` is selectable. Green standalone and in the full
  suite.

## Verification

- `cargo test -p allternit-api` (full suite, run twice — once pre-merge-sync,
  once on the exact merged tree): **1293–1294 passed**; failures:
  - `aci_code::tests::host_paths_outside_the_sandbox_are_refused` —
    **pre-existing** (fails deterministically; `aci_code.rs` byte-identical
    to `origin/main`, `git diff origin/main HEAD -- aci_code.rs` empty;
    unrelated pure string-validation logic).
  - `webhook_subscription_routes::tests::signed_delivery_to_matching_subscriptions`
    — **flaky, not ours**: passed in the first full run and passes standalone;
    timing-sensitive webhook delivery under parallel test load; file identical
    to `origin/main`.
- `cargo build --release -p allternit-api` — **compiles** (85 pre-existing
  warnings; sqlx-postgres future-incompat note pre-existing). Built again
  from detached `origin/main` post-merge so
  `$CARGO_TARGET_DIR/release/allternit-api` matches merged main (orchestrator
  deploys it, not this session).

## Incidents

- **Shared-checkout sync blocked (lifecycle step 6/7 deviation).**
  `git -C /Users/joe/altw/allternit pull --ff-only` aborted: the shared
  checkout holds a large amount of other live sessions' uncommitted in-flight
  work (modified `memory_kernel_service.rs`, deleted
  `V183__memory_entities_summary.sql` / `V184__memory_maintenance.sql`,
  `memory_extraction.rs`, gizzi-code TUI files, ACU python, `pnpm-lock.yaml`,
  etc.) that overlaps the merge. Per the worktree-ownership rules this
  session did not stash/commit/revert any of it. Consequently the ledger
  attestation — normally a direct commit on main in the shared checkout with
  `STEER_GUARD_OFF=1` — was landed via PR from the session worktree instead
  (same content, same conventional message, reviewable trail). Syncing the
  shared checkout remains **owed** once the in-flight work there is committed
  or cleaned by its owning sessions.

## Honest deferrals

- **CommRails WIH DAG skipped** (CLI not on PATH; build cost) — deviation
  sanctioned by the orchestrating session; recorded here per instruction.
- Pre-existing `aci_code` test failure and flaky
  `signed_delivery_to_matching_subscriptions` left as-is (not this session's
  scope; evidence above).
- Shared-checkout `pull --ff-only` not completed (see Incidents) — the next
  session with a clean shared tree should run it and
  `scripts/git-discipline-check.sh`.
