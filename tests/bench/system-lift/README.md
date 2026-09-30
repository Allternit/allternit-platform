# Tiny-profile system-lift harness (WP13)

Compare the **same backbone ID** on identical generated BUG_FIX tasks, once as
a naked one-shot patch generator and once through an injected `coding.bug_fix.v1`
ComputeGraph. This directory adds no production routes, authentication or
provider bindings. Nothing here deploys or merges code.

**Numbers count only after WP10 lands and WP12 passes the alpha gate.** The
offline graph is a wiring simulation, not the production BUG_FIX graph. Every
mock report is marked `eligible: false` and `HARNESS ONLY`. Its deliberately
scripted async retry demonstrates plumbing, not evidence of system lift.

## Offline run

Requires Bun, Node, Git and the root Vitest 1.6.1 dependencies. In the assigned
worktree, COMMON_RULES permits a read-only dependency symlink:

```sh
ln -s /Users/joe/Desktop/allternit-workspace/allternit/node_modules node_modules
bun test tests/bench/system-lift/fixtures.test.ts tests/bench/system-lift/metrics.test.ts tests/bench/system-lift/backends.test.ts tests/bench/system-lift/harness.test.ts
bun tests/bench/system-lift/cli.ts run --backend mock --model-id offline-fixture-backend --count 12 --seed 13 --out .tmp-wp13/results
```

Do not recreate the link if dependencies already exist. Tests, caches, generated
repos and reports stay within this worktree. No shared dependencies are modified:
fixture Vitest configuration directs caches to `.vitest-cache` inside the fixture
and disables Vitest result caching. Temporary execution repos are removed in
`finally`; JSON/Markdown reports remain at the selected output path.

Generate N standalone tiny TS/Vitest repos with ground truth:

```sh
bun tests/bench/system-lift/cli.ts generate --count 30 --seed 42 --out .tmp-wp13/fixtures
```

The generator cycles through off-by-one, null handling, wrong import, missing
async await, numeric sort and falsy default bugs. Seeds vary data and report
parameters; each repo contains one seeded defect and an unaffected regression
test. `ground-truth.json` is written **beside** the repos for inspection. It
contains the correct source, canonical unified diff and explanation. The
benchmark itself retains truth in the evaluator and includes only its digest in
the report. It never sends truth to the backend or graph adapter. Existing tests
are part of the repo given to the model; the evaluator always uses pristine tests.

## Real run with a selected small model

Use an **already running** gizzi-code HTTP endpoint configured with the chosen
small model and existing credentials. Do not start another gateway or create a
login path. Inspect its model-pool registry and select its opaque `backend_id`:

```sh
curl "$GIZZI_HTTP_URL/model-pool?role=S2&capability=cap.code.edit"
export BENCH_BACKEND_ID='the-selected-registry-backend-id'
# Optional: existing HTTP server headers, held in the environment, never in reports.
# export SYSTEM_LIFT_HTTP_HEADERS='{"authorization":"..."}'
bun tests/bench/system-lift/cli.ts run --backend http \
  --model-id "$BENCH_BACKEND_ID" --gizzi-url "$GIZZI_HTTP_URL" \
  --graph-adapter tests/bench/system-lift/wp10-adapter.local.ts \
  --count 60 --seed 42 --max-tokens 100000 --max-calls 24 \
  --timeout-ms 60000 --bootstrap-samples 10000 --out .tmp-wp13/real-results
```

The graph adapter module must exist; the harness rejects HTTP runs without it.
The CLI selects no model implicitly. The HTTP backend resolves `x-model_ref`
**from the model pool**, then calls existing `/session` and
`/session/:id/message` routes. It disables advertised tools for each isolated
generation, requests no fallback models, checks the returned identity, rejects
tool calls/multiple turns, collects token telemetry and aborts/deletes its own
session. It never calls a provider endpoint. Run against an isolated configured
HTTP context with external tools/MCP disabled, so background runtime features
cannot add work to the naked baseline. This is a text-only baseline transported
through the existing session endpoint; it still inherits that endpoint's runtime
preamble. A completely raw inference comparison would need a future model-pool
inference endpoint, not a direct-provider shortcut in this harness.

Token ceilings are enforced on reported usage before applying a patch or
starting another call. The session HTTP API has no per-request hard token ceiling;
one generation can overshoot the remaining allowance. The harness records that
usage, stops effects, and marks the report ineligible. Configure matching server
generation limits for a strict budget experiment. Call caps and HTTP timeouts
are enforced. Budget exhaustion, unknown/estimated token usage or protected-file
tampering prevent eligibility. Negative model outcomes remain observations,
not discarded samples.

## WP10 graph adapter contract

`types.ts` defines `BugFixGraph`, `GraphContext` and `GraphEvidence`.
`SystemRunner` invokes `execute(context)`; it does **not** implement a replacement
production graph. When WP10 is available, provide a module exporting:

```ts
import type { BugFixGraph } from './types'

export async function createBugFixGraph(
  options: { backendId: string },
): Promise<BugFixGraph> {
  // Bind the landed WP10 executor here, adapting its native event/receipt types.
  // Return id: 'coding.bug_fix.v1', revision: the exact graph/executor revision,
  // production: true, gates: { wp10: landing SHA, wp12: alpha-pass artifact ID }.
  // execute(context) returns resolved, verified receipt evidence.
  throw new Error('Attach the landed WP10 executor before real runs')
}
```

This is an integration sketch, not a runnable real adapter. No unimplemented
production endpoint is assumed. The WP10 branch was initially unavailable, then
published during this work. Alignment was checked at
`1a264c7aa105788e2934ab7d9625ecc7611d5b6e`: its
`commrails/src/kernel/bug_fix.rs` exports `TEMPLATE_ID = "BUG_FIX"`,
`GRAPH_ID = "coding.bug_fix.v1"`, and
`instantiate(task_id, writable_resources)`. Map the fixture task ID and
`editablePaths` to filesystem resources (`fs:src/lib.ts`), not inferred model
authority. The branch's graph includes N00–N23 plus explicit fallback/escalation
nodes. That snapshot does not expose a production benchmark executor endpoint;
the adapter must attach its eventual executor without requiring this branch to
compile. The graph ID also matches the frozen seed fixture.

The adapter must:

- Execute the actual BUG_FIX graph (normalize, diagnose, bounded patch,
  mutation gate, target verification, repair loop, affected verification,
  requirement check, diff review, verifier-owned completion, durable RunReceipt).
- Use `context.model.complete` for **all generator calls**, with
  `options.backendId`. No fallback backbone or hidden generator is allowed.
  Requests can set `stage`, `instruction`, and `responseFormat: 'text'` for
  hypotheses, reviews and other graph-node outputs; the returned `text` is
  available without applying a patch. Patch nodes use the default patch format.
  Separate decision/verifier backends must be declared in the adapter revision.
  Record their token consumption through `context.accountUsage` as work happens;
  it shares the total token budget. Do not double-count generator calls, which
  the harness meters automatically. Return `telemetryComplete: true` only when
  all cognitive usage has been accounted for. Otherwise production usage is
  reported as unavailable and the report is ineligible.
- Use `context.applyPatch` for mutations and `context.verify` for trusted test
  feedback. Respect the deadline/call/token budgets; do not spawn untracked
  processes or effects. The harness awaits adapter completion; the adapter owns
  cancellation of non-model work and must honor `context.signal`. Model calls and
  patch/verification handles reject work after the deadline. Do not route fixtures to the installed
  Desktop database, a shared repo or production API workspace.
- Resolve receipt references from the real store and validate run binding,
  identity and evidence before setting `receiptsValidated: true`. Return a
  RunReceipt, MutationReceipt and VerificationReceipt IDs; worker and verifier
  must differ and the verifier must own completion. Require all five ABI
  criteria: `target_tests_pass`, `affected_tests_pass`, `no_new_regressions`,
  `diff_review_accept`, `requirements_satisfied`.
- Fill `gates.wp10` and `gates.wp12` with auditable landing/pass references.
  These are adapter attestations, not independently fetched CI certifications.
  Missing gates keep results ineligible. Model substitution requires no graph
  edits; select a different backend ID in CLI/config.

Any CLI harness used inside the adapter launches in its auto-approve mode per
Q24. Allternit's mutation/verification gates remain authoritative.

## Scoring and interpretation

Each fixture is checked before scoring: buggy target fails and regression
passes. Both modes start from separate pristine repos. Pair order is randomized
with the recorded seed to reduce warm-up/order effects; hardware, budgets and
fixture IDs are recorded. Execution is sequential on the same machine.

Only `src/lib.ts` can be patched, via a canonical unified diff checked by Git.
Wrong paths, changes to tests/config/dependencies, symlinks and conflicting
patches are rejected. Final scoring reconstructs a fresh trusted repo, copies
candidate source, and runs the original target and regression suites. A
protected-file modification fails integrity even if its fake tests passed.
Fixture execution is for disposable trusted benchmark code; it is not an OS
sandbox for hostile arbitrary source.

`report.json` contains per-run evidence, errors, input/output/reasoning/cache
tokens, call counts and timing, plus paired summaries. `report.md` contains the
table. Fix rate means trusted target **and** regression tests pass with integrity.
Regression rate means the previously passing regression suite fails. Completion
also requires runner success and, for system mode, validated verifier-owned
receipts. Naked mode has no kernel receipts (n/a). Wall time includes graph
execution/verification and final independent evaluation, excluding setup and
baseline checks. Missing token data stays unavailable, never zero-filled.

Rates use Wilson 95% confidence intervals. System-minus-naked deltas use a
deterministic paired percentile bootstrap over fixtures (2,000 resamples by
default). The two mode outcomes for one fixture always remain paired. Positive
fix lift is better; negative regression/token/time deltas are better. Small N can
produce degenerate bootstrap bounds and wide Wilson bounds. Repeated variations
of six synthetic families are not independent evidence of general coding ability;
use the report to validate the harness, then expand tasks for substantive claims.
