---
status: implemented_awaiting_orchestrator_review_and_rust_verification
dag: dag_176229
node: n_624
wih: wih_7075
scope: bounded_core_only
files_changed:
  - .steering/checkpoint.md
  - cmd/allternit-api/src/memory_drive.rs
  - cmd/allternit-api/src/lib.rs
  - docs/MEMORY_DRIVE_CORE_NOTES.md
  - docs/MEMORY_DRIVE_CORE.sentinel
deviations:
  - No feature scope deviation from MEMORY_DRIVE_CORE_TASK.md.
  - Rust tests are written but unrun because compilation is explicitly prohibited.
remaining:
  - Shared-checkout stop-gate resolution by the orchestrator within its own authorization and branch ownership boundaries.
  - Orchestrator review of this core module.
  - Authorized Rust compilation and focused unit-test execution.
  - Subsequent bounded tasks for routes, scoped tokens, push validation, indexing and writer integration.
  - Subsequent Phase 1 tasks for gizzi checkouts, source-link routing, shared UI, user and dev guides and navigation.
  - Full four-phase dependency plan and Phase 1 completion evidence remain pending the broader task.
brain_updates: []
---

# Memory Drive core implementation

This is the deliverable in [MEMORY_DRIVE_CORE_TASK.md](MEMORY_DRIVE_CORE_TASK.md), the bounded first part of Phase 1. It does **not** attest completion of Phase 1 or integration with any current runtime writer. Work remains uncommitted in the owned platform worktree. The workspace UI worktree was preserved without feature edits. No agents were spawned.

## Shared-checkout stop gate

After the bounded-core handoff, hook `stop:4` reported shared checkout `allternit` main 56 commits behind origin/main, unmerged branches and dirty files belonging to other work. No git-discipline PASS is claimed. Those findings were recorded in the owned worktree's steering checkpoint for the orchestrator. This task explicitly confines work to the owned worktree and leaves merges unauthorized; no shared-checkout update, merge, branch deletion or allowlist change was performed. The sentinel denotes core review readiness, not successful session landing or resolution of this separate gate.

## Implemented storage boundary

`memory_drive` is exported from `allternit-api`. It uses the already available system git and existing Rust dependencies; no dependency or migration changes were made. `MemoryDrive::new` requires an absolute safe repository path, the caller's authenticated owner, and a validated branch. There is no environment-selected data directory or implicit account identity. The caller must first enforce its existing identity/ownership/team gates and select the correct owner's path. A digest in an uncommitted bare-repo owner marker prevents accidental reuse by another owner; this marker supplements authentication and is not a replacement for it.

`initialize` creates a complete bare repo in a unique sibling directory, then publishes the directory by rename. Concurrent losers inspect the winner rather than rerunning initialization on it. Repo and temporary index directories use Unix 0700. On Windows, the caller must supply a root with the appropriate account ACL. An initial `MEMORY.md` commit uses the same CAS path as every edit; concurrent seed losers return the committed winner's snapshot. Unborn heads use `rev-parse --verify --quiet` (exit 1), and ref publication supplies the zero old OID. The initial commit and the subsequent first memory edit are separate commits.

Read operations include head, sorted tree, file, complete snapshot, per-file/all-drive history and diff. Snapshot reads inspect blob modes and sizes before loading content, then validate the entire tree. Only non-executable regular UTF-8 markdown blobs are admitted; symlinks and executable blobs fail closed. History accepts only validated file paths and is capped at 100 commits. Diff accepts complete object IDs, uses no external diff/textconv driver, and validates both snapshots first.

`apply_batch` supports raw file replacement, file deletion, stable-ID entry upsert/move, and entry deletion. One successful changed batch makes one commit. Duplicate updates and repeat deletes are no-ops; a no-op still checks the expected head. Each mutation creates a private `GIT_INDEX_FILE`, writes blobs with `hash-object`, constructs the tree using `update-index`/`write-tree`, creates the commit with `commit-tree`, and atomically publishes with `update-ref <ref> <new> <expected-old>`. A competing write yields an explicit `DriveError::Conflict` with expected and actual heads. Callers must re-read, reconcile their intent and retry. There are no force operations, resets or shared checkouts. Scratch guards remove temporary initialization/index directories on normal and error returns. A losing CAS may leave unreachable git objects for normal git maintenance; it never changes the canonical ref.

`replace_snapshot` lets a future import/Dream/undo caller submit a reconciled full tree through the same CAS. It creates a **new** commit with the current expected commit as parent. It does not implement the future semantic undo algorithm: that caller still must preserve intervening changes before submission.

