# Bot Mode — prove it works on all four surfaces (live backend)

**Owner decision (2026-09-10):** prove-it-works sweep, against LIVE `api.allternit.com`, full depth (chat + groups + routines/messaging + computer viewport).

**Repo:** `~/altw/allternit` (symlink → `~/Desktop/allternit-workspace/allternit`). All repo conventions per root `AGENTS.md` (session worktree ritual, steering checkpoints, ledger attestation).

## Topology facts that shape the plan (verified by recon)

- **Cloud API is control-plane only.** `api.allternit.com` relays `/api/v1/agent-sessions*` and `/api/v1/native-sessions*` to the user's registered data-plane node. It has **no** `/api/v1/computers` or `/bots/*/desktop` routes — bot computers always provision on the local data-plane `allternit-api` (:8013, currently running, PID 4012) and require a VM driver (`INCUS_URL` or `TART_HOST_URL`) on that process.
- **Web/PWA are one codebase** (`surfaces/ai.allternit.com`). PWA = same Vite app + hand-rolled `public/sw.js`; zero bot-specific PWA handling exists, so "PWA works" = same code path verified under `display-mode: standalone`.
- **Desktop** (`surfaces/allternit-desktop`) is a pure shell; bot mode = bundled platform web app + sidecars. Agent-sessions flag is always-on in desktop shell → desktop bot chat already routes via live cloud-api relay. Desktop Clerk sign-in is interactive (auth window) — a human sign-in is required once per profile.
- **gizzi-code** CLI bot mode (B1–B5, PR #121) is a local-machine feature; its test suites currently fail in this checkout only because `packages/sdk/dist` is unbuilt.
- Auth plumbing: `localStorage['allternit_token']` (Clerk JWT) → `Authorization: Bearer` on both cloud-api and local :8013. Vite dev supports **seed auto-login** (`VITE_CLERK_SEED_EMAIL/PASSWORD`, dev-only, `platform-auth-client.tsx:564-602`).
- Root `bot-e2e-*.cjs` harnesses: `scripts/e2e/bot/bot-e2e-verification.cjs` (headless, richest — fetches echo-bot ids live from `/api/v1/agents`), `scripts/e2e/bot/bot-e2e-web.cjs` (headed), `scripts/e2e/bot/bot-e2e-desktop.cjs` (Electron; `ALLTERNIT_PLATFORM_URL` overridable, default 3014 but dev UI is **3013**). No auth step in any of them — Clerk-disable/bypass was assumed. `scripts/e2e/bot/bot-e2e-playwright.cjs` is stale (dead worktree path) — ignore.

## Inputs needed from Eoj (execution-time)

1. **Clerk test-account password** (seed email is documented in `TESTING.md`; password rotated 2026-09-03 and lives in the password manager — I will not dig for it). Alternative: manual sign-in in the headed browser.
2. One **interactive Clerk sign-in** in the desktop auth window when we reach the desktop phase.

## Phase 0 — session setup (repo ritual)

- Create linked worktree `allternit-session-<id>` on branch `session/<id>` from latest `main`; `cd` into it. Do NOT touch the shared checkout (it has uncommitted changes).
- Write plan file there (`.steering/checkpoint.md` updates at every milestone).

## Phase 1 — gizzi-code CLI (local verification)

1. Build SDK dist: run `cmd/gizzi-code`'s `script/ensure-sdk-dist.sh` (root cause of the 23 test failures).
2. Run bot suites: `bun test --timeout 30000 --preload ./test/preload.ts test/runtime/bots/ test/cli/bot.test.ts test/cli/bots-pane.test.ts` → expect ~168/0. Investigate/fix any genuine failure.
3. Live CLI smoke against this machine's real `~/.gizzi`:
   - `gizzi bot create e2e-smoke --title "E2E Smoke"`, `gizzi bot list`, `gizzi bot show e2e-smoke`
   - `gizzi bot chat e2e-smoke "Reply with exactly: BOTMODE-OK"` → expect a real reply turn (headless print-mode)
   - `gizzi bot routine add e2e-smoke --schedule 'every 5 minutes' --prompt '…'`, `gizzi bot routine list`, remove it
   - `gizzi bot delete e2e-smoke`
4. Optional spec-parity micro-fixes if trivial and safe: missing `-p <name>` alias for `bot chat` (spec D6), stale strings (`bot.ts:197`, `capability-epoch.ts:33`). Only if nothing else is burning.

## Phase 2 — web app against LIVE cloud API

1. Edit `surfaces/ai.allternit.com/.env.local` (gitignored, local only):
   - REMOVE both `*_PLATFORM_DISABLE_CLERK=1` lines
   - Set `NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY=pk_live_…` (copy from committed `.env.production`)
   - Set `VITE_CLOUD_API_URL` + `NEXT_PUBLIC_ALLTERNIT_CLOUD_API_URL=https://api.allternit.com`
   - Set `NEXT_PUBLIC_ALLTERNIT_AGENT_SESSIONS_API=1`
   - KEEP `VITE_ALLTERNIT_GATEWAY_URL=http://127.0.0.1:8013` (computer path uses it)
   - Add `VITE_CLERK_SEED_EMAIL` + `VITE_CLERK_SEED_PASSWORD` (needs Eoj's password)
2. Restart vite dev (`pnpm dev`, port 3013). Verify: seeded sign-in succeeds, `/api/v1/agent-sessions` responds 200 (not 401) via live relay.
3. Point `scripts/e2e/bot/bot-e2e-verification.cjs` at `http://localhost:3013` (BASE_URL) and run: Bot Hub → 1:1 chat (identity reply) → group chat (2 bots reply in-character) → recents-purity + rail layout checks. Collect RESULT JSON + screenshots.
4. Fix anything that fails in the session worktree; re-run until green.

## Phase 3 — PWA standalone

- Same running dev app; Playwright Chromium with `page.emulateMedia({ features: [{ name: 'display-mode', value: 'standalone' }] })` (plus `manifest.json`/`sw.js` registration assertions): bot Hub → 1:1 bot chat → reply. Confirms no install-mode breakage. (True offline bot use is out of scope by design — every bot path calls `/api/`, which the SW never caches.)

## Phase 4 — desktop (packaged app, live control plane)

1. Use the existing packaged preview in `allternit-desktop-preview` worktree (`Allternit-Desktop-1.1.1-b1765-arm64.dmg` — newest verified build) OR rebuild via `scripts/build-desktop.sh` if Phase-2/3 fixes landed (commandment #8 rebuild).
2. Launch the app; **Eoj signs in once** via the Clerk auth window (interactive, headed).
3. `ALLTERNIT_PLATFORM_URL=http://localhost:3013 node bot-e2e-desktop.cjs` (desktop shell loads the dev UI with live cloud-api env). Verify: Bot Hub → bot chat streams real `ses_*` session via live relay → group chat → recents purity.

## Phase 5 — bot computer (live provisioning on local data plane)

1. Check VM driver on the running :8013 process: `ps eww 4012 | grep -E 'INCUS_URL|TART_HOST_URL'` (+ `which tart`).
2. With a valid Clerk bearer token (from the signed-in browser), provision a bot computer: `POST /api/v1/bots/<botId>/desktop/provision` → poll status → confirm `ws_url` + screenshot endpoint respond → VNC ws handshake (single-owner claim via `bot-computer-vnc.ts` rules).
3. If no VM driver is configured: configure the Tart driver (tart-host infra exists in-repo, PR #255 added per-VM guest-VNC forwarding) if tart is installed, else report as the one environmental blocker with exact config needed.

## Phase 6 — land + attest (only if code changed)

- If any fixes landed: PR from `session/<id>` with verification evidence, `gh pr merge --merge`, sync main, dated `agent-ledger` summary + LEDGER.md entry.
- If release path touched: `node scripts/release-preflight.mjs` must be 26/0; rebuild desktop bundle; verify change present in bundle; never repoint `desktop-v1.1.1`.
- Worktree/branch cleanup per ritual.

## Deliverable

A verification matrix — four surfaces × bot-mode flows with pass/fail + evidence (screenshots, RESULT JSONs, test counts, API responses) — plus landed fixes for anything that failed, or an explicit blocker report for anything environmental (VM driver, creds).

## Risks / notes

- Live bot chats hit real model providers (small quota cost, accepted).
- `.env.local` changes are local-only and gitignored — no secret lands in git.
- If the Clerk seed password isn't available, Phase 2/3 fall back to manual headed sign-in; Phase 4 always needs one manual sign-in regardless.
- The shared checkout's uncommitted changes and other sessions' worktrees are left untouched throughout.
