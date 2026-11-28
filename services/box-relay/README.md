# box-relay

A Cloudflare Worker that gives the browser computer (unkernel, `box.allternit.com`) a narrow way out to the internet. It speaks Wisp v1 over WebSocket and only reaches package registries and GitHub. The rules come from `CONTRACT-colibri-unkernel.md` section 9 (plan step 71).

**Not deployed.** No route, custom domain or workers.dev URL is bound. The route (proposed `wss://relay.allternit.com/wisp/v1`), the DNS record, any `wrangler secret put` and the first `wrangler deploy` all wait for Eoj's go-ahead. `RELAY_ENABLED` is `"false"` in `wrangler.jsonc`, so even a deployed copy answers `503 {"error":"relay_off"}`. There is no `deploy` script on purpose.

## What it does

1. `GET /wisp/v1` (or `/wisp/v1/`) with `Upgrade: websocket` and `?t=<Clerk session token>`.
   - `RELAY_ENABLED` not exactly `true`: 503 `{"error":"relay_off"}`. The box shows "Network is off for now."
   - No token or a bad one: 401. Checked once at the upgrade (RS256, Clerk JWKS, expiry with 60 s leeway, `iss` must be a platform Clerk issuer).
   - Not a WebSocket upgrade: 426. Any other path: 404.
2. The socket goes to a Durable Object (`BoxRelay`) named by the Clerk user id. The token is not forwarded.
3. Each Wisp CONNECT is checked before any TCP connection:
   - the hostname must be an exact match in `src/allowlist.ts`, and the port must be 443;
   - IP literals (IPv4 in any form, IPv6) are refused;
   - UDP streams are refused (`0x41`); anything off the list gets Wisp CLOSE `0x48` (blocked);
   - the name is resolved over DNS-over-HTTPS; if any address is private, loopback, link-local, CGNAT, documentation, multicast or in `198.18.0.0/15`, the stream gets `0x48` (rebinding guard). Otherwise the relay connects to that checked address with `cloudflare:sockets` `connect()`, so a second lookup cannot swap in another address.
4. Wisp v1 has no half-close. When the remote side finishes, the relay sends CLOSE `0x02` and closes the stream. A client CLOSE flushes the data already queued, then closes the socket.

## Caps per user (starting values, tuned at deploy time)

| Cap | Value | When hit |
|---|---|---|
| Open streams | 32 at once | CLOSE `0x49` (throttled) |
| New streams | 100 per minute | CLOSE `0x49` |
| Bytes, both directions | 512 MiB per UTC day (stored in the Durable Object) | CLOSE `0x49` on the stream, new streams refused |
| Sockets | 1 per user | a second socket closes the first with WebSocket code 4001 |
| Socket age | 2 hours | WebSocket closed with 1000 |

Logs hold counts and hostnames only: never payload, never URLs (the URL carries the token).

## Settings (`vars` in `wrangler.jsonc`, none are secrets)

| Name | Default | Meaning |
|---|---|---|
| `RELAY_ENABLED` | `"false"` | Exactly `"true"` turns the relay on |
| `CLERK_JWKS_URL` | `https://clerk.allternit.com/.well-known/jwks.json` | Where the token's signing keys come from |
| `CLERK_ISSUERS` | `https://clerk.allternit.com,https://allternit.com/__clerk` | Accepted `iss` values (same as `cmd/allternit-api/src/auth.rs`) |

Binding: `BOX_RELAY`, Durable Object class `BoxRelay` (SQLite-backed, migration `v1`).

## Before the first deploy (Eoj's gate)

- Check the Cloudflare plan limits for WebSockets, Durable Objects and outbound TCP (`connect()`), and the subrequest count per Durable Object event (each stream costs one DNS fetch, cached 30-300 s, and one `connect()`).
- Check that `connect()` reaches the listed hosts. Outbound TCP from Workers to Cloudflare's own IP ranges is restricted, and some registries (npm, crates) sit behind Cloudflare.
- Confirm the hostname (`relay.allternit.com`), then add the route, the DNS record and set `RELAY_ENABLED` to `"true"`.

## Develop

This package is not part of the pnpm workspace (see `pnpm-workspace.yaml`). It uses plain npm.

```sh
npm install
npm run typecheck
npm test        # vitest: the token gate, the allowlist, the caps, the Wisp framing
```

The user's own Wisp URL (`wss://`, mode `custom` in the box) does not go through this relay, so none of these rules apply to it.