## Format and validation

The format follows [Agent Memory Repo SPEC.md](https://github.com/AgentMemoryRepo/agentmemoryrepo/blob/main/SPEC.md), with the stricter product constraints in the core task. `MEMORY.md` accepts `# Memory` and the standard named `# Memory: …`, has one `## Index`, and stays within 200 lines/16 KiB. The index is regenerated deterministically from topic files, linking `[[topic/path]]` without `.md`. All cross-links must resolve inside the proposed tree.

Entries are one-line bullets with source/date metadata. Rendered Allternit entries carry a stable `id` extension and support additional validated metadata. An external bullet without an `id` is accepted; its identity is derived deterministically from text, source and date, and may then be explicitly retained across updates. Product writes still require source/date. Missing historical provenance is represented explicitly as `source: imported:unknown`; no session is invented. Relative product links, HTTPS and loopback HTTP are accepted. URL credentials, unsafe schemes, multiline metadata, metadata injection and ambiguous duplicate keys fail validation.

The module rejects traversal, absolute/hidden/special paths, transcript/log filenames, scripts, control characters, raw HTML and image links. Limits: 64 KiB per file, 2 MiB per tree, 256 files, 1,000 operations per batch; the short index cap may be reached before the file-count cap. Git stdout capture is capped at 8 MiB and stderr at 64 KiB, with both pipes drained concurrently. Git uses argv/stdin, a cleared subprocess environment, explicit per-command authors, disabled interactive credential prompting, and no system/global config or hooks. No credential files are read. Error messages omit matched secrets, input and git stderr.

Every candidate file, including unchanged content, is scanned before committing. The scanner reuses `memory_kernel_service::mentions_secret` and adds punctuation-adjacent provider keys, git tokens, JWTs and private-key patterns. This remains a heuristic scanner, not a guarantee of detecting every possible secret.

## Written tests (not executed)

16 focused tests are in `memory_drive::tests` (14 on non-Unix platforms):

- Bounded subprocess capture drains all input while retaining only its limit.
- Owner-bound, idempotent initialization and initial commit/header.
- Metadata round trips and injection/date rejection.
- Published bullets without IDs and named entrypoints.
- Unix repository privacy.
- Stale revision rejection, re-read/retry and independent-entry preservation.
- Actual concurrent initialization/first-add race with retry.
- Idempotent updates/deletes, tree reads, historical files, real git diff/history.
- Secret/path/source/size rejection without changing the canonical ref.
- Single-commit batch import, replay no-op, duplicate IDs and dangling links.
- Invalid ref/OID input rejection.
- Update/delete of an externally derived identity without duplication.
- Full snapshot CAS and restoration as a new commit preserving history.
- Unchecked candidate symlink/executable/secret rejection without publishing it.
- Same-ID rejected-write retry, no duplicate and scratch cleanup.
- Symlink repository ancestor rejection.

The orchestrator's corrections in [MEMORY_DRIVE_CORE_REVIEW_FINDINGS.md](MEMORY_DRIVE_CORE_REVIEW_FINDINGS.md) were preserved: unborn-ref query, optional standard IDs, named entrypoint and Unix 0700 directories.

## Exact verification evidence

Executed on 2026-10-06 in the owned platform worktree:

```text
rustfmt --edition 2021 cmd/allternit-api/src/memory_drive.rs
exit 0; formatter completed (syntax formatting only, not compilation/type checking)

python3 surfaces/docs/scripts/check_links.py
checked 470 nav entries, 465 pages: 0 problem(s)
exit 0

git diff --check -- cmd/allternit-api/src/lib.rs
exit 0; no output

rg -c '#\[test\]' cmd/allternit-api/src/memory_drive.rs
16
exit 0
```

A `python3` inline check loaded the four changed core files, asserted terminal newlines and no trailing whitespace, and parsed the sentinel with `json.loads`:

```text
checked 4 core files: no trailing whitespace; sentinel JSON valid
exit 0
```

This covers the new untracked module and artifacts as well as the tracked export. No git commit/push/merge, cargo invocation, build, typecheck, dev server, runtime smoke test, production import, data deletion or deploy was performed. These tests are **not claimed to pass**. Formatter acceptance does not establish Rust type correctness or concurrent runtime behavior.

Pending authorization, the focused Rust verification command is:

```sh
cargo test -p allternit-api --lib memory_drive::tests -- --test-threads=2
```

This command compiles code and therefore has not been executed. Any integration smoke test/server boot also remains unauthorized. The core sentinel signals readiness for orchestrator review only; it does not unlock later phases or claim Phase 1 acceptance.
