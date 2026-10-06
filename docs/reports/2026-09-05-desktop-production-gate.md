# Allternit Desktop — Mandatory production gate

**Date:** 2026-09-05  
**Scope:** Packaged Allternit Desktop 1.1.0 (macOS arm64) + local `allternit-api` `:8013` + cloud control plane `https://api.allternit.com` + Clerk `accounts.allternit.com`.  
**Rule:** Every item below is **mandatory**. Do not ship until the Pass column is checked against a signed, notarized build that was not asar-patched by hand.

This is a test list, not a status report. Items marked **FAILING NOW** were observed in the 2026-09-05 live desktop session and must be re-tested after the dual-API / brain-picker fixes land in a real platform rebuild.

---

## How to run

1. Fresh userData (or a dedicated test profile). Do not reuse a machine that already has pairing-quota burns unless quota was reset.
2. Dedicated account: `info@allternit.com` (do **not** retry the yahoo password).
3. Launch the packaged app with `--remote-debugging-port=9222` for console capture.
4. Record: screenshot of each view, CDP console errors, and `curl` of local + cloud health.
5. Fail the gate on any red console error that is not on the acknowledged-waiver list at the bottom.

---

## A. Boot, identity, pairing

| ID | Test | Pass when | FAILING NOW |
|----|------|-----------|-------------|
| A1 | Cold launch splash → Get started is clickable | Button is not under `-webkit-app-region: drag`; click opens Clerk | |
| A2 | Email/password sign-in completes | No `{code:"signed_out"}`; pairing exchange 200 | |
| A3 | Device pairing persists across relaunch | `auth:get-session` returns userId + runtimeId; no splash loop | |
| A4 | Account switcher lists the signed-in account | Email is `info@allternit.com`, not blank/null | **FAILING NOW** (`/api/v1/me` historically crashed on null email; source-fixed, binary not restaged) |
| A5 | Sign out + sign in again | Session cleared; re-pair does not 403 daily cap on a successful first exchange | |
| A6 | Pairing quota | Failed exchanges do **not** increment the daily cap; cap message is accurate | |
| A7 | Clerk Device Trust | With Device Trust off, password login still works | |

---

## B. Dual-API (local data plane vs cloud control plane)

| ID | Test | Pass when |
|----|------|-----------|
| B1 | Kernel env | Logs show `cloud_api_url: Some("https://api.allternit.com")` and `self_hosted: Some(false)` |
| B2 | Operator `/api/*` stays on loopback | CDP: `/api/v1/providers`, `/api/onboarding/config`, `/api/v1/providers/auth/status` are `allternit-api://localhost:8013` (or `http://127.0.0.1:8013`), **not** `allternit-api://cloud` |
| B3 | Explicit cloud URLs still hit cloud | Pairing, mesh enroll, fabric/credits go to `https://api.allternit.com` with the device token |
| B4 | Device token on Clerk-only cloud routes | Must fail closed (401), never hang the shell. UI must not go blank (`kids: 0`) |
| B5 | Company config | `~/.allternit/company.json` has `gatewayUrl=http://127.0.0.1:8013`, `cloudApiUrl=https://api.allternit.com` |
| B6 | Hosting choice | Settings / Connect lets the operator pick **Local kernel**, **Allternit Cloud**, or a **remote self-hosted** runtime; switching reloads providers for that target |

---

## C. Brains / Connect (the production chat path)

| ID | Test | Pass when | FAILING NOW |
|----|------|-----------|-------------|
| C1 | Home is not stuck on "No AI connected" when a local brain exists | Banner gone; composer shows a model name | **FAILING NOW** until picker + auto-select ship |
| C2 | Connect picker → Local | Lists Ollama, oMLX, sidecar/local-brain models actually installed (`qwen3:4b`, omlx Qwen, etc.) | |
| C3 | Connect picker → CLI | Lists installed CLIs from `/api/onboarding/discover` (claude, codex, …) with Connect/Install | |
| C4 | Connect picker → Cloud accounts | API-key providers that were connected in Settings / Cloud Console appear as **Cloud runtime** with a model list | **FAILING NOW** when auth/status is sent to cloud (401) |
| C5 | Select a local model and send one chat | Stream returns tokens; no 401/502 in console | |
| C6 | Select a connected cloud/CLI account and send one chat | Same, billed/logged against that account | |
| C7 | Selection persists across relaunch | Last brain is restored | |
| C8 | Model Lab → "use in chat" | Pending selection lands in Home composer | |
| C9 | `/api/local-brain/models` and `/api/provider/ollama/models` | 200, non-empty when those runtimes are installed | |

---

## D. Shell views — open every one, no blank, no dead door

Walk with `window.dispatchEvent(new CustomEvent('allternit:open-view', { detail: { viewType } }))` **and** by clicking the rail, because rail items hide after ACI/Code mode changes.

For each view: capture body text, console errors, and HTTP ≥400 on `/api/`, `/v1/`, gizzi, clerk.

### D1. Primary rail (chat mode)

