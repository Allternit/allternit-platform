# Checkpoint — fix/factory-pane-hygiene (dag_181656)

Goal: land the three pane-server hygiene fixes from the 2026-10-09 post-mortem (orphaned pane server from a deleted Desktop runtime → "workspace w6 not found").

Just did: all three fixes implemented in allternit-wt-fpane (worktree of main d6799e78fd):
- Fix 1 (n_1495): ping carries `server{pid,exe,exe_deleted}` (schema/response.rs + api/server.rs); `ensure_engine_running` hard-errors on exe_deleted with restart guidance (cli/ao.rs); `pane doctor` status line shows pid/exe and flags STALE BUILD. Missing `server` block = "cannot tell", not stale (old servers still accepted).
- Fix 2 (n_937): `factoryEnginePorts(env)` = [env] || [3018 Desktop, 3011 engine default]; checkFactoryEngine probes in order, first answer wins; warn/info name every probed port. Tests +3 in test/factory/engine.test.ts (all 40 pass). PORT_REGISTRY.md row updated.
- Fix 3 (n_4316): factory_backend.rs `map_spawn_error` turns workspace_not_found into "pane died instantly after spawn — transcript names the cause: <path>"; spawn.rs appends a transcript tail (shared `transcript_tail` helper, 8 lines) to any pane.spawn error, and the existing post-spawn bail reuses the helper (5 lines, now blank-line-filtered + 300-char cap). Tests added in both crates.

Next: cargo test -p allternit-factory-pane -p allternit-factory-engine (running in bg), gizzi typecheck (bg), then fmt-check, commit, push, PR, merge, ledger attestation, worktree cleanup. NO agent spawns (Eoj directive).

Open questions: none — version-mismatch hard-fail rejected (same-version-different-build is common/harmless; exe_deleted catches the harmful case); auto-kill of orphan rejected (engine errors with guidance; Desktop owns its pane server lifecycle).

## Update — fmt incident + recovery
`cargo fmt -p allternit-factory-pane -p allternit-factory-engine` reformatted BOTH CRATES wholesale (232 files, ~15k lines): the repo has no rustfmt.toml and its hand style is not stable under the local stable rustfmt. Recovery: `git checkout -- factory/` (dropped the noise), re-applied the 16 edits by hand. LESSON: never run cargo fmt in this repo; match the surrounding hand style and keep diffs minimal. Also: cargo test surfaced `generated_protocol_schema_artifact_is_current` — the checked-in schema artifact at factory/pane/docs/next/api/herdr-api.schema.json must be regenerated with `HERDR_UPDATE_API_SCHEMA=1 cargo test -p allternit-factory-pane --lib generated_protocol_schema_artifact_is_current` whenever the API schema changes (done). A second failure (`detect::manifest::tests::older_cached_remote_manifest...`) is unrelated to this diff — verifying it fails on pristine main too (running in the shared checkout).
