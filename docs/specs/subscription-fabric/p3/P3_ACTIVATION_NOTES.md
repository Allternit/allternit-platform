# P3 Activation NOTES — worker pool, connect endpoint, lane drain

Scope delivered (task brief: `P3_ACTIVATION_TASK.md` in this folder): the activation seam
that unblocks the P3 manual gate. `POST /v1/accounts` used to write the row and stop — no
Sessions window, no probe, and enqueued tasks sat in the scheduler forever. This PR wires
the worker layer that already existed (`supervisor.ts`, `worker.ts`, `detach.ts`,
`progress.ts`) into the running gateway, and nothing more.

## What was wired where

| Piece | File | What it does |
|---|---|---|
| `WorkerPool` | `src/worker/pool.ts` | Per-lane runtime manager keyed `(provider, account_id)` (reuses the supervisor's `WorkerKey`). `activate()` is the single account→browser seam: reconcile-first via `supervisor.ensureWorker`, launch once (idempotent), one non-spending probe, probe outcome persisted to `accounts.session_health`. |
| Connect endpoint | `src/http/routes_accounts.ts` | `POST /v1/accounts/:id/connect` (scope `accounts:manage`, matching siblings). 404 unknown / 409 disabled / 503 no pool / 502 activation threw; 200 returns the fresh account row, `session_health` = probe outcome. |
| Lane drain | `src/worker/drain.ts` | `startDrain(deps) → stop()`. Polls the (pure, synchronous, notification-free) scheduler on `drainIntervalMs` (default 250, injectable) and serves ready lanes via `runAttempt`. |
| Boot wiring | `src/main.ts` | Constructs supervisor → pool → watch scheduler/activity tracker → `dispatch` (P4 `DispatchDeps`), passes `pool` into `GatewayDeps`, starts the drain after HTTP is up, and `close()` now stops drain → supervisor → pool (browsers) before servers/db. `RunningGateway` exposes `pool` and `supervisor`. |
| Adapter factory | `adapters/chatgpt-web/adapter.ts` | Added the zero-arg `createAdapter()` (+ default export) the pool's default launcher looks for at `<adapter dir>/adapter.ts`. |
| CLI | `cmd/cli/src/commands/subs.ts` | `subs connect <provider>` = GET accounts → reuse enabled account for the provider, else POST create (existing output line kept verbatim) → POST `/:id/connect`. Output is the connected account JSON. |

## Launcher / AdapterRuntime shapes (and why)

```ts
type LaneKey = WorkerKey; // { provider, account_id } — reuse, not a parallel key type

interface LaneRuntime {           // exactly what runAttempt() consumes, plus probe+close
  adapter: SubscriptionAdapter;
  page: PageLease;
  makeResolver: (page: PageLease) => SelectorResolver;
  probe(): Promise<ProbeResult>;
  close(): Promise<void>;
}

type Launcher = (lane: LaneKey, manifest: AdapterManifest, profileRef: string) => Promise<LaneRuntime>;
```

`RunRequest` takes `{ adapter, page, makeResolver }` separately, so the runtime mirrors that
triple instead of inventing a new shape — the drain passes them straight through. `probe()`
and `close()` ride along because activation and shutdown are pool concerns. The default
Launcher (`createPlaywrightLauncher`) = SDK-style persistent **headed system Chrome**
(`channel: "chrome"`, `launchPersistentContext` under `<stateDir>/<profile_ref>`) +
adapter instantiated from its own package (`createAdapter()` at `<dir>/adapter.ts`). Tests
inject a fake `Launcher`; no unit test launches a browser.

The default launcher's `probe()` wraps `adapter.probe()` and appends a critical `challenge`
check via the pack's `challenge` locator (§A5/Critical #5: an interstitial must surface as
`challenge_presented` even when the auth probe technically passes).

## Probe → health mapping

`healthFromProbe(result)` in `pool.ts`:

| Probe outcome | `session_health` |
|---|---|
| `ok` (all critical checks pass) | `ready` |
| critical `challenge` check failed | `challenge_presented` |
| critical `auth.state` check failed | `auth_required` |
| any other critical locator failed | `ui_drift` |
| probe threw (navigation/page failure) | `provider_down` |
| launcher threw a profile-lock error (`SingletonLock` / "already in use") | `profile_locked` (and `activate()` rethrows) |

