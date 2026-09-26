# Checkpoint — session/backend-fixes

## Goal
Three gateway fixes: B1 stale defaultModel repair (config.rs), B2 tasks client-id + flexible metadata + idempotent create (task_routes.rs), B3 memory recall 500 (V183 migration cherry-pick + non-fatal entity section + migration-stack test).

## Just did
- B1: completed prior session's uncommitted diff in cmd/allternit-api/src/config.rs — stale ollama//lmstudio/ default_model replaced with claude-cli/claude-sonnet-4-6 and persisted via save_user_config; gizzi-mirror None-case unchanged.
- B2: CreateTaskRequest gained #[serde(default)] id: Option<String>; metadata widened to Option<serde_json::Value> (stringified into the TEXT column); create_task uses client id when present, INSERT ... ON CONFLICT(id) DO NOTHING then SELECT (200 existing / 201 created). cloud-api finding: cmd/allternit-cloud-api/src/db/cowork_models.rs CreateTaskRequest is a different struct (no metadata field, no id), no deny_unknown_fields anywhere in cmd/allternit-cloud-api → serde ignores unknown fields; no change needed.
- B3: V183 was free on origin/main (prev max V182) → cherry-picked fa2160e47 from session/memory-entities-summary (landed here as 6f00c3fec; blob identical, clean merge for the sibling branch). recall() entity section wrapped in non-fatal closure (warn + skip). Added migration-stack test migration_stack_adds_memory_entities_summary via DbHandle::new_memory().
- Verify: `cargo test -p allternit-api` → 1294 passed / 1 failed; the failure `aci_code::tests::host_paths_outside_the_sandbox_are_refused` is PRE-EXISTING on origin/main (lookbehind regex `(?<!...)~[/']` never compiles in Rust's regex crate, scan_patterns silently skips it; test+pattern both born in b2cfc30f3; aci_code.rs untouched by this session). New migration-stack test passes.
- Deviation (sanctioned by orchestrator): CommRails WIH DAG skipped — CLI not on PATH, build cost.

## Next
cargo build --release -p allternit-api (running), then lifecycle: commit/push/PR/merge/ledger/cleanup/discipline-check.

## Open questions
None.

---

# Checkpoint — session/subsfab-p3

## Goal
Execute docs/specs/subscription-fabric/p3/P3_PHASE_1_TASK.md: subscription-gateway worker layer + artifact store (fake-adapter tested). Deliverables: artifacts/store.ts, queue/scheduler.ts, worker/{supervisor,worker,reconcile,detach,progress}.ts, http enqueue wiring, vitest suite, P3_PHASE_1_NOTES.md sentinel.

## Just did
All deliverables implemented and verified: artifact store (content-addressed, quarantine xattr, sha256 + magic-byte MIME verify, relative local_path), pure priority scheduler, worker executor (durable two-write markSubmitted, full AdapterEvent consumption incl. quota/model/needs_user/detach, submission_ambiguous on sent_unconfirmed errors), supervisor (reconcile-before-resume gate, stall watchdog 90s/1200s config-overridable), reconcile sweep (§A2 outcomes), detach watch scheduler (backoff cap 15min, read-only watch ctx), progress bridge, http enqueue wiring. Gates: build PASS, 108/108 tests PASS (77 P1 green), no provider literals. NOTES sentinel written at docs/specs/subscription-fabric/p3/P3_PHASE_1_NOTES.md.

## Next
Session lifecycle: commit + push session/subsfab-p3, PR, merge (--merge), sync shared checkout, ledger attestation, worktree cleanup, git-discipline-check.

## Open questions
None.

---

# Checkpoint — session/gizzi-tui-parity (parallel session, keep both)

## Goal
gizzi-code TUI parity program (8 phases, owner-approved plan; dag:dag_625298 / wih_8397). P0–P3 landed (PRs #743–#746); P4 (telemetry surfaces) implemented, merging.

## Just did
- P0–P3 landed: permission bypass fix, animated startup screen, streaming/tool polish, organized /model picker (ledger summaries 0434/0442/0447/0518).
- P4 LANDED (PR #748, merge 4847f900): per-turn telemetry line + /usage Plan quota.
- P5 IMPLEMENTED (coder subagent, reviewed): /artifact rebuilt — honest finding: only local artifact writer is `gizzi html-artifact publish` (<cwd>/.gizzi/artifacts/<slug>/config.json); Gemini leftover path removed; new runtime/artifacts/browse.ts (pure: gizzi-first/claude-fallback root, canvas configs + loose .md listing, artifactInputToMarkdown inverse renderer); viewer renders via Markdown with scroll/Esc/e-open + FilePathLink; new Artifacts keybinding context; FileWriteTool created-file paths now clickable. 14 browse + 62 keybind/workspace + 9 slash-menu tests green; typecheck clean; preflight 52/0; pty smoke of list+viewer passed.

## Next
- Land P5 (commit → PR → merge → ledger), then P6: memory parity mini-audit (current memory UX vs Claude /memory + # quick-add + CLAUDE.md hierarchy and Kimi) then implement deltas.

## Open questions
- .steering/checkpoint.md is shared across parallel sessions and conflicts on every merge — resolution convention used here: keep both sessions' sections, active session on top.
