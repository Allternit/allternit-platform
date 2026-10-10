# Factory pane-server hygiene — stale-build gate, doctor port, spawn error mapping (2026-10-09)

Session: 7891ba36 (Kimi Code) · Branch: `fix/factory-pane-hygiene` · PR: **#1448** · Merge: `07ed45947d` · Tracked as `dag_181656` (n_1495, n_937, n_4316)

## Why

2026-10-09 ~02:00: the Factory could not start executors — `spawning failed with "workspace w6 not found"` on `dag_418709` (artifacts v2 phases). Root cause: the Desktop updater had deleted the superseded runtime dir (`r20261006.2305-…`), but the pane server process started from it (pid 80193) survived, still answered `~/.config/ao/sessions/ao/herdr.sock`, and the current engine's `ensure_engine_running` accepted any ping answer. The orphan's TCC grants for `~/Desktop` were dead, so every pane it spawned died instantly (`getcwd … Operation not permitted` in the ao-av2-* logs); the pane server reaped the dead workspaces, and the engine's next `pane.list {workspace_id: "w6"}` returned `workspace_not_found`. Operational recovery that night (kill orphan → Desktop respawned a clean server) is **not** what this PR is — this PR makes the software catch and explain the condition.

## What changed (three fixes, all in #1448)

1. **Stale-build gate** (`factory/pane`): the ping response now carries `server { pid, exe, exe_deleted }` (new `ServerPingInfo` in `api/schema/response.rs`, filled in `api/server.rs`). `ensure_engine_running` (`cli/ao.rs`) rejects a server whose executable was deleted, with the fix in the error ("Restart Allternit Desktop…"). Old servers without the `server` block stay accepted — "cannot tell" is not "stale". `allternit-factory pane doctor` prints pid/exe on the OK line and flags `STALE BUILD` when `exe_deleted`. Version-equality was deliberately **not** a gate: nothing in-repo sets `HERDR_BUILD_ID`, so versions are just `CARGO_PKG_VERSION` and equal across builds; `exe_deleted` is the precise signal for the harmful case.
2. **Doctor port reality** (`cmd/gizzi-code/src/cli/commands/doctorChecks.ts`): `gizzi doctor` probed `ALLTERNIT_FACTORY_PORT || 3011` only, so every Desktop install (engine on `PORTS.FACTORY` **3018** — the extension bridge owns 3011, see `surfaces/allternit-desktop/src/main/config.ts`) got a false "nothing answers" warning. Now probes env-port → 3018 → 3011, first answer wins, and the warn/info names every probed port. `docs/Operations/PORT_REGISTRY.md` row updated; +3 tests (40/40 in `test/factory/engine.test.ts`).
3. **Spawn error mapping** (`factory/pane/src/factory_backend.rs` + `factory/engine/src/agents/spawn.rs`): `workspace_not_found` during spawn now reads "the pane died instantly after spawn — its transcript names the cause: <path>", and the engine appends the transcript tail (shared `transcript_tail` helper, blank-line-filtered, 300-char cap) to any pane-spawn error, so the next incident surfaces `getcwd … Operation not permitted` instead of a bare workspace id. +5 unit tests.

## Verification

- `cargo test -p allternit-factory-pane -p allternit-factory-engine` — engine 418 lib + all integration suites green; pane `api_ping` 11 / `cross_area` 9 / `live_handoff` 20 green; `herdr-api.schema.json` regenerated (`HERDR_UPDATE_API_SCHEMA=1 cargo test … generated_protocol_schema_artifact_is_current`).
- **Pre-existing flake documented:** `detect::manifest::tests::{fallback_explain_preserves_active_manifest_version, older_cached_remote_manifest_does_not_shadow_newer_bundled_manifest}` fail intermittently under full-suite parallelism and **reproduce on pristine main** (verified in the shared checkout at d6799e78fd, same as on this branch); they pass in isolation. Not caused by, and not fixed in, this PR.
- `bun test cmd/gizzi-code/test/factory/engine.test.ts` 40/40; `bun run typecheck` green.
- `node scripts/release-preflight.mjs` — 55/0 (touches the Desktop-bundled sidecar source).
- PR CI: all 7 checks green.

## Incidents / lessons

- **`cargo fmt` is a trap in this repo**: there is no `rustfmt.toml` and the tree's hand style is not stable under the local stable rustfmt — `cargo fmt -p allternit-factory-pane -p allternit-factory-engine` reformatted **232 files (+15k/−4k)**. Caught in `git status` before commit, reverted with `git checkout -- factory/`, edits re-applied by hand. **Never run `cargo fmt` here**; match surrounding style and keep diffs minimal.
- `gh pr create` against `Gizziio:branch` fails ("No commits between…") — the `Gizziio/allternit-platform` remote path is a rename redirect to `Allternit/allternit-platform`; push the branch there and use a same-repo `--head`.
- Merging a PR that includes `.steering/checkpoint.md` collides with whatever session currently has that file dirty in the shared checkout (it did). The shared checkout's uncommitted checkpoint was preserved through the pull via stash → conflict-resolved to **their** content (mine is safe in git). Consider not committing `checkpoint.md` in PRs.

## Deferred / honest notes

- The Desktop-side auto-heal (factory-engine-manager restarting the pane server when the runtime changes) was considered and left out: the engine now hard-errors with guidance, and Desktop owning its pane server's lifecycle is the cleaner line. Candidate follow-up if the orphan recurs.
- `dag_418709` (artifacts v2, 4 nodes) is **unrelated to this PR**: its phases were completed by Eoj's headless `claude -p` runs (commits on the `feat/artifacts-v2-*` branches, DONE/NOTES in `scratch/artifacts-v2/*`); the Factory pane teams brought up for it were stood down without `--rm-worktree` (work preserved). The dag nodes remain unclaimed skeletons — Eoj to decide how to close/land.
- Desktop binary rebuild from merged main (lifecycle step 8) was **not** run in this session — see the session report for why (deferred to Eoj's go-ahead at this hour).
