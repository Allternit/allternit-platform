# P4 Phase 1 — Real Router + Quota Pools (subscription-gateway)

You are working in a git worktree of `Allternit/allternit-platform` (branch `session/subsfab-p4`,
cut from main @ 16b0d96e2). Zero prior context — this file is your whole brief. Read the files
it points at before writing code.

## What this project is

The Subscription Capability Fabric: a local gateway daemon that routes capability tasks
(chat, image, slides…) across a user's existing provider subscriptions instead of metered
APIs. P0–P3 are merged: zod contracts package, gateway daemon (HTTP/SSE, SQLite queue,
worker supervisor, reconcile), adapter SDK, chatgpt-web adapter, and an `allternit subs|caps|task|artifacts`
CLI. Tonight you build **P4 Phase 1: the real capability router + quota-pool state machine**,
replacing the static stub. Phase 2 (observability + model catalog) is a separate brief.

## Repo rules (hard)

- **pnpm only.** Never npm/npx/yarn. Node/pnpm are already installed.
- Run `pnpm install` at the repo root of THIS worktree first.
- **Never run `playwright install`** — the browser CDN hard-stalls on this network.
  Tests use system Chrome via the SDK helper (`channel: "chrome"`); don't change that.
- Do NOT commit or modify anything under `surfaces/`, `cmd/allternit-api/`, or other
  services — other live sessions own those dirty files.
- Do NOT push, do NOT open PRs, do NOT merge. Leave commits on `session/subsfab-p4`.
- Commit with conventional format, e.g. `feat(subscription-gateway): real capability router + quota pools (P4 phase 1)`.
- Do not touch `platform/packages/subscription-fabric-contracts/` unless a type gap makes
  progress impossible; if you hit one, note it in NOTES and work around it gateway-locally.

## Grounding — read these first

1. `docs/specs/subscription-fabric/REVIEW_CLAUDE.md` **§A2** (line ~418) — normative
   `CapabilityRouter` / `RouteDecision` / `RouteCandidate` interface. Already implemented
   as zod schemas in `platform/packages/subscription-fabric-contracts/src/routing.ts` — import from there.
2. Same file **§A4** (line ~448) — normative routing-under-quota-uncertainty rules. Your
   implementation must follow it literally; the test table below checks each rule.
3. `platform/packages/subscription-fabric-contracts/src/quota.ts` — `QuotaPool` (states:
   `available|estimated|unknown|degraded|cooling_down|exhausted`, `local_budget`,
   `local_used_in_window`, `cooldown_until`, `last_signal`, `reset_at`) and `QuotaSignal`
   (kinds: `limit_banner|hard_error|model_downgraded|slow_mode|reset_notice|counter_visible`).
4. `platform/packages/subscription-fabric-contracts/src/task.ts` — `Task` (note
   `constraints.sensitivity`, `priority: interactive|normal|background`,
   `requested_model_class`, `options`) and `TaskAttempt` (note failure classes).
5. `platform/packages/subscription-fabric-contracts/src/account.ts` — `Account`, `SessionHealth`.
6. `platform/packages/subscription-fabric-contracts/src/manifest.ts` — `AdapterManifest`
   (capabilities, lane, adapter_id, version).
7. `services/subscription-gateway/src/router/resolve.ts` — the static stub you replace.
8. `services/subscription-gateway/src/queue/scheduler.ts` and `src/worker/` — how attempts
   are dispatched today; wire `resolve()` in at task-pick time (policy re-check per hop per §A2).
9. `services/subscription-gateway/test/` — existing test layout (vitest). Match it.

## Deliverables (all inside `services/subscription-gateway/`)

### 1. `src/router/` — real `FabricRouter implements CapabilityRouter`

Pure, deterministic, no I/O, no clocks read directly:

- Constructor takes `{ now?: () => Date, idGen?: () => string, policy?: RoutingPolicy }`
  with test-friendly defaults. `RoutingPolicy` (gateway-local type) carries:
  `metered_approval_threshold_usd`, `allow_metered`, `model_class_rank: Record<string, number>`
  (so "reasoning ≥ X" comparisons are data, not code), lane cost order
  (`subscription < local < credits < metered`).
