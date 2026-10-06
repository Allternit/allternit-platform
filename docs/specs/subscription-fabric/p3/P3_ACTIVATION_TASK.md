# P3 Activation — worker pool, connect/probe endpoint, task drain (subscription-gateway)

You are working in a git worktree of `Allternit/allternit-platform` (branch
`session/subsfab-activate`, cut from main @ 5fb680ccd). Zero prior context — this file is
your whole brief. Read the files it points at before writing code.

## Why this PR exists

The P3 manual gate (a human logs into ChatGPT through the gateway) is BLOCKED on missing
wiring: the gateway has a complete worker layer that `main.ts` never activates. Tonight:

- `allternit subs connect chatgpt` created the account row but NO Sessions window opened
  and no probe ran — `POST /v1/accounts` only writes the row.
- A task enqueued via `POST /v1/tasks` would sit in the scheduler forever — nothing drains
  lanes into `runAttempt`.

This PR is the activation seam, and nothing more. Reuse what exists; do not redesign.

## Repo rules (hard)

- **pnpm only.** Never npm/npx/yarn. Run `pnpm install` at the worktree root first.
- **Never run `playwright install`** — CDN hard-stalls on this network.
- Unit tests must NOT launch real browsers — hide launching behind an injectable factory
  and fake it in tests (the conformance suite already covers the real adapter).
- No push/PR/merge. Conventional commits. Do NOT touch the contracts/SDK packages.
- Provider-name literals stay out: `grep -rniE 'chatgpt|claude|kimi|gemini|grok|deepseek|
  openai|anthropic' services/subscription-gateway/src cmd/cli/src` must stay at its
  current state: zero matches except under `src/adapters/` / `adapters/`.
- Scope: `services/subscription-gateway/` and `cmd/cli/` only (plus this docs folder:
  `docs/specs/subscription-fabric/p3/`).

## Grounding — read these first

1. `services/subscription-gateway/src/main.ts` — boot: wires db/keychain/registry/events/
   HTTP/scheduler/router. No supervisor, no workers. Your wiring lands here.
2. `src/worker/supervisor.ts` — `WorkerSupervisor.ensureWorker(key)`: reconcile-before-
   serve, worker status map. Already exists; use it, don't replace it.
3. `src/worker/worker.ts` — `runAttempt(deps, req)`: full attempt lifecycle, event feed,
   `WorkerDeps` incl. optional `dispatch?: DispatchDeps` (P4's per-hop re-route hook —
   currently never wired by anyone) and `supervisor`/`onEvent` hooks.
4. `src/router/dispatch.ts` — P4: `resolveForNewTask` (already called by routes_tasks at
   enqueue), `requeueAfterFailure` (must run at the worker boundary when an attempt fails
   terminally — wire it via `WorkerDeps.dispatch`).
5. `src/adapters/registry.ts` — `loadAdapterRegistry` / `AdapterRegistry`: manifests +
   adapter factories (`byId` etc.).
6. `adapters/chatgpt-web/adapter.ts` + `src/adapters/chatgpt-web/` — the real adapter;
   look at its `probe()` and what runtime it needs (browser context/page via the SDK).
7. `platform/packages/subscription-adapter-sdk/src/helpers.ts` (or `test/helpers.ts` in
   the gateway) — `launchBrowser()`: system Chrome (`channel: "chrome"`), persistent
   profile dir. This is the launch mechanism the pool uses (injected in tests).
8. `src/http/routes_accounts.ts` — accounts routes today: list/get/create/disconnect.
   You add `POST /v1/accounts/:id/connect`. Follow the existing auth/scoping pattern
   (`requireScope("accounts:manage")` — connect changes account state).
9. `src/http/routes_tasks.ts` — task create: already resolves + enqueues through the
   scheduler (P4). Note how it keys the scheduler lane (`queueKeyOf`).
10. `src/queue/scheduler.ts` — the FIFO; you drain it, you don't change it.
11. `src/store/queries.ts` — `upsertAccount` etc. for health writes.
12. `src/http/server.ts` — `GatewayDeps` and how routers receive deps.
13. `cmd/cli/src/commands/subs.ts` — `connect` today: POST /v1/accounts only. Extend to
    also call your new connect endpoint.

## Deliverables

### 1. `src/worker/pool.ts` — `WorkerPool` (the factory)

Per-lane runtime manager, keyed by `(provider, account_id)` (§A8 worker ownership):

- `interface LaneKey { provider: string; account_id: string }` (or reuse WorkerKey if it
  matches — check supervisor.ts first and prefer reuse).
