# Agent Work Attestation — dead tests purge + orphaned Rust wiring

**Date:** 2026-10-27 23:30  
**Session ID:** test-cleanup  
**Branch:** chore/test-cleanup, fix/wire-orphan-rust (both merged, deleted)  
**Agent:** claude  
**Commit:** platform [#1926](https://github.com/Allternit/allternit-platform/pull/1926) `10da24dc`, [#1934](https://github.com/Allternit/allternit-platform/pull/1934) `ac016f5c`; ai [#861](https://github.com/Allternit/allternit-ai/pull/861) `dfb7fa58`  
**Ledger entry:** [../LEDGER.md](../LEDGER.md)

## What was done

- **#861 / #1926 — deleted tests that could not fail.** 21 Playwright debug probes (ai `tests/`), 16 dead TS tests (whole-file `describe.skip`, imports of deleted modules, no-assert placeholders), 19 Rust test fns (constructor-only, `assert!(true)`, results discarded), 10 Rust test files never compiled. Added the "tests catch bugs" rules to AGENTS.md in both repos and the workspace AGENTS.md.
- **#1934 — wired in ~15k lines of Rust that no `mod` line reached:**
  - multimodal-streaming: controller/signaling/transport/video `mod.rs` placeholders replaced with the real implementations.
  - allternit-workflows: stub template compiler/loader/validator replaced with the full versions from `templates/`.
  - allternit-runtime: `supervision/runtime.rs`, `tool_loop/tools.rs`. memory: `memory_agent_adapter.rs`. kernel-tools: `gui.rs`.
  - cloud-wizard: deleted `api.rs`, `state.rs` (never declared in `lib.rs`).
  - Bug fixes: `Storage::get_subgraph` (intent-graph-kernel) returned no edges; `store_memory` (memory v2) wrote placeholder provenance/retention_policy that `retrieve_memory` could not parse. Their `#[ignore]`s removed.

## How it works

- Scan used static signals (skip-all, orphan imports, zero assertions, mod-tree reachability from cargo metadata targets). Mock-heavy tests that guard real past bugs (platform-sw, env-vite8, wrong-conversation sends), axum route-overlap tests and opt-in live `#[ignore]` tests were kept on purpose. `vendor/` and `archive/` untouched.
- None of the #1934 crates are in allternit-api's dependency tree; only cloud-wizard is in cloud-api, and it only lost two never-compiled files.

## Verification

- #1926 Rust cuts: `cargo check --offline --tests` on the 16 touched crates — pass.
- #1934: `cargo check --offline --tests -p multimodal-streaming-execution -p allternit-workflows -p allternit-runtime -p allternit-simple-memory -p allternit-tools -p allternit-cloud-wizard -p allternit-cloud-api -p allternit-memory -p intent-graph-kernel` — pass, 0 errors (after merge).
- #1934 was merged on Eoj's instruction before the check ran.

## Known gaps / remaining work

- **#1934 tests never ran to completion.** Deferred by Eoj. Run, through the queue and with a time limit:
  - `scripts/build-queue run cargo-test -- timeout 900 cargo test --offline --no-fail-fast -p allternit-workflows -p allternit-runtime -p allternit-simple-memory -p allternit-tools -p allternit-memory -p intent-graph-kernel`
  - `scripts/build-queue run cargo-test -- timeout 300 cargo test --offline -p multimodal-streaming-execution -- --test-threads=1`
- **A multimodal-streaming test hangs forever** (cargo test sat at 0% CPU for 18 min and stalled the machine-wide build queue). Not yet identified — the `--test-threads=1` run above prints its name. Always run this crate with `timeout`.
- One earlier cargo-test run in the worktree failed (101) before the agents were stopped; which crate is unknown.
- The two bug-fix tests (`intent-graph-kernel` get_subgraph, `v2_truth_preservation`) are un-ignored but unconfirmed passing.

## Files changed

- See the three PRs above.
