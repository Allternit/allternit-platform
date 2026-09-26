# HANDOFF — Subscription Capability Fabric, 2026-09-26 late session

> Written for session-continuation. This REPLACES the previous HANDOFF.md (everything it
> described through P3 merge is DONE). Repo: `Gizziio/allternit-platform`, shared checkout
> `~/Desktop/allternit-workspace/allternit` (on main). Specs: `docs/specs/subscription-fabric/`
> (reading order: SPEC.md → HARDENING.md (binding) → IMPLEMENTATION_PLAN.md → REVIEW_CLAUDE.md (normative)).

## 1. Where the program is

Merged to `origin/main` tonight (all attested in `agent-ledger/`, all verified):
- Docs+P0 contracts (#735), P1 gateway (#738), P2 adapter SDK (#742), P3 worker+chatgpt-web+CLI (#751),
  **P4 real router+quota pools+observability+subs model catalog (#753)**, **NUL hygiene + gate (#754)**.
- `origin/main` @ `5fb680ccd` (plus whatever the parallel gizzi-tui-parity session lands).
- Test baselines: subscription-gateway **221/221**, adapter-sdk 68/68, cli **48/48**, contracts 12-variant
  AdapterEvent. Provider-name-literal grep must stay empty outside `adapters/chatgpt-web/`.

**Remaining plan phases: P5 (kimi-web/claude-web + MCP surface), P6 (gemini-web declarative proof),
P7 ("Allternit Sessions" machine). Eoj explicitly deprioritized these — the P3 manual gate is the mission.**

## 2. The mission: P3 manual gate (human-driven by design)

Goal: Eoj logs into ChatGPT through the gateway; a real task runs; crash-reconcile proven. The script
(from `services/subscription-gateway/adapters/chatgpt-web/README.md`):

```bash
cd ~/Desktop/allternit-workspace/allternit && git pull --ff-only && pnpm install
# gateway must be running (see §4 for the working invocation)
allternit subs connect chatgpt    # Sessions window opens → Eoj logs in by hand → probe READY
allternit subs status             # expect ready
allternit task run chat.create --prompt "Say hello" --wait     # SSE stream
kill -9 <gateway pid>             # mid-stream or restart after; reconcile adopts or flags
                                  # submission_ambiguous — never double-submits (verify in ChatGPT UI)
allternit task run image.generate --prompt "a small red circle on white" --wait
                                  # artifact row + sha256 + quarantine xattr
```

**Gate state right now (snapshot at handoff):**
- Gateway HAS run on this machine once: `~/.allternit/subscriptions/` exists (state.db + WAL, gateway.sock
  mode 0600), cli-token issued in macOS Keychain (`security find-generic-password -s
  com.allternit.subscription-gateway -a cli-token -w`). Account row exists: `96d4ccb2-808e-4c78-b68f-64fff059d069`,
  health `auth_required`. That state is reusable — do NOT delete the state dir.
- **Gate blocker (being fixed):** `POST /v1/accounts` only wrote the row; `main.ts` never activated workers,
  so no Sessions window ever opened. The activation PR (below) is the fix.

## 3. The activation PR — in flight, your first decision

Worktree: `~/Desktop/allternit-workspace/allternit-session-subsfab-activate`, branch
`session/subsfab-activate` (cut from `5fb680ccd`, pushed? NO — not pushed yet, no commits yet).
Task brief (complete, self-contained): `docs/specs/subscription-fabric/p3/P3_ACTIVATION_TASK.md` in that
worktree. Scope: `src/worker/pool.ts` (WorkerPool + injectable Launcher, probe→health mapping, Critical #5
no-auto-retry), `POST /v1/accounts/:id/connect`, `src/worker/drain.ts` (lane drain → runAttempt with P4
`requeueAfterFailure` wired via `WorkerDeps.dispatch`), `main.ts` wiring, CLI `subs connect` = create+connect,
tests (pool/connect-endpoint/drain/boot/CLI), NOTES sentinel `P3_ACTIVATION_NOTES.md`.

Executor: **alive in tmux session `ao-subsfab-activate`** (regular kimi `--yolo` via
`zsh -ic` wrapper — OPENROUTER keys live only in ~/.zshrc). At handoff: `pool.ts` written, 8-item todo on
item 1, ~54% context, no commits. It is SLOWER than tonight's earlier executors but progressing.

**Your options, in order of speed:**
1. `tmux attach -t ao-subsfab-activate` and watch. If it's still moving, let it finish; then do the normal
   independent review (build + `CI=1 pnpm -F subscription-gateway test` + literal grep + NUL gate) →
   commit → push → PR → merge `--merge` → attest → teardown (recipe in §6).
2. If it stalls (pane dead or >20 min no motion): kill the pane, take over in the worktree yourself. The
   brief is precise; pool.ts is a starting reference. Land it the same way.
3. Fastest (Eoj-approved path if he says so): skip review depth for this one PR — functional check only,
   merge, restart gateway, run the gate. The repo rules allow Eoj to waive; say he did.

## 4. Environment facts that will bite you if ignored (all learned the hard way tonight)

- **Run the gateway with tsx, not node**: plain `node dist/main.js` dies on extensionless ESM imports
  from the contracts package (`ERR_MODULE_NOT_FOUND`). Working invocation:
  `cd services/subscription-gateway && pnpm exec tsx src/main.ts` (or `pnpm -F subscription-gateway dev`
  = tsx watch). 
- **CLI the same way**: `node cmd/cli/bin/allternit.js` fails on Node 26 (`assert` JSON-import syntax).
  Use `cd cmd/cli && pnpm exec tsx src/index.ts <cmd>` (e.g. `... src/index.ts subs status`).
- **A stale gateway may still be running from THIS session** (started ~11:43 via tsx, pre-activation code,
  holds the UDS socket). Find it: `pgrep -f "tsx src/main.ts"`. Kill before starting a new gateway —
  two gateways cannot share `gateway.sock`. (It was background task `bash-gntqc9t0` in the old session;
  if that session is dead, use pkill.)
- **NEVER run `playwright install`** — the chromium CDN hard-stalls on this network. Tests use system
  Chrome (`channel: "chrome"`, installed) via the SDK helper; unit tests fake the launcher entirely.
- **Worktree git commits pass a steering commit-gate that takes ~3 min each** — a pane sitting at
  `git commit` is NOT stalled. Approving permission prompts: `tmux send-keys -t ao-<slug> "2"` (session-approve).
- **NUL-byte trap**: `grep $'\x00'` CANNOT detect NULs (bash expands it to an empty pattern = matches
  everything). Real check: `perl -ne 'if (/\x00/) { ... }'` or watch for `Bin` in git diff / `file` saying
  "data". `scripts/check-fabric-sources-clean.sh` (wired into the gateway test script) is the durable gate.
- **Repo rules**: session worktrees only (never edit the shared checkout's source; other live sessions own
  its dirty files — `git-discipline-check.sh` FAIL on those is PRE-EXISTING, do not "fix" it). pnpm only.
  Merge = merge commit (`gh pr merge <n> --merge`), attest after (`STEER_GUARD_OFF=1 git ...` in the shared
  checkout), delete session branch local+remote, `rm -rf node_modules dist` in the worktree before
  `git worktree remove --force`. CARGO_TARGET_DIR shared-target rule if you touch Rust (you shouldn't).
- **The ao-* recipe**: `ao-spawn <slug> <worktree> "zsh -ic 'exec kimi --yolo'"` → trust prompt:
  `tmux send-keys -t ao-<slug> Enter` → one `ao-send` line pointing at the task file → `ao-watch <slug>
  <notes-path> 3600 20` (background, disable_timeout) → review independently, never trust NOTES.

## 5. After the activation PR merges — gate runbook

1. `cd ~/Desktop/allternit-workspace/allternit && git pull --ff-only`, kill any old gateway (§4), restart
   from merged main via tsx (§4). Verify: socket exists, keychain token readable, `subs status` answers.
2. `pnpm exec tsx src/index.ts subs connect chatgpt` (from cmd/cli). **Eoj logs in by hand in the window.**
   `subs status` → health `ready`. (If it stays `auth_required`, the window is open waiting — that is by design.)
3. `task run chat.create --prompt "Say hello in exactly three words" --wait` — expect SSE stream + completed.
4. `kill -9` the gateway mid-task (or rerun a second task immediately after kill), restart, check the task
   lands `adopted` (thread recovered) or `submission_ambiguous` (flagged, NOT resubmitted) — verify no
   duplicate message in the ChatGPT UI. This is the Critical #2 proof.
5. `task run image.generate --prompt "..." --wait` — artifact row with sha256 in
   `~/.allternit/subscriptions/artifacts/<sha2>/<sha>`, quarantine xattr set (`com.apple.quarantine`).
6. Gate verdict → ledger attestation (honest: pass/fail per step, selector failures listed verbatim).

**Expectation setting (from P3_PHASE_2_NOTES):** chatgpt-web selectors are v1-unverified against the real
UI. Probe/task failures on selector mismatch are a LIKELY outcome — that is the gate working: capture the
exact failure (adapter events / gateway log), and the gate verdict names the selectors needing repair
(small follow-up PR). `image.generate` runs inline (not detached) in v1; `chat.continue` dispatcher unwired;
disconnect kill-switch (§A6.9) unbuilt — all documented deferrals, not gate blockers.

## 6. Landing recipe (same as tonight's 7 PRs)

Executor commits (conventional) → your review: `pnpm install` in worktree, `pnpm -F subscription-gateway
build`, `CI=1 pnpm -F subscription-gateway test`, `pnpm -F @allternit/cli build`, `CI=1 pnpm -F @allternit/cli
test`, literal grep, scope check, `file` on every new source file (must say text, not data) → push →
`gh pr create` (real summary + evidence) → `gh pr merge <n> --merge` → shared checkout `git pull --ff-only`
→ write `agent-ledger/summaries/YYYY-MM-DD-HHMM-<slug>-...md` + LEDGER.md line, commit
`docs(ledger): attestation for session/<slug>` with `STEER_GUARD_OFF=1`, push → `ao-kill <slug>` →
`rm -rf node_modules dist` in worktree → `git worktree remove --force` → branch -D local + `git push origin
--delete`. `scripts/git-discipline-check.sh` FAILs on the shared checkout's PRE-EXISTING dirty files from
other live sessions — document as pre-existing, do not touch those files.

## 7. Standing hard gates for fabric packages

- Provider-literal grep `chatgpt|claude|kimi|gemini|grok|deepseek|openai|anthropic` — empty in
  contracts/SDK/gateway-core/CLI src+test; exception: `adapters/chatgpt-web/` only.
- Never promise production results from untested prompts; audit-before-quote for Tier C; money and client
  comms human-approved (irrelevant here but standing).
- Port discipline: gateway UDS `~/.allternit/subscriptions/gateway.sock`; TCP 127.0.0.1:7788 registered.

## 8. If the gate PASSES

Next session candidates (Eoj's call, in plan order): P5 kimi-web (chat + presentation→pptx, the D11/D12
detached-progress proof) + claude-web + MCP surface; P6 gemini-web declarative proof; P7 the "Allternit
Sessions" machine tiers; D6 media-router repoint at the gateway (waits on gate stability). The Rust-side
picker merge (gateway catalog → `/api/v1/models` in cmd/allternit-api) is a small fetch+concat job now that
`/v1/catalog` has shape parity (see P4_PHASE_2_NOTES.md).
