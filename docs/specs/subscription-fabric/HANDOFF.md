# HANDOFF — Subscription Capability Fabric, 2026-09-26 cloud-gate pivot

> Written for session-continuation. REPLACES the previous HANDOFF (everything it described
> through the activation PR is DONE and merged). Repo: `Gizziio/allternit-platform`, shared
> checkout `~/Desktop/allternit-workspace/allternit` (on main). Specs:
> `docs/specs/subscription-fabric/` (reading order: SPEC.md → HARDENING.md (binding) →
> IMPLEMENTATION_PLAN.md → REVIEW_CLAUDE.md (normative)).

## 0. The owner's directive that defines this phase

**The P3 manual gate must run on an Allternit cloud computer, NOT on Eoj's desktop.**
(Eoj, 2026-09-26, after watching the Sessions window open on his desktop Chrome: "we said we
would set this up with a cloud computer within allternit not on the desktop computer.") The
spec agrees: `HARDENING.md:147-160` (D3-refined) — the boundary is "single-tenant ownership +
Allternit-provisioned containment, not physical locality"; the fabric runtime belongs on an
"Allternit Sessions" machine, **T1 Hosted** = Allternit cloud VPS provisioned via
`cmd/allternit-computer-cloud`. The README's "It must never run on cloud infrastructure" line
(`services/subscription-gateway/README.md:27`) is STALE — D3-refined superseded it; fix it in
the next PR.

## 1. State of the program (all merged + attested)

- **PR #761 (`0191825a8`) — P3 worker activation, landed 2026-09-26 ~14:20 CDT.** WorkerPool
  (per-lane runtimes, injectable Launcher, probe→health mapping, Critical #5 no-auto-retry),
  `POST /v1/accounts/:id/connect`, `startDrain` (account-driven lanes, one attempt per lane,
  auto-activate with cooldown, P4 `requeueAfterFailure` wired via `WorkerDeps.dispatch`),
  main.ts boot wiring, chatgpt-web `createAdapter()`, CLI `subs connect` = create-if-absent +
  connect. NOTES: `docs/specs/subscription-fabric/p3/P3_ACTIVATION_NOTES.md`.
- Attestations: `agent-ledger/summaries/2026-09-26-1420-subsfab-activate-kimi-code-p3-activation.md`
  (+ earlier phases tonight: #735, #738, #742, #751, #753, #754).
- **Baselines: gateway 244/244 (28 files) + NUL gate, CLI 50/50, provider-literal grep clean.**
- Local gate smoke WAS verified before the pivot: `subs connect chatgpt` reused account
  `96d4ccb2-808e-4c78-b68f-64fff059d069` (no duplicate), Sessions window opened, probe →
  `auth_required` (correct), graceful SIGTERM closed pool+browser cleanly. Gateway log:
  `worker runtime launched → probe → auth_required`. **The activation code works — only its
  placement was rejected.** Gateway is currently STOPPED; no stray processes; local state dir
  `~/.allternit/subscriptions/` (state.db + account row + keychain cli-token) intact.
- Remaining plan phases: P5 (kimi-web/claude-web + MCP), P6 (gemini-web), P7 ("Allternit
  Sessions" machine). **Eoj deprioritized P5–P7 as full phases — but the gate now requires
  the P7-lite slice below.**

## 2. The one blocker: the gateway cannot boot off macOS

`boot()` refuses to start without the local macOS Keychain (`src/security/keychain.ts`
shells `/usr/bin/security`; `requireKeychain` in main.ts exits 1). No Linux/DPAPI backend
exists. The spec anticipated exactly this (`keychain.ts:4` D15 note: "on the Windows Sessions
image a DPAPI-backed store fills this role").

**The seam is already there:** `KeychainBackend` is an interface (`available/get/set`),
`boot({ keychain })` is injectable, tests use `fakeKeychain`. The next PR is small:

1. Pluggable backend selection in config (`SUBS_GATEWAY_KEYCHAIN=file|keychain`, default
   keychain on macOS). New `FileKeychainBackend` (0600 file under stateDir; encrypt-at-rest
   is a follow-up — say so honestly in the PR) + tests (extend `test/keychain.test.ts`).
2. README/HARDENING docs alignment (drop the stale "never cloud" line, point at D3-refined).
3. Normal landing recipe (§5). Scope: `services/subscription-gateway/` only.

## 3. Running the gate on a T1 cloud computer (runbook v2)

The computers plane is real and serving (Incus guests on the VPS behind
`mail.news.allternit.com`; CLI `allternit computers …`; full details in the explore findings
— ask the repo: `docs/learnings/CLOUD_COMPUTER_MAP.md`, phase NOTES 1–5,
`docs/desktop-cloud-mvp/RUNBOOK.md`). Cost: **~30¢/hr for 4 GB Linux, metered per minute**;
create is spend-gated (org credits + monthly cap) — **Eoj must approve the spend before
creation.** Steps:

1. **Spend + direction approval from Eoj** (Option A recommended; B=remote-CDP split and
   C=local-contained were rejected/misaligned — see git history of this file if needed).
2. Land the keychain-backend PR (§2).
3. `allternit computers create --kind cloud_desktop --os linux --cpu-cores 2 --memory-mb 4096
   --disk-mb 20480 --name sessions` (needs `ALLTERNIT_API_URL` + `ALLTERNIT_TOKEN`).
   Note: shell/upload are approval-gated `403 confirmation_required` → human approves →
   retry with `--approval-id`. That is by design.
4. Install Node + pnpm + Chrome deps in the guest; upload the repo (or the built gateway
   tarball) via `allternit computers upload`; boot the gateway with
   `SUBS_GATEWAY_KEYCHAIN=file SUBS_GATEWAY_TCP=1 tsx src/main.ts`.
5. Eoj logs into ChatGPT via the **streamed display**: `POST /api/v1/computers/:id/embed-token`
   → `/embed/computers/:id?token=…` (noVNC). WARNING: writable VNC
   (`ws-token {purpose:"vnc"} read_only=false`) is unit-tested but NEVER live-smoked
   (phase-5 owed debt) — if interactive input fails, fall back to
   `computers screenshot` + `POST /:id/mouse|keyboard` one-shots (approval-gated). Eoj drives
   the login himself — never handle his credentials.
6. CLI from the Mac: the gateway's UDS is inside the guest; either run the CLI inside the
   guest via `computers shell`, or expose via the authenticated in-VM HTTP proxy
   (`POST /:id/proxy/enable {port:7788}` — Incus guests only) and point `SubsClient` at it
   (SUBS_GATEWAY_TOKEN + base URL — check `cmd/cli/src/subs/client.ts` for what's
   configurable; a small CLI patch for remote base URL may be needed — additive).
7. Gate steps unchanged: `subs connect chatgpt` → login → `subs status` ready →
   `task run chat.create --prompt "Say hello in exactly three words" --wait` → `kill -9`
   the gateway mid-task → restart → adopted/`submission_ambiguous`, never double-submits
   (verify in ChatGPT UI) → `task run image.generate --wait` → artifact + sha256 + quarantine.
   Expectation-setting stands: chatgpt-web selectors are v1-unverified against the real UI;
   selector failures are the gate working — capture them verbatim for the repair follow-up.
8. Verdict attestation in the ledger (honest pass/fail per step). Stop the cloud computer
   when done (idle auto-stop sweeper exists, but stop it explicitly; snapshots exist if the
   Sessions image should be kept).

## 4. Environment traps (all still binding)

- Run gateway + CLI via **tsx**, never plain node (extensionless ESM / JSON-import asserts).
- **NEVER `playwright install`** — CDN hard-stalls; tests use system Chrome; unit tests fake
  the Launcher.
- **Cold-Chrome test flake**: the two real-browser suites (`worker.test.ts`,
  `chatgpt-web-conformance.test.ts`) can fail their 10 s launch hook on a cold Chrome —
  reproduces on unmodified base, passes warm. Pre-existing, documented; do not "fix" by
  deleting tests.
- **NUL trap**: `grep $'\x00'` cannot detect NULs (empty-pattern lie); use
  `scripts/check-fabric-sources-clean.sh` (wired into the gateway test script).
- Worktree rules: session worktrees only, pnpm only, never touch the shared checkout's dirty
  files (other live sessions own them; `git-discipline-check.sh` FAIL on those is
  PRE-EXISTING). Commits in session worktrees may pass a ~3 min steering gate — a pane at
  `git commit` is not stalled.
- `scripts/git-discipline-check.sh` PASS output is the session-end evidence block; ledger
  attestation with `STEER_GUARD_OFF=1 git ...` in the shared checkout, `gh pr merge <n>
  --merge`, delete session branch local+remote, `rm -rf node_modules dist` before
  `git worktree remove --force`.
- If main moved under your PR ("Base branch was modified"), just retry the merge.

## 5. Landing recipe (unchanged)

Executor commits (conventional) → independent review: `pnpm install`,
`pnpm -F subscription-gateway build`, `CI=1 pnpm -F subscription-gateway test`,
`pnpm -F @allternit/cli build` + test (if CLI touched), literal grep
(`chatgpt|claude|kimi|gemini|grok|deepseek|openai|anthropic` — empty outside `adapters/`),
scope check, `file` on new sources (must say text) → push → PR with real evidence →
`gh pr merge <n> --merge` → shared checkout `git pull --ff-only` → ledger summary +
LEDGER.md line (`STEER_GUARD_OFF=1`) → push → teardown worktree.

## 6. Standing hard gates

- Provider-literal grep stays empty outside `adapters/chatgpt-web/`.
- Never promise production results from untested prompts; audit-before-quote for Tier C;
  money and client comms human-approved. **Cloud-computer creation is spend — Eoj approves
  first, with the hourly number shown.**
- Never handle Eoj's provider credentials; he drives every login window himself.
- Port discipline: gateway UDS `~/.allternit/subscriptions/gateway.sock`; TCP 7788 registered.

## 7. If the cloud gate PASSES

Resume plan order (Eoj's call): P5 kimi-web + claude-web + MCP surface; P6 gemini-web
declarative proof; full P7 Sessions machine tiers (the T1 path above IS its first slice —
generalize the runbook into `cmd/allternit-computer-cloud` templates); D6 media-router
repoint at the gateway; Rust-side picker merge (gateway catalog → `/api/v1/models`, small
fetch+concat now that `/v1/catalog` has shape parity — see P4_PHASE_2_NOTES.md).
