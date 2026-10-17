# Phones 2.0 — relay lane notes

Lane: phone stream relay (`session/phones-relay`, worktree `allternit-wt-phones-relay`).
Date: 2026-10-17. Base: `origin/main` @ de93fa9558.

## What was built

The browser-facing WebSocket plane that lets any surface view/control a linked
Android through the account's Desktop app, mirroring `computer_ws.rs` minus
VNC: scrcpy H.264 (binary) + JSON control envelopes (text) are **opaque** to
the relay.

### Files created

| File | What |
|---|---|
| `cmd/allternit-api/src/phone_ws.rs` | The whole relay: tokens, registry, relay pumps, 4 routes, tests. |
| `cmd/allternit-cloud-api/src/routes/registered_phones.rs` | cloud-api upsert/list route for `registered_phones` (device-token auth), + tests. |
| `cmd/allternit-cloud-api/migrations_pg/093_registered_phones.sql` | `registered_phones` table (identity + `last_seen_at` only). Next migration number after 092. |
| `surfaces/docs/api/phone-stream-relay.mdx` | Dev-doc page for this contract (nav: docs.json "Overview" API group). |

### Files modified

| File | Change |
|---|---|
| `cmd/allternit-api/src/lib.rs` | `pub mod phone_ws;`; `AppState.phone_link: Arc<PhoneLinkRegistry>`; `test_helpers` gained `app_state_with_phone_directory` (stub directory injection) — the private constructor now takes an optional registry. |
| `cmd/allternit-api/src/main.rs` | AppState init (`PhoneLinkRegistry::new_http()`); route mounts (below). |
| `cmd/allternit-cloud-api/src/routes/mod.rs` | `pub mod registered_phones;` |
| `cmd/allternit-cloud-api/src/lib.rs` | `.merge(routes::registered_phones::routes())` |
| `docs/dependency-map/features.json` | New `feature:phone-stream-relay` (status `building`) after `feature:bot-phones`. |
| `docs/dependency-map/runtime-links.json` | Nodes + edges for the new module and its cloud-api/runtime relationships. |
| `surfaces/docs/docs.json` | Nav entry `api/phone-stream-relay`. |

## Route table

allternit-api (dev default `http://127.0.0.1:18013`; the installed Desktop owns 8013):

| Route | Mount | Auth |
|---|---|---|
| `GET /api/v1/phones` | protected (`v1_routes` merge) | Clerk / desktop credential |
| `POST /api/v1/phones/:serial/ws-token` | protected | Clerk / desktop credential; `readOnly:false` additionally ACI confirmation-gated |
| `/ws/phone-link/session` | protected nest `/ws/phone-link` | Desktop credential (`x-allternit-desktop-access-token` + user headers → AuthUser via auth_middleware) |
| `/ws/phones/:serial/stream?token=` | **public** nest `/ws/phones` (outside auth middleware) | HMAC token is the credential (same model as the computer VNC public route); optional same-origin session must match `sub` |

cloud-api (Postgres):

| Route | Auth |
|---|---|
| `PUT /api/v1/runtime-devices/me/registered-phones` — `{phones:[{serial,model?,name?}]}` upserts rows for the device's account, touches `last_seen_at` | runtime device bearer only (account = `runtime_devices.user_id`; a user token "has no single runtime", same decision as `phone_sync`) |
| `GET /api/v1/runtime-devices/me/registered-phones` | runtime device bearer |

## Token claims

HS256, `b64url(header).b64url(claims).sig`, header `{"alg":"HS256","typ":"PT"}`,
secret = `desktop_ws_secret(state)` (`ALLTERNIT_DESKTOP_WS_SECRET`, same secret
source as the computer ws tokens). TTL 300 s. Claims exactly:

```json
{ "sub": "<account>", "serial": "<adb serial>", "purpose": "phone_stream", "read_only": true, "exp": <unix secs> }
```

Mint response: `{"token": "…", "expiresInSec": 300}` (camelCase per the frozen
contract — note the computer route answers `expires_in`, the phone contract is
`expiresInSec`). `readOnly` defaults **true**; `readOnly:false` requires the
ACI confirmation grant (`enforce_confirmation`, route key `phones.ws_token`,
`ConfirmationClass::Risky`, `?approval_id=` redemption) — the same
explicit-request pattern as a full-control VNC token.

Stream route enforcement at upgrade: signature + expiry + `serial` match +
`purpose == "phone_stream"` → 403 otherwise; 4401 close with
`{"type":"end","reason":"token_expired"}` when the token crosses expiry
mid-stream (remaining TTL computed at upgrade, timer-closed at it). All other
stream ends send their `{"type":"end","reason":…}` JSON text frame first and
close 1011: `desktop_offline`, `invalid_hello`, stalls.

## Registry behavior (decided model)