- `resolve(task, snapshot)`:
  - Enumerate every `(manifest, account)` pair from the snapshot. For each, either emit a
    `RouteCandidate` or append to `rejected[]` with a specific `RejectReason`
    (`capability_not_offered`, `account_disabled`, `adapter_disabled`, `health_not_ready`,
    `pool_exhausted`, `pool_cooling_down`, `pool_degraded`, `sensitivity_blocked`,
    `metered_not_allowed`, `approval_required`, `ui_drift` — all already in the schema).
    Every considered pair must end up in exactly one place — no silent drops.
  - Pool eligibility per §A4 (see Deliverable 2 for the state machine; here consume pool state):
    - `unknown` pools ARE eligible, ranked below `available`/`estimated` on the same lane,
      but above any worse-cost lane. Uncertainty alone never pushes work to metered.
    - `local_used_in_window ≥ local_budget` (when budget set) → drop in rank, NOT excluded.
    - `degraded` → `interactive` priority tasks may use the pool; `background` tasks reject
      with `pool_degraded`. (`normal` behaves like `interactive` for v1 — say so in NOTES.)
    - `model_downgraded`: pool counts as `exhausted` for tasks whose
      `requested_model_class` rank ≥ the policy's reasoning threshold, until `reset_at`
      (or 3 h default from signal).
    - `cooling_down`/`exhausted` with unexpired `cooldown_until`/`reset_at` → reject
      (`pool_cooling_down` / `pool_exhausted`).
  - Sensitivity: `constraints.sensitivity` of `local_only`/`confidential` → reject
    ui_bridge/subscription lanes with `sensitivity_blocked` unless policy explicitly allows
    (default deny). Metered/local lanes unaffected. §A2: policy must be re-checkable on
    every hop — `fallbacks` must be pre-filtered by the same rules (a fallback that would
    fail sensitivity must not appear).
  - Metered lane: rejected with `metered_not_allowed` when `allow_metered` false; else kept
    with `requires_approval: true` when `est_metered_usd` ≥ threshold.
  - Circuit breaker (§A4 last bullet): adapter version with > 3 consecutive
    `selector_not_found`/`provider_ui_changed` failures across any capability → state
    `ui_drift`, reject with `ui_drift`, no routes until health shows probe passed
    (use `session_health[account_id]` fields — read the schema; if it lacks a
    probe-passed marker, consume whatever indicates recovery and note the mapping in NOTES).
  - Ranking: lane cost class first, then pool state rank
    (`available > estimated > unknown > degraded`), then `local_budget`-breached
    demotion, then stable tiebreak by `adapter_id`+`account_id` (deterministic — no
    `Math.random`). `primary` = head, rest become `fallbacks`.
  - `explain`: human-readable sentence summarizing the pick and the top rejection reason.
- `onAttemptFailed(task, attempt, snapshot)`: map the attempt's failure class → either
  `"stop"` (non-retryable: policy_denied, sensitivity_blocked, approval declined) or a
  fresh `resolve()` on the post-failure snapshot with the failed (adapter,account) pair
  rejected (so the next candidate is chosen). Hard-limit/`hard_error` failures feed the
  pools module via the worker — the router itself stays pure.

### 2. `src/pools.ts` — quota-pool state machine (pure functions)

