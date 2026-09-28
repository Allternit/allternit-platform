# AO Fabric Node — P3 Implementation Notes

**Date:** 2026-09-10
**Branch:** `ao/fabric-node` (worktree `allternit-ao-fabric-node`)
**Spec:** `Allternit Brain/Research/specs/ao-fabric-node.md`
**Spike:** `Allternit Brain/Research/drafts/spike-p3-clerk-device-auth.md` (binding: own 3-leg Ed25519 pairing, NOT Clerk OAuth; the node never holds a Clerk token)

## Status: IMPLEMENTED, LIVE-VERIFY BLOCKED ON HUMAN APPROVAL

All code is implemented, built, unit-tested, and pushed. The live pairing
flow works end-to-end up to the human browser-approval step, which has not
yet been completed: **six consecutive pairing codes expired unapproved**
(three before this session resumed, three more on 2026-09-10:
`NNGP-4CE9`, `VNHK-DAAA`, `D32B-9CBE`, `262T-VAG6`, plus `798G-BU8F`
outstanding at time of writing). The node poller sees only HTTP 428
(pending) until each code's TTL expires, so no approval ever reached the
server. This is an honest blocker, not a defect in the node: every step
the node controls is proven below.

## What is implemented (commit `6a5777398` + `c9aa10578`, on `ao/fabric-node`)

Seven additive modules under `infrastructure/executor/ao-engine/src/ao/fabric/`:

| Module | Contents |
|--------|----------|
| `mod.rs` | Module wiring |
| `wire.rs` | serde wire envelopes + golden-fixture tests |
| `identity.rs` | Ed25519 node identity, `~/.agent-orchestrator/fabric/identity.json` (0600, atomic write) |
| `cloud.rs` | Pairing lifecycle: create → poll exchange → rotate → heartbeat |
| `relay.rs` | Line-faithful Rust port of `cmd/agent-daemon/src/index.ts` relay client |
| `shim.rs` | axum HTTP+WS loopback on 127.0.0.1:**8014** translating the Fabric `/v1/*` surface to the engine socket API |
| `cli.rs` | `ao fabric pair\|serve\|status` |

New deps in `ao-engine/Cargo.toml`: `axum`, `reqwest`, `tokio-tungstenite`,
`ed25519-dalek` (all `workspace = true`), `rand 0.8`, `futures-util 0.3`.
Dispatch: `src/cli.rs:111` — `"fabric" => crate::ao::fabric::cli::run(...)`.

Relay port decisions (faithful to agent-daemon): authenticate-first WSS,
request/chunked-response tunnel, `socket_*` tickets, allow-lists,
90 s / 5 MB caps. One documented deviation: the `socket_open` path guard
uses the request allow-list (which includes `/v1/`) because
agent-daemon's socket list omits `/v1/` but the shim's events socket lives
there.

Shim surface: `/health`, `/status`,
`/v1/remote-control/sessions` (+ `/:id`, `/messages` POST →
`pane.send_input` text+enter, `/abort` POST → ctrl+c, `/events` WS),
`/v1/permission` + `/v1/question` → empty arrays (the ao engine has no
such queues; replies → `true`), `POST /v1/session` → 501 (ao sessions
come from `ao spawn`). Sessions enumerated from
`~/.agent-orchestrator/state.json` ∩ engine `workspace.list` presence.
Engine access via `ApiClient::local()`; engine session forced to `ao` the
same way `cli::ao::ensure_ao_session` does it.

## Server-side PR

**PR #220** (`ao/runtime-type-ao-enum`, commit `c8809a0f8`): adds `"ao"` to
the `runtimeType` string match at
`cmd/allternit-cloud-api/src/routes/runtime_pairing.rs:1154-1161`.
**Not yet deployed** — the live API still rejects `runtimeType "ao"` with
`400 {"error":"BAD_REQUEST","message":"runtimeType must be desktop, vps, hosted, provisioned, or ios","code":"BAD_REQUEST"}` (verified by direct curl). Per spike decision D2 the node pairs with
`--runtime-type desktop` until #220 ships; the wire contract is identical.

## Verification evidence

- **Build:** `cargo build -p herdr` clean (zig@0.15 on PATH).
- **Unit tests:** 16/16 new fabric tests pass (serde golden fixtures).
- **`detect::manifest` parallel failures (9):** classified as the
  **known pre-existing flake class**. They are order-dependent parallel
  tests that pass completely serial: verified `59/59` with
  `cargo test -p herdr --bin ao detect::manifest -- --test-threads=1`
  (P1 session's notes document the same class; the failures exist on
  main and are untouched by this change).
- **Pre-spec smoke (spike Q5.2):** live `api.allternit.com` reachable;
  unknown-runtimeType rejection shape verified;
  `cloud_error_message` extracts the `message` field correctly.
- **Pairing create:** `ao fabric pair --runtime-type desktop --name ao-dev-mac`
  → HTTP 201, prints user code + verification URL, identity keypair
  generated at `~/.agent-orchestrator/fabric/identity.json` (0600).
  Logs: `~/.agent-orchestrator/evidence/ao-fabric-node/pair-live*.log`.
- **Exchange polling:** full TTL window polled with only 428s (correct
  pending semantics), clean expiry at TTL. A live bug was found and fixed
  here: the first exchange poll died with `error sending request` on a
  reused dead keep-alive connection after pairing create succeeded —
  fixed with `pool_max_idle_per_host(0)` in `cloud.rs` (`c9aa10578`);
  after the fix the poller ran the complete 10-minute window.