Ordering is deliberate: challenge outranks auth (a challenge page has no logged-in probe),
auth outranks drift (a logged-out page is missing the composer too, but the actionable
state is "log in"). `auth_required` and `challenge_presented` **leave the window open** —
the human drives; re-POST connect (or re-run `subs connect`) to re-drive the probe. A
resident not-ready runtime is re-probed on `activate()` **without relaunching**. Challenge
transitions append one account-scoped `needs_user` ledger event (`task_id = account:<id>`,
SSE fan-out via the hub) — once per transition, never per probe. Nothing ever auto-retries
a challenge (Critical #5).

## Drain loop semantics

- **Lane enumeration is account-driven** (`listAccounts`, `enabled` only) — never
  queue-driven. A task pinned at a lane with no account row stays queued forever and no
  browser is ever launched for it. The unrouted lane is never served by the drain.
- **Pick/lock:** per lane per tick — `scheduler.peek`; skip if an attempt is already
  running on that lane (`running` set = the one-attempt-per-lane lock, §A5); runtime must
  exist **and** be `ready`; then `scheduler.next` (no await between peek and next — the
  pick is atomic) and a `getTask` status re-check (drops entries cancelled while queued).
  Task status transitions inside `runAttempt` are the real lock against double-picks.
- **Auto-activation:** a queued task whose lane has an account but no resident runtime
  (fresh boot, post-crash) triggers `pool.activate` — reconcile-first, then launch+probe.
  Drain-driven activations single-flight per lane and back off `activationCooldownMs`
  (default 30 s) after a throw, so a locked profile is not re-launched every 250 ms.
  Lanes whose probe lands non-ready are **not** re-probed by the drain — the human
  re-drives `activate()` via the connect endpoint.
- **Failures:** `runAttempt` outcomes (incl. P4 `requeueAfterFailure` via
  `WorkerDeps.dispatch = { db, registry, router, scheduler }`) own retry/re-route; the
  drain logs thrown errors and keeps ticking. `close()` stops the interval; in-flight
  attempts are not awaited (a graceful stop mid-attempt is the same crash path
  `kill -9` exercises — reconcile on next boot).
- The interval handle is `unref()`d so a forgotten `stop()` never hangs the process.

## Reconcile wiring (Critical #2, post-restart path)

The supervisor's `AdapterLookup`/`makeReconcileCtx` reach the pool's resident runtimes
through closures (`pool.adapterFor` / `pool.reconcileCtx`) because reconcile only runs
inside `activate()`, after both objects exist. `reconcileCtx` is read-only
(`markSubmitted` throws) and refuses to reconcile against a different account's browser —
no resident runtime for the attempt's lane is a loud error, and with no runtime the sweep
takes its safe ambiguous path (`needs_user`, never resubmits).

## Test counts

- Gateway: **221 → 244** (new: `test/pool.test.ts` 9, `test/connect-endpoint.test.ts` 7,
  `test/drain.test.ts` 6, +1 additive case in `test/boot.test.ts`;
  `test/helpers.ts` gained a `pool` passthrough in `makeDeps`). Suite note: the two
  real-browser files (`worker.test.ts`, `chatgpt-web-conformance.test.ts`) can fail their
  launch hook's 10 s default timeout on a **cold** Chrome — reproduces on unmodified base
  `5fb680ccd`, passes warm; pre-existing environmental flake, not this PR.
- CLI: **48 → 50** (new: `subs connect` create-then-connect, reuse-no-duplicate).
- Provider-literal grep: unchanged — zero matches outside `adapters/`.
- NUL gate (`scripts/check-fabric-sources-clean.sh`, part of the gateway test script): green.

## What the live gate must know

- Run the gateway with **tsx** (`pnpm exec tsx src/main.ts` from
  `services/subscription-gateway`) — plain `node dist/main.js` dies on extensionless ESM
  imports from contracts. CLI the same way: `pnpm exec tsx src/index.ts` from `cmd/cli`.
- Kill any old gateway first (`pgrep -f "tsx src/main.ts"`) — one `gateway.sock` at a time.
- The pool's default launcher loads `<adapter dir>/adapter.ts` through tsx's import hook;
  a precompiled `adapter.js` next to it would win instead (convention documented in pool.ts).
- `subs connect chatgpt` is now one motion: create-if-absent + connect. The Sessions window
  (headed Chrome, persistent profile `~/.allternit/subscriptions/profiles/<account_id>`)
  opens on connect; probe logs land on the gateway's stdout logger
  (`subscription-gateway: probe <lane> → <health>`). If `subs status` stays
  `auth_required`, the window is open waiting for the human — that is the design, not a
  hang. Re-run `subs connect chatgpt` after logging in to re-drive the probe.
- The existing account row (`96d4ccb2-…`, health `auth_required`) is reused by the CLI —
  no duplicate account is created.
- After a `kill -9`, restart and re-run `subs connect chatgpt` (or submit a task): the
  drain's auto-activation runs `ensureWorker` first, so every `sent_unconfirmed` attempt
  is reconciled (adopt / flag ambiguous) before the lane serves anything new.
- `image.generate` runs inline (not detached) in v1; `chat.continue` dispatcher unwired;
  disconnect kill-switch (§A6.9) unbuilt — documented deferrals, not regressions.