- `applySignal(pool, signal, now)` → updated `QuotaPool`, implementing §A4 literally:
  - `limit_banner`/`slow_mode`/`counter_visible` → `degraded`.
  - `hard_error` → `exhausted` until `reset_at` if the error names one (parse from
    `raw_excerpt` best-effort), else `cooling_down` with the backoff ladder.
  - `model_downgraded` → `degraded`, plus `reset_at` = signal time + 3 h default.
  - `reset_notice` → back to `unknown` (NOT `available` — A4: never trust without a real task).
  - Cooldown ladder: 30 m → 1 h → 2 h → 4 h, doubling, capped at 24 h. Track the current
    rung on the pool (e.g. in a gateway-local map or a field you can derive — the contract
    has no rung field; keep rung state in the gateway's SQLite store, pass it in).
  - Cooldown expiry recomputed lazily by consumers via `effectiveState(pool, now)`: expired
    cooldown → `unknown`, never `available`.
  - No active probing anywhere: recovery is only ever back to `unknown`.
- `recordLocalUse(store, pool_key, now)` — increments `local_used_in_window` on a rolling
  window (window seconds from the pool record; persist window anchor in the store).

### 3. Wire-in

- `src/queue/scheduler.ts` (and the worker dispatch path — read it first): replace any
  "pick the only registered adapter" logic with `router.resolve(task, snapshot)` at pick
  time; on attempt failure call `onAttemptFailed`. Snapshot assembly (accounts+manifests+
  pools+health) belongs in a small `src/router/snapshot.ts`. Feed quota signals from
  adapter/attempt results into `applySignal` at the worker boundary (where P3 already
  surfaces failure classes). Keep the change minimal — no scheduler redesign.
- `/v1/capabilities` route (`src/http/routes_capabilities.ts`): extend the response with
  per-entitlement pool state + reject reason when unavailable (drives the CLI picker and
  the P4 Phase 2 catalog). Backwards-compatible shape (add fields, don't rename).

### 4. Tests — table-driven, `services/subscription-gateway/test/router/` + `test/pools/`

Minimum scenarios (the plan's verify list — each is a named table case):

1. unknown-pool ranking: unknown pool on subscription lane beats metered lane, loses to
   available/estimated on same lane.
2. degraded pool: interactive task may route; background task gets `pool_degraded` rejection.
3. cooldown backoff ladder: successive hard errors → 30 m, 1 h, 2 h, 4 h; cap 24 h; expiry
   → `unknown`; a successful task after expiry resets the rung.
4. circuit breaker: 4 consecutive `selector_not_found` across capabilities → `ui_drift`
   rejection; health flip to probe-passed → eligible again.
5. sensitivity exclusion: `confidential` task rejects subscription-lane candidates with
   `sensitivity_blocked`, fallbacks contain no ineligible lane, metered/local eligible.
6. `rejected[]` explanations: for a snapshot with mixed states, every non-routed pair
   appears with the correct reason; `explain` names the top reason.
7. simulated `model_downgraded` event flips pool to degraded; reasoning-class task treats
   pool as exhausted; below-reasoning class task still routes; after `reset_at` → `unknown`.
8. `local_budget` breach: pool demoted but still routable when it's the only candidate.
9. `onAttemptFailed`: retryable failure re-routes to next fallback; non-retryable → `"stop"`.

Run: `pnpm -F subscription-gateway build` then `CI=1 pnpm -F subscription-gateway test`.
Existing suites (128 gateway + others) must stay green.

## Hard gates before you write NOTES

- `grep -rniE 'chatgpt|claude|kimi|gemini|grok|deepseek|openai|anthropic' services/subscription-gateway/src/router services/subscription-gateway/src/queue services/subscription-gateway/test/router services/subscription-gateway/test/pools`
  → must print NOTHING (HARDENING §6 manifest-driven routing; the router never branches on
  provider names). Same grep over the whole `src/` (minus `src/adapters/`) must stay clean
  as it was after P3.
- Full gateway suite green; typecheck/build green.
- `git status` shows changes only under `services/subscription-gateway/` and this
  `docs/specs/subscription-fabric/p4/` folder.

## When done — write the sentinel

Write `docs/specs/subscription-fabric/p4/P4_PHASE_1_NOTES.md`:

- What was built (modules, key functions, wiring points), files changed/added.
- Test counts (before/after), build status.
- Any contract workarounds, the session-health→probe-recovery mapping you chose, the
  `normal`-priority degraded-policy decision, and anything Phase 2 must know.
- Open questions / risks.

Then commit (router+pools+tests+wiring as one commit; NOTES + this brief in a second
`docs(specs)` commit) and stop. Do not start Phase 2.
