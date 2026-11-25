# OpenTunnel relay (live links)

The live-link relay is upstream OpenTunnel (MIT) pinned at `v0.6.2` (`UPSTREAM`), plus one small patch.
We don't vendor the source: `scripts/fetch.sh` clones the pinned commit into `src/` (git-ignored) and
applies `patches/*.patch`. Plan: Apps + MCP 2026, plan v3 lane 17 (v2 step 12), decision D32.

## The patch (`patches/0001-allternit-admission-and-admin-kill.patch`)

Upstream `POST /api/tunnel` is open to anyone and has no operator delete. The patch adds:

1. **Admission.** With `ADMISSION_SECRET` set, creating a tunnel needs a credential that cloud-api mints for
   one live share link. Without it the relay answers `400 {"_tag":"InvalidRequestError","message":"Tunnel
   admission refused: <reason>"}` (400 so the OpenTunnel SDK reports the message; reasons: `credential
   required`, `malformed credential`, `bad signature`, `credential expired`, `credential lifetime too long`,
   `invalid link`). Without `ADMISSION_SECRET` the server behaves as upstream.
2. **Admin revoke by ID.** `POST /api/admin/tunnel/<tunnel id>/delete` with `Authorization: Bearer
   $ADMIN_TOKEN` deletes the tunnel and closes its bridges on every machine: `204`, or `404
   TunnelNotFoundError` if there is no live tunnel with that ID. Treat 404 as already done.
3. The `tunnel.created` analytics event gains `share_link_id`.

Files touched upstream: new `src/admission.rs` (with unit tests), `http.rs` (strip the prefix, check on
create, tunnel ID from the credential), `service.rs` (`kill` = `remove` without the tunnel token),
`admin.rs` (the delete path), `lib.rs`.

### Admission credential (cloud-api mints it; relay only verifies, no network call)

```text
ota1.<payload>.<signature>
payload   = base64url(JSON {"sub": "<share link subdomain>", "lid": "<share link id>", "exp": <unix seconds>})
signature = base64url(HMAC-SHA256(ADMISSION_SECRET, "ota1." + payload))     base64url = no padding
```

- `sub` becomes the tunnel ID, so the public host is `<subdomain>.live.<APP_DOMAIN>`, the name the
  share-link registry already minted. One credential makes at most one tunnel (a second create answers 409
  `HostnameUnavailableError`).
- Lifetime: at most 1 hour ahead (10 minutes recommended), 60 s clock skew.
- It travels as `Authorization: Bearer <credential>` on `POST /api/tunnel`, or as a path prefix: the SDK can
  only set its API base URL, so `@allternit/share-tunnel` uses `api = <relay>/admit/<credential>`. Every
  other route ignores the prefix and keeps its tunnel-token auth (the SDK's bridge and certificate refresh
  use absolute paths, so they never carry it).

### What cloud-api must add (not in this PR)

- Env: see "Relay env ↔ cloud-api env" below (built in platform #2970).
- Mint: `POST /api/v1/share-links` for `kind: "live"` returns `relay_credential` (the string above), and
  `POST /api/v1/share-links/:id/relay-credential` → `{ credential, expires_at }` for an active live link
  (410 when revoked or killed, 409 when paused).
- Kill, revoke and pause: `POST /api/v1/admin/share-links/:id/kill`, owner `DELETE /api/v1/share-links/:id` and `POST /api/v1/share-links/:id/pause`, for
  a live link, call `POST {ALLTERNIT_RELAY_ADMIN_URL}/api/admin/tunnel/{subdomain}/delete` with
  `Authorization: Bearer {ALLTERNIT_RELAY_ADMIN_TOKEN}`; 204 and 404 both mean the tunnel is gone. The host's next
  heartbeat gets 410 (revoke, kill) or `status: "paused"` (pause) and stops too.

## Relay env (in addition to the upstream ones, see lane 28's systemd env)

| Var | Meaning |
| --- | --- |
| `ADMISSION_SECRET` | secret, 32+ random bytes; turns admission on |
| `ADMIN_TOKEN` | secret; enables `/api/admin/*`, including the tunnel delete (bearer is checked on the admin listener too) |
| `ADMIN_LISTEN` | where `/api/admin/*` is served, e.g. `100.83.199.24:18080` (Tailscale) or `127.0.0.1:18080`. Admin is 404 on the public TLS/HTTP listeners; unset or `off` = no admin at all. Must differ from `HTTP_LISTEN` |
| `ANALYTICS_URL`, `ANALYTICS_TOKEN` | cloud-api `POST /api/v1/internal/share-links/usage` and its secret (bytes meter, D8) |

## Relay env ↔ cloud-api env

Keychain/env only, never in code. "Same value" rows must hold byte-identical strings on both sides.

| Relay (OpenTunnel) | cloud-api | Rule |
| --- | --- | --- |
| `ADMISSION_SECRET` | `ALLTERNIT_RELAY_ADMISSION_SECRET` | Same value. cloud-api signs credentials, the relay verifies. cloud-api unset → live links 503 `relay_not_configured`; relay unset → admission off. |
| `ADMIN_TOKEN` | `ALLTERNIT_RELAY_ADMIN_TOKEN` | Same value. cloud-api sends it as `Bearer` on the tunnel delete. Unset in cloud-api → no delete; the heartbeat still stops the host. |
| `ADMIN_LISTEN` (admin listener, not public) | `ALLTERNIT_RELAY_ADMIN_URL` | Not a secret: the admin listener origin, e.g. `http://100.83.199.24:18080`; cloud-api appends `/api/admin/tunnel/<subdomain>/delete`. Never the public origin (it answers 404). |
| `ANALYTICS_TOKEN` | `ALLTERNIT_SHARE_USAGE_SECRET` | Same value, 32+ chars. The relay sends `Bearer`; cloud-api compares in constant time (401 wrong, 503 unset). |
| `ANALYTICS_URL` | (cloud-api's public origin) | Not a secret: `https://<cloud-api>/api/v1/internal/share-links/usage`. |

## Analytics the meter reads (upstream, unchanged)

`analytics.rs` posts a JSON array per batch (up to 200 events, every 2 s, `Authorization: Bearer
$ANALYTICS_TOKEN`, never retried) of `{ "source": "opentunnel", "type", "timestamp", "payload": {
"schema_version": 1, "region"?, "tunnel_id", ... } }`. Bytes arrive only on `connection.closed`
(`bytes_in`, `bytes_out`, `duration_ms`, `outcome`), one per visitor connection (`relay.rs`).

## Local relay for tests

`scripts/local-relay.sh` (docker MySQL 8, `cargo build`, a loopback server with `ISSUER=local`) prints the
`OPENTUNNEL_TEST_*` env that `platform/packages/share-tunnel`'s integration test reads;
`scripts/local-relay.sh stop` cleans up. Train only.

## Updating upstream

Change `UPSTREAM` to the new tag and commit, run `scripts/fetch.sh /tmp/ot`, fix any patch that no longer
applies, and regenerate it with `git -C /tmp/ot diff > patches/0001-...patch`. Keep the patch small.
