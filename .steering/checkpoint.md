# WP13 — tiny-profile system-lift benchmark

## Goal
Deliver WP13 in this worktree and open a draft PR without merging.

## Just did
Read COMMON_RULES, WP13, architecture benchmark requirements, canonical BUG_FIX graph/completion policy, and auth ownership notice. Worktree starts at 5edf3f4b5. WP10 fetch failed: remote branch not published. Shared node_modules linked read-only as permitted.

Fixture milestone: six seeded TS/Vitest categories with private ground truth, restricted unified diffs and trusted independent evaluation. `bun test tests/bench/system-lift/fixtures.test.ts`: 2 pass, 0 fail, 60 assertions; each buggy target fails and each oracle repair passes target/regression.

## Next
Commit/push fixture milestone, then implement runners, metrics, CLI and offline integration tests.

## Open questions
WP10 integration and WP12 alpha validation pending upstream. allternit-commrails is unavailable on PATH; DAG registration deferred rather than building an unrelated CLI or writing shared state. Keep ground truth private. Only explicitly requested completion notes/sentinel may be written outside this worktree.
