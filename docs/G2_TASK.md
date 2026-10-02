# G2: close the three connector gaps from #1182

You are working in THIS worktree only (branch `prog-g2-connector-gaps`). A previous worker left a WIP commit touching `effects/template.rs`, `effects/thread.rs`, `executor/generic.rs` and `agency_api/mod.rs`. Read it (`git show HEAD`), keep what's right, and finish the job. Run `gh pr diff 1182` to see the connectors (`cmd/allternit-api/src/agency_api/effects/`).

## Rules (non-negotiable)
- Read `~/Desktop/Allternit/Research/agency-kernel-reconciliation-2026-09-29/tasks/COMMON_RULES.md`.
- Never touch `~/Desktop/allternit-workspace/allternit` (shared checkout). Never `git stash`. Commit and push after every meaningful step (`git push origin prog-g2-connector-gaps`).
- Use `CARGO_TARGET_DIR=$PWD/target`. Lean builds only (`cargo test -p allternit-api --lib <filter>`).
- Don't kill processes you didn't start.
- Don't change `ALLTERNIT_AGENCY_TASK_TYPES` defaults (prod stays BUG_FIX only).
- No migration unless unavoidable. If you need one, use the next free V above main's max and say so.
- Agency e2e tests that touch process env must hold `crate::agency_api::tests::E2E_ENV`. Run-starting tests use `run_limits()`.

## The three gaps
1. **Thread posts reach the live session.** Today `thread:` only appends to the thread's event ledger. Make the post also appear live in the open thread/session, the same way other bot/thread messages do (find the existing live path: thread event stream / SSE / bus used by the thread UI). It must stay idempotent: a replayed effect never posts twice live.
2. **A template that stops for attention parks the parent run instead of failing it.** When the child template run needs a person, the parent Agency run goes to `needs_attention`, linked to the child's attention item. When that is answered, the parent resumes and re-checks the child (idempotent, fenced). Only a real failure fails the effect.
3. **Computer connector against a real computer.** Add an opt-in live test: `#[ignore]` plus env `ALLTERNIT_LIVE_COMPUTER_TEST=1`. It drives `computer:local` through the real computer-use gateway with READ-ONLY actions only (screenshot, observe, cursor_position). RUN it once on this Mac against the running Desktop's computer-use gateway, and record the result.
   - NEVER send click/type/key/shell in the live test.
   - If the gateway isn't reachable, record exactly why (URL, error); don't fake it.
   - Fix any bug the live run reveals.

Keep P1's fenced/idempotent effect path and caps.

## Done means
- `cargo test -p allternit-api --lib -- agency kernel_ui` is green on two full runs in a row.
- Smoke boot:
  - `cargo build -p allternit-api --bin allternit-api`;
  - run it ~20 s with a fresh temp HOME + `ALLTERNIT_DATA_DIR` on a free port;
  - `/health/live` returns 200;
  - kill it and ONLY the `gizzi-code fabric-worker` it spawned (compare pgrep before/after).
- A DRAFT PR: `gh pr create --draft --repo Gizziio/allternit-platform`. The body starts "Merging auto-deploys prod (Contabo)." and ends with "🤖 Generated with [Claude Code](https://claude.com/claude-code)". Don't merge.
- Delete `target/` at the end.

## When finished
Write `docs/G2_NOTES.md` with:
- the PR URL;
- what you did for each gap;
- the live computer test result (exact output);
- test results (both runs);
- the smoke boot result;
- open items.

Then create the empty file `docs/G2_NOTES.md.sentinel`.
