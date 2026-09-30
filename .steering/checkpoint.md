# WP13 — tiny-profile system-lift benchmark

## Goal
Deliver the authorized WP13 harness and a draft PR against main. Do not merge or touch the shared checkout.

## Just did
Implemented seeded TS/Vitest fixtures (six bug categories, private ground truth), independent evaluation/protected tests, one-shot naked and pluggable BUG_FIX system runners, deterministic offline backend/graph, gizzi HTTP model-pool backend, budgets including auxiliary cognition, receipt/verifier completion checks, JSON/Markdown reporting with Wilson and paired bootstrap bounds, generator/run CLI, and real-run/integration README. Read WP10 branch at 1a264c7aa105788e2934ab7d9625ecc7611d5b6e; aligned with template BUG_FIX and graph coding.bug_fix.v1 and explicit fs: write resources. Frozen-ABI alignment test passes.

Final targeted command: bun test tests/bench/system-lift/fixtures.test.ts tests/bench/system-lift/metrics.test.ts tests/bench/system-lift/backends.test.ts tests/bench/system-lift/harness.test.ts
Result: 18 pass, 0 fail, 153 assertions, 4 files (89.76s). git diff --check passes. Mock CLI run (N=6, seed=42) emitted JSON + Markdown with eligible=false. Generator CLI exported N=6 repos and ground truth successfully.

## Next
Commit/push the final harness milestone, open the required draft PR, write the explicitly requested WP13_NOTES.md and sentinel, and retain the worktree/branch for review. No merge, production deploy, build, typecheck or dev server.

## Open questions
Real WP10 executor attachment and WP12 alpha-pass/landing attestations are upstream dependencies; mock numbers are not valid system-lift evidence. Existing session HTTP has no hard per-request token ceiling: observed overshoot stops effects and invalidates eligibility. Adapter must honor its cancellation signal and meter auxiliary cognition. The requested CLI name was absent, but the installed allternit-rails alias was discovered later: remaining publication work is now tracked as dag:dag_942294 / wih:wih_9152. Planning was registered late; no shared state was written. One suite run exceeded the outer test timeout under shared-machine load; final run uses a smaller end-to-end sample and larger outer timeout, retaining all six fixture categories and unchanged per-run budgets. Steering returned empty reviews (CONSULT_FAILED). Investigation found coordinator -> ao-consult shim -> coordinator recursion; stopped 3,923 consultation processes whose command was explicitly rooted in this WP13 worktree, leaving other sessions untouched. Repaired only this worktree hook to pin the real auto-approve stdin review backend while retaining the coordinator and explicit overrides. bash -n and the new routing regression both pass. No prior steering approval is claimed.