- **Live session for the hard gate:** `ao spawn hardgate-demo <repo> bash`
  → `ao-hardgate-demo` alive, transcript showing prompt
  (`~/.agent-orchestrator/evidence/ao-fabric-node/spawn.log`,
  `status-spawn.log`).

## Blockers / findings

1. **HUMAN GATE (blocking):** browser approval of a pairing code. Six
   codes expired unapproved. The deployed approve page itself is verified
   current — the lazy chunk `RuntimePairingPage-*.js` on
   ai.allternit.com contains the `/api/v1/runtime-pairings/code/:code/approve`
   call — so the failure is that the approval click never happens (or
   never reaches the server). **Exact remaining steps once a human is
   present:** (1) run `ao fabric pair --runtime-type desktop --name ao-dev-mac`,
   (2) open `https://ai.allternit.com/pair?code=<CODE>` (NOT the
   CLI-printed `platform.allternit.com` URL — see finding 2), sign in if
   redirected, click **Connect runtime**, (3) the poller saves the paired
   identity automatically.
2. **Deployment finding (root cause of repeated pairing confusion):** the
   server's `verification_url` default is built from
   `ALLTERNIT_PLATFORM_URL`, defaulting to `https://platform.allternit.com`,
   producing `https://platform.allternit.com/pair?code=…`. That SPA has
   **no `/pair` route** — the page 404s to the landing page. The working
   page is `https://ai.allternit.com/pair?code=…` (route confirmed at
   `surfaces/ai.allternit.com/src/routes.tsx:128` and verified deployed).
   Pairing codes are host-independent, so the CLI's printed URL being
   wrong does not block pairing — but it has now wasted multiple pairing
   windows. Fix (no protocol change, spike D9): set
   `ALLTERNIT_PLATFORM_URL=https://ai.allternit.com` on the deploy, or add
   a `/pair` redirect route to the platform SPA.
   **Fixed 2026-09-28 (session/pair-url-fix):** platform `public/_redirects` sends `/pair` to
   `ai.allternit.com/pair` (query kept), and gizzi rewrites the printed/opened link
   (`Pairing.approvalUrl`). Needs the platform site deployed to take effect for old CLIs.

## Hard gate status

**Not reached — blocked on finding 1.** Prepared: live `ao` session
`ao-hardgate-demo` spawned and alive; `ao fabric serve` verified to start
the shim only when paired (it correctly refuses unpaired). Remaining
steps after pairing completes: `ao fabric serve` → `curl 127.0.0.1:8014/v1/remote-control/sessions`
→ confirm relay WSS connect in serve log → PWA at
`https://fabrictransport.allternit.com/` (signed in as the approving
user) → open the ao node → pick up `ao-hardgate-demo` → screenshot +
typed-echo proof with the TUI detached.

## Deferrals (explicit, by design)

- Push-notify on pairing approval (spike D8) not implemented — polling
  only, per spec's unattended-operation scope.
- `POST /v1/session` returns 501 — ao sessions originate from `ao spawn`,
  not from the Fabric control plane.
- `/v1/permission` and `/v1/question` return empty queues — the ao engine
  has no pending-permission/question concept; auto-reply is `true`.
- runtimeType `"ao"` requires PR #220 to be deployed; until then
  `--runtime-type desktop` (documented D2 fallback).

## Reproduce

```bash
cd allternit-ao-fabric-node
PATH="/opt/homebrew/opt/zig@0.15/bin:$PATH" cargo build -p herdr
AO_BIN=$PWD/target/debug/ao
"$AO_BIN" fabric pair --runtime-type desktop --name ao-dev-mac
# approve at https://ai.allternit.com/pair?code=<CODE>
"$AO_BIN" fabric serve          # shim on 127.0.0.1:8014 + relay WSS
"$AO_BIN" fabric status         # fingerprint, pairing state, token expiry, relay state
```

## Addendum 2026-09-10 (orchestrator) — pairing completed, gate PARTIAL

The human gate cleared 2026-09-10 ~01:28 CDT: `ao fabric pair` approved in the
PWA (paired as seed@allternit.dev, runtime rt_af9d675b04b24bf88b8de119e9a2b650,
token valid to 2026-12-09). Verified live: loopback shim serves
`/v1/remote-control/sessions` with `ao-hardgate-demo` listed; "Secure runtime
relay connected"; Fabric PWA shows ao-dev-mac 3/3 online with fresh heartbeat
(screenshot: ~/.agent-orchestrator/evidence/ao-fabric-node/pwa-node-online.png).

**Hard gate: PARTIAL, blocked by two platform-side wiring bugs (NOT ao-side):**
1. PWA session list calls `GET /api/v1/sessions` same-origin → 404. The
   fabric-session service exists and returns 200 at
   `https://ai.allternit.com/api/v1/sessions` — the fabrictransport PWA's API
   base is miswired for this deploy.
2. `GET /api/v1/runtime-devices/<rt>/proxy/v1/remote-control/sessions` with a
   Clerk session token returns 401 "Invalid or expired token" even though the
   PWA's own runtime-list calls succeed — proxy auth wiring differs (unresolved,
   platform side).
Plus a design gap: ao has no caller for the platform's
`/api/v1/agent-sessions/sync` (cloud-api route exists), so pre-existing ao
sessions never reach the platform session catalog.

All ao-side P3 scope is proven end-to-end. Full detail:
~/.agent-orchestrator/evidence/ao-fabric-node/P3_GATE_PROGRESS.md