- `WorkerPool(deps: { db, registry, supervisor, launch?: Launcher, logger? })` where
  `Launcher` is the injectable seam:
  ```ts
  type Launcher = (lane: LaneKey, manifest: AdapterManifest, profileRef: string) => Promise<AdapterRuntime>;
  ```
  `AdapterRuntime` is whatever the adapter needs to run attempts + probe against a live
  browser: at minimum `{ adapter, probe(): Promise<ProbeResult>, close(): Promise<void> }`.
  Define it from what `runAttempt` actually consumes (read worker.ts — if runAttempt takes
  the adapter and a page/context separately, mirror that; don't invent shapes).
  Default Launcher = SDK `launchBrowser` + adapter factory from the registry.
- `async activate(lane)`: idempotent — returns the existing runtime if already active.
  Else: supervisor.ensureWorker (reconcile-first), then launch browser (headed, real
  Chrome, persistent profile under the account's `profile_ref`), run `probe()` once:
  - probe pass → `upsertAccount` health `"ready"`; runtime stays resident.
  - logged out / auth wall → health `"auth_required"`, **leave the window open** for the
    human to log in, return the runtime (probe can be re-driven by another `activate` —
    the CLI prints exactly this instruction already).
  - challenge / unusual-activity → health `"challenge_presented"`, close nothing, never
    retry automatically (Critical #5: stop + surface; emit a `notify`/SSE event if the
    existing notifier pattern makes it one call).
  - provider unreachable / UI drift → map to the existing SessionHealth values
    (`provider_down` / `ui_drift`).
- `runtimeFor(lane)`: the resident runtime or null.
- `async shutdown()`: close all runtimes (called from `RunningGateway.close()` in
  main.ts).

### 2. `POST /v1/accounts/:id/connect` (routes_accounts.ts)

Body optional `{}`. Loads the account, refuses 404/known-bad states per the existing
route style, calls `WorkerPool.activate` for its lane (pool arrives via `GatewayDeps`),
returns the updated account JSON (health reflects the probe outcome). Also update the
`connect` CLI command to call it after account creation, so
`allternit subs connect chatgpt` = create (if absent) + connect in one motion — match the
README's promised flow. Keep the CLI's existing output lines.

### 3. Task drain — `src/worker/drain.ts` (small, explicit)

After enqueue, tasks must actually run. Minimal correct design:

- `startDrain(deps: { db, scheduler, pool, makeAttemptDeps }): () => void` — subscribes to
  scheduler enqueue notifications if the scheduler exposes any (check; if it has none,
  poll on an interval `drainIntervalMs` default 250, injectable) and on wake:
  for each lane with queued tasks whose pool runtime is active-and-ready (or activatable —
  only auto-activate when the task's routing pinned provider/account or resolved lane
  matches an existing account; do NOT auto-launch browsers for lanes with no account):
  pick next per scheduler's existing fairness (interactive > normal > background if the
  scheduler already encodes it — it does per P3 notes; just use its pick API), call
  `runAttempt` with full deps including `supervisor`, `onEvent` (heartbeat → SSE via the
  existing hub wiring in worker.ts deps if present), and `dispatch` = P4's
  `requeueAfterFailure` bound with {db, registry, router, scheduler} so per-hop re-route
  works end-to-end.
- One attempt per lane at a time (§A5: no parallel submits on one account; detached watch
  pages are the only parallelism and they're already handled inside the worker layer).
- Errors from runAttempt resolve per its existing outcome types; the drain loop must not
  die on a failed attempt (log + continue) and must not double-pick a task (the worker's
  status transitions are the lock — rely on them; if the scheduler has a claim/lease API
  use it, else check task status before picking).
- Wire `startDrain` in main.ts after HTTP is up; stop it in `close()`.

### 4. main.ts wiring

Construct WorkerPool (with default Launcher) + startDrain; add pool to `GatewayDeps`
(type lives in server.ts next to the others); extend `RunningGateway.close()` to
`pool.shutdown()` + stop drain. Keep boot order: after registry load, before/around HTTP.

### 5. Tests (extend existing suites — do not regress)

- `test/pool.test.ts` — fake Launcher (no browsers): activate idempotent (one launch for
  two calls); probe pass → ready + runtime resident; auth wall → `auth_required` + no
  auto-retry (activate called once, no second probe unless re-invoked); challenge →
  `challenge_presented`; shutdown closes runtimes once each.
- `test/connect-endpoint.test.ts` (or extend the accounts route tests) — POST connect:
  200 + updated health on pass; 404 unknown account; probe outcome reflected in body;
  auth scoping matches sibling routes.
- `test/drain.test.ts` — fake pool/runAttempt: enqueued task for a ready lane → attempt
  runs with the resolved lane's adapter ctx; terminal failure → `requeueAfterFailure`
  called (dispatch wired); failed attempt doesn't kill the loop (a second task still
  runs); no account for the lane → no launch, task stays queued.
- Extend `test/boot.test.ts` if it asserts dep wiring — additive only.
- CLI: extend `cmd/cli/src/commands/subs.test.ts` — `subs connect` now POSTs create then
  connect (mock server asserts both calls; output unchanged).
- Baselines to keep green: gateway 221 tests / 25 files; CLI 48. Add your new counts on
  top. Run `pnpm -F subscription-gateway build` + `CI=1 pnpm -F subscription-gateway test`
  and `pnpm -F @allternit/cli build` + `CI=1 pnpm -F @allternit/cli test`.

## Hard gates before NOTES

- Provider-literal grep above: no new matches outside adapters.
- The NUL gate runs as part of the gateway test script — keep it green (never write NUL
  bytes; if `git diff` shows `Bin` on any file you wrote, fix it before committing).
- `git status` scope: only the three allowed trees.

## When done

Write `docs/specs/subscription-fabric/p3/P3_ACTIVATION_NOTES.md`: what was wired where,
the exact shapes you chose for Launcher/AdapterRuntime (and why), probe→health mapping
table, drain loop semantics (pick/lock/interval), test counts before/after, and anything
the live gate must know (e.g. how to watch for the window opening, where probe logs go).
Commit (feat(subscription-gateway) for the gateway work, feat(cli) for the CLI, docs for
notes+brief) and stop.