| View | viewType | Pass when |
|------|----------|-----------|
| Home / Chat | `chat` | Composer + recents; brain connected (C1) |
| Code | `code` | Code canvas / "Need a hand with the codebase?" is OK empty; gizzi `:4096` healthy or a visible offline state (not a white screen) |
| ACI | `browser` | Explorer + Mini-apps Store, not a hung webview |
| Agent \| Bot Hub | `agent-hub` | Roster loads; New bot is not a 404 |
| Projects | `project` / `projects` | List or empty state from local API |
| Artifacts Library | `library` | LibraryView, not SPA HTML |
| Model Lab | `model-lab` | Catalog + local models |
| Automation Tasks | `cowork-cron` | Task list or empty; creating a task requires a brain |
| Remote Control | `remote-control` | Window or in-shell view; tunnel start/stop does not crash |
| Customize | `customize` | Theme/rail settings persist |
| Design | `design` | Design window/view opens (desktop may spawn a second window) |
| Settings | `settings` | Account, backend, permissions, brains |
| Account | account footer | Shows signed-in email |
| Cloud Console | `cloud-console` | Resources / credits / fabric tabs; 401 is a signed-out empty state, not a crash |
| Second Brain | `brain` | Mini-app or view loads |

### D2. ACI-mode rail

| View | viewType | Pass when |
|------|----------|-----------|
| Mini-apps Store | `mini-apps-store` | Catalog; install of a pinned app does not throw |
| Office & Extensions | `browser-extensions` | Word/Excel/PowerPoint add-in status (installed/missing), not a dead button |
| Site APIs | `site-apis` | HAR/capture UI or empty |
| OpenClaw / Hermes / Oh My Pi / Vault Viewer | mini-app ids | Each either launches or shows a missing-runtime message |

### D3. Settings / Control Center tabs

Open Settings and click **every** section: Account, Brains/Connect, Backend (local vs cloud vs remote), Permissions, Updates, Voice, Office add-ins, MCP, Feature flags, Advanced.

Pass when: no section is an empty click that does nothing; every "Connect" either starts OAuth/CLI or explains the missing sidecar.

---

## E. Sidecars and unconnected services

| ID | Service | Pass when | FAILING NOW |
|----|---------|-----------|-------------|
| E1 | `allternit-api` :8013 | `/health` 200; static platform served | |
| E2 | `gizzi-code` :4096 | Health 200; Code view can start a thread | **Often down** in the live session |
| E3 | Voice sidecar :8001 | Dictation start/stop; Settings → Voice shows available | **Down** (PyInstaller/macOS) |
| E4 | Office engine | `shell:get-office-host-status` matches installed Office; open a doc | **Down** |
| E5 | Connector host | Connector list 200 or explicit "not installed" | **Down** |
| E6 | Mesh-node | Status stopped/running; no renderer crash | |
| E7 | Updater | `app:check-for-updates` against `Allternit/desktop`; invalid GH response is a visible error, not a hang | **Invalid GH response** |
| E8 | Computer-use / accessibility | Permissions banner: Fix Permissions opens System Settings; Dismiss hides | Accessibility denied is OK if banner is honest |
| E9 | Usage metering | Home does not 503-loop. Empty usage is OK until Clerk JWT is stored for cloud metering | **503 / cloud 401** |

Production must not ship with E2–E5 silently dead. Each needs either a healthy sidecar in the DMG **or** a first-run installer panel that blocks "you're ready" until the operator skips with an explicit warning.

---

## F. Cloud Console accounts (control plane)

| ID | Test | Pass when |
|----|------|-----------|
| F1 | Credits tab | Balance loads with Clerk-authed cloud call, or a clear "sign in to cloud" |
| F2 | Fabric resources | Create/list/terminate against `api.allternit.com` |
| F3 | Private fabric nodes | Enrollment token create/copy |
| F4 | Connected provider accounts | Keys/CLI accounts chosen in Cloud Console / Settings → Brains appear in the desktop Connect picker (C4). If cloud-only, desktop must copy them down via a device-token-safe route |
| F5 | Org switch | Switching org reloads credits + provider list |

---

## G. End-to-end happy paths (block ship if any fail)

1. **Local chat:** select Ollama/local-brain → send "ping" → streamed reply.
2. **Cloud/CLI chat:** select a connected Anthropic/Claude CLI (or other connected account) → send "ping" → streamed reply.
3. **Code:** open Code → run a trivial command against a folder → output in canvas.
4. **ACI:** open Explorer → navigate a URL → page renders in the webview.
5. **Bot Hub:** start a template bot session → first message.
6. **Design:** open Design → blank canvas, no white screen.
7. **Update check:** Settings → Check for updates → up-to-date or available (never JSON parse crash).
8. **Relaunch:** quit, reopen, still signed in, still on last brain.

---

## H. Console / network audit (do on every view)

Capture CDP `console` + `pageerror` + HTTP ≥400.

**Ship blockers (any of these on a happy-path view):**

