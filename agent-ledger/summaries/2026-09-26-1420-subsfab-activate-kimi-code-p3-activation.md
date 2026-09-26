# session/subsfab-activate — P3 worker activation (kimi-code continuation)

**Merged:** PR #761, merge commit `0191825a8` (origin/main synced in shared checkout).
**Commits:** `a70a0ee2d` feat(subscription-gateway), `a204f21be` feat(cli), `7fa32b377` docs(specs).
**Scope:** `services/subscription-gateway/`, `cmd/cli/`, `docs/specs/subscription-fabric/p3/` only.

## What was done

Continuation per `docs/specs/subscription-fabric/HANDOFF.md` (2026-09-26 late session). The prior
executor (tmux `ao-subsfab-activate`, kimi --yolo) died on a provider 5-hour quota 403 with
`pool.ts` written and no commits; this session killed the pane and took over in the worktree
(HANDOFF option 2). Delivered the full activation seam from
`docs/specs/subscription-fabric/p3/P3_ACTIVATION_TASK.md`:

- **`src/worker/pool.ts`** (kept from executor, reviewed): `WorkerPool` — per-lane
  `(provider, account_id)` runtime manager; `activate()` = reconcile-first → launch once
  (idempotent) → non-spending probe → persist `session_health`. Injectable `Launcher`;
  default = headed persistent-context system Chrome + adapter `createAdapter()`.
  Probe→health: challenge > auth > ui_drift; `provider_down` on throw; `profile_locked` on
  SingletonLock. Challenge = one account-scoped `needs_user` event, never auto-retried
  (Critical #5). Not-ready runtimes re-probe on re-activate without relaunch.
- **`src/worker/drain.ts`** (new): `startDrain` — account-driven lane enumeration (never
  queue-driven; no account ⇒ no launch, task stays queued), 250 ms poll, one attempt per
  lane (§A5), ready-gate, auto-activate with 30 s failure cooldown, unref'd interval,
  P4 `requeueAfterFailure` via `WorkerDeps.dispatch`.
- **`POST /v1/accounts/:id/connect`** (routes_accounts.ts): 404/409/503/502 per sibling
  style; 200 returns the account with the probe outcome.
- **`src/main.ts`**: supervisor + pool + watch scheduler + activity + dispatch wired; drain
  after HTTP; close order stopDrain → supervisor → pool → servers → db.
- **`adapters/chatgpt-web/adapter.ts`**: zero-arg `createAdapter()` (+ default export).
- **`cmd/cli subs connect`**: create-if-absent (reuses enabled account — no duplicate rows)
  + connect in one motion; existing output lines kept.
- **NOTES:** `docs/specs/subscription-fabric/p3/P3_ACTIVATION_NOTES.md` (shapes, mapping
  table, drain semantics, reconcile wiring, operator notes for the live gate).

## Verification evidence

- `pnpm -F subscription-gateway build` clean; `CI=1 pnpm -F subscription-gateway test` →
  **244/244 across 28 files** (baseline 221 + pool 9, drain 6, connect-endpoint 7, boot +1);
  NUL gate `check-fabric-sources-clean: OK`.
- `pnpm -F @allternit/cli build` clean; `CI=1 pnpm -F @allternit/cli test` → **50/50** (48 + 2).
- Provider-literal grep (`chatgpt|claude|kimi|gemini|grok|deepseek|openai|anthropic`):
  zero matches outside `adapters/` (unchanged). All new files verified UTF-8 text via `file`.

## Incidents

- **Cold-Chrome suite flake:** the two real-browser test files (`worker.test.ts`,
  `chatgpt-web-conformance.test.ts`) failed their launch hook's 10 s default timeout on the
  first run. Reproduced on unmodified base `5fb680ccd` (stash test), and standalone
  Playwright launch+close probes healthy — pre-existing environmental flake, passes warm.
  Full suite green on rerun (28/28, 244/244). Documented in NOTES.
- **Concurrent sessions on main:** during landing, origin/main advanced 4 commits (release +
  carry-desktop-companion sessions: PR #760, version bumps). First `gh pr merge` bounced
  ("Base branch was modified"); retry merged cleanly (disjoint files). The shared checkout's
  dirty files (`.steering/checkpoint.md`, desktop resources, `pnpm-lock.yaml`, deleted
  `bunfig.toml`, etc.) belong to those live sessions — left untouched per repo rules;
  `git-discipline-check.sh` FAIL on them is PRE-EXISTING.

## Deferrals (honest)

- The P3 **manual gate itself is not yet run** at this attestation — it needs Eoj at the
  keyboard for the interactive ChatGPT login (HANDOFF §5 runbook). That is the next step and
  gets its own verdict attestation.
- `image.generate` runs inline (not detached) in v1; `chat.continue` dispatcher unwired;
  disconnect kill-switch (§A6.9) unbuilt; chatgpt-web selectors v1-unverified against the
  real UI (selector-mismatch failures at the gate are expected and are the gate working).
- The stale pre-activation gateway from the earlier session (pids holding `gateway.sock`)
  must be killed before restarting from merged main — handled in the gate step.