- In-memory in this api instance: `HashMap<(account, serial), Arc<PhoneLinkSession>>` + a waiter map. Persistence is only cloud PG `registered_phones` (identity + `last_seen_at`); no screen/input data is ever persisted or logged.
- `online` in `GET /api/v1/phones` = the key exists in this instance's registry.
- Session = one desktop-attached socket. First frame must be `{"type":"hello","phones":[…]}` (validated: 1–64 unique serials, ≤128 chars no whitespace/control, model/name ≤256; 10 s window). Hello phones are upserted to cloud PG (best-effort) and marked online; the same upsert re-runs at disconnect as the `last_seen_at` refresh.
- **One active stream per socket** (the design decision the task left open — documented for the Desktop lane): a socket may hello many phones (all register online) but its desktop→browser output routes to viewers of ONE serial at a time; a viewer for another of the socket's serials buffers as a waiter (attach-or-wait is one atomic registry pass, so no wake is lost). scrcpy is one device per process, so the Desktop opens one phone-link socket per streamed phone. This is what keeps opaque payloads routable without inventing in-band metadata.
- Relay-level notifications the relay itself writes on the session socket (distinct from the opaque data plane): `{"type":"viewer_attached","serial":"S"}` / `{"type":"viewer_detached","serial":"S"}` (best-effort `try_send`, dropped if the pipe is full) so the Desktop knows when to start/stop scrcpy.
- Heartbeat: relay pings every 30 s; peer silent 90 s → session ends (phones offline, viewers/waiters get `desktop_offline`).
- Reconnect race: a newer session replacing the same `(account, serial)` wins; the stale socket's unregister is id-checked and cannot remove the new session's key.
- Backpressure: bounded channels (128 both directions). Full → binary (video) frames are dropped (counted, logged sparsely); text (control) waits up to 5 s, then that side is treated as dead.
- Read-only enforcement at the pipe: a `read_only` token's browser→desktop frames are all dropped (opaque payload ⇒ the only safe enforcement). Full-control passes everything.

## Contract deviations / decisions to flag

None against the frozen browser contract (routes, JSON shapes, token claims,
4401 semantics are implemented verbatim). Judgment calls inside the open parts
of the task, all documented above: `expiresInSec` naming (per contract),
1011 for non-4401 ends, one-active-stream-per-socket, viewer_attached/
detached notifications, wait-until-token-expiry for offline desktops, ACI
gate for full-control minting (the "explicit-request pattern computer_ws
uses"), migration `093` using core `gen_random_uuid()` (PG13+; no existing
migration uses a uuid default, and `user_files` generates ids client-side —
the task's explicit DDL was followed).

## Local smoke test (dev port 18013)

```bash
# Terminal 1 — boot the api. The port default IS 18013; never set
# ALLTERNIT_API_PORT=8013 (that belongs to the installed Desktop).
cd /Users/joe/Desktop/allternit-workspace/allternit/allternit-wt-phones-relay
CARGO_TARGET_DIR=/Users/joe/Desktop/allternit-workspace/.shared-target/allternit-wt-phones-relay \
  ALLTERNIT_DESKTOP_WS_SECRET=dev-phone-secret \
  ALLTERNIT_DESKTOP_ACCESS_TOKEN=dev-desktop-token \
  ALLTERNIT_LOCAL_DEV_BYPASS=true \
  cargo run -p allternit-api

# Terminal 2
# server up
curl -s http://127.0.0.1:18013/health
# phones list with the desktop credential (unpaired runtime → empty list)
curl -s -H 'x-allternit-desktop-access-token: dev-desktop-token' \
        -H 'x-allternit-user-id: user-local' \
        http://127.0.0.1:18013/api/v1/phones
# → {"phones":[]}
# minting for an unregistered phone → 404
curl -s -X POST -H 'x-allternit-desktop-access-token: dev-desktop-token' \
        -H 'x-allternit-user-id: user-local' -H 'content-type: application/json' \
        -d '{}' http://127.0.0.1:18013/api/v1/phones/serial-1/ws-token
# → {"error":"phone not found"} (403/503 never for this case)
# public stream route: garbage token with upgrade headers → 403 from the
# token gate (without upgrade headers axum's ws extractor answers 400 —
# that only proves the route is mounted, not the gate).
curl -s -o /dev/null -w "%{http_code}\n" \
     -H "connection: upgrade" -H "upgrade: websocket" \
     -H "sec-websocket-key: dGhlIHNhbXBsZSBub25jZQ==" -H "sec-websocket-version: 13" \
     "http://127.0.0.1:18013/ws/phones/serial-1/stream?token=garbage"
# → 403
# full ws relay flow is covered by the in-crate duplex tests (below); a
# browser/Desktop e2e is the orchestrator's integration step after both
# lanes land.

## Smoke result on this machine (2026-10-17)

Booted on 18013 with the env above; `GET /api/v1/phones` and ws-token
answered `503 {"error":"phone registry is unavailable"}` — this machine's
runtime is paired, so the directory called the DEPLOYED cloud-api, which
does not serve `/api/v1/runtime-devices/me/registered-phones` until this
branch's cloud-api side ships (verified in the server log: `cloud returned
404 Not Found`). That is the intended rollout behavior; on an unpaired
runtime the same endpoints answer `{"phones":[]}` / `404 phone not found`.
The public stream route answered 400 without upgrade headers and 403 with
headers + a garbage token — the token gate works end-to-end.
```

## Test output summary

Filled in after the test run — see below.