- `allternit-api://cloud/api/v1/providers` 401
- `allternit-api://cloud/api/v1/providers/auth/status` 401
- `allternit-api://cloud/api/onboarding/config` 401
- Renderer hang on `app:get-platform-url` (sendSync vs handle)
- Blank shell (`kids: 0`)
- Uncaught exception / pageerror
- `/api/me` returning HTML SPA fallback when the UI expected JSON (`/api/v1/me` is the real route — calling the unversioned path is a bug)

**Acknowledged waivers (must still be listed on the ship ticket, not silent):**

- Usage metering 401/empty until cloud accepts device tokens or the desktop stores a Clerk JWT
- Accessibility denied until the operator grants it
- Unsigned Windows/Linux installers (owner-only: do not announce as production)
- Apple notarization / `DESKTOP_RELEASE_TOKEN` (owner-only)

---

## I. Release / installers (owner)

| ID | Test | Pass when |
|----|------|-----------|
| I1 | macOS DMG notarized + stapled | Gatekeeper open with no right-click bypass |
| I2 | Windows + Linux | Built in CI with native `allternit-api`; not announced until that is true |
| I3 | Distro feed `Allternit/desktop` Latest = 1.1.0 for this train | Updater finds this build |
| I4 | No secrets in the repo or baked bundle | No `sk_live_`, no test passwords, no `dev-api-token` backdoor on production cloud |

---

## Sign-off

Ship only when A–G are green on a **rebuilt** platform bundle (not an asar/env patch), E2–E5 are either running or explicitly skipped in first-run UI, and I1/I3 are done.

Tester: _____________  Build SHA: _____________  Date: _____________

---

## Appendix — 2026-09-05 live audit (asar-patched 1.1.0, not a ship build)

Walked the packaged macOS app over CDP after pointing the kernel at `https://api.allternit.com` and restaging `allternit-api`. This is evidence for the FAILING NOW column, not a pass.

### What worked

- Shell rendered (`#root` kids=4). Signed in as the paired device user.
- Local kernel `/api/v1/providers`, `/api/v1/providers/auth/status`, `/api/onboarding/config`, `/api/v1/me` returned 200 **from the renderer** (device token).
- `/api/provider/ollama/models` 200 (Qwen GGUF pulled). `/api/local-brain/models` 200 (`qwen3:4b`).
- Gizzi `:4096` came up. Connect picker opened (Select Model).
- Dual-API intercept: operator calls stayed on `localhost:8013` after the env/gateway patches.

### Dead doors / unconnected

| Surface | Finding |
|---------|---------|
| Home | **"No AI connected"** even with local models installed. Picker listed Anthropic/Claude/OpenAI as *Available providers* (Connect/Install) and **did not list Ollama / qwen3:4b**. Cause: Gizzi `auth/status` success path dropped local brains; picker UI only renders providers from auth/status. |
| Account footer | Synthetic email `user_…@users.allternit.local` after DB reset — not `info@allternit.com`. |
| `/api/v1/me/usage` | 503 `usage_metering_upstream_error` cloud 401. |
| `/health` | `{"status":"degraded","ready":{"db":true,"jwks":false,"gizzi":true}}` — JWKS not cached yet. |
| Updater | `Update check failed. The server sent an invalid response.` |
| Voice | PyInstaller `pyexpat` built for macOS 26, host OS older — sidecar exits 1. |
| Office engine | Did not become healthy; app continues without it. |
| Connector sidecar | Started from TS source; not verified healthy. |
| Restage hazard | Newer `allternit-api` panics on existing desktop SQLite: `V14__agent_modes_and_primary` checksum mismatch. Required DB reset. **Must be a ship test.** |
| Cloud 401s (pre-fix) | `allternit-api://cloud/api/v1/providers`, `.../auth/status`, `.../onboarding/config`, `.../usage/summary` — baked `VITE_ALLTERNIT_GATEWAY_URL=https://api.allternit.com`. |
| Rail walk | Clicking ACI hides Home rail items (Agent Hub, Projects, …). Must use `allternit:open-view` **and** click Home first. |

### 2026-09-05 later — Connect picker + auto-select (live)

After a platform rebuild into the packaged app:

- Home auto-selects **oMLX / Nail 35B-A3B**. Banner **No AI connected** is gone. Selection persisted in `allternit:model-selection`.
- Connect picker **Local** section is first: Ollama (Qwen GGUF), Local-mlx, oMLX (Nail + Qwen3 4B), Muse-glimmer, Maple-preview. Cloud/CLI follow. Gizzi catalog junk (tokengo, subconscious, …) is no longer the first screen.
- A full rail walk after that hung CDP: gizzi-code was at ~100% CPU and the kernel stopped answering `:8013`. Treat runaway gizzi as a ship test (E2).

### Source fixes in this session (need a real platform + API rebuild to ship)

- `operator-gateway.ts` — desktop never uses `api.allternit.com` as the operator gateway.
- Model picker unions discovered local models even if auth/status omits them.
- Model selection auto-picks + persists a local brain.
- `list_provider_auth_status` unions Ollama + sidecar after Gizzi discovery.
- `get_my_usage` returns empty usage on cloud 401/403 instead of 503 (not in the restaged binary until the incremental cargo finishes).
