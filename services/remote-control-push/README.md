# Allternit Remote Control Push Worker

Cloudflare edge Web Push delivery service for the Allternit Remote Control dashboard / PWA.

## What it does

- **Push subscriptions**: stores browser Push API subscriptions in KV, keyed by runtime, so a runtime can fan out notifications when it needs user attention.
- **Push delivery**: receives a notify request and sends a Web Push to every subscribed browser for a runtime. Payloads are encrypted to each subscription (RFC 8291 `aes128gcm`, `src/webpush.ts`); push services reject plain bodies. `pnpm test` checks the encryption against the RFC 8291 test vector.

The runtime relay (WebSocket / HTTP proxy / socket tickets) lives in `cmd/allternit-cloud-api/src/routes/runtime_relay.rs`, not in this worker.

## Routes

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| GET | `/vapid-public-key` | None | VAPID public key for browser subscription (plain text) |
| POST | `/subscribe` | Clerk Bearer | Store a browser push subscription. Body: `{ runtimeId, endpoint, keys: { p256dh, auth }, label? }` |
| POST | `/unsubscribe` | None | Remove a browser push subscription. Body: `{ runtimeId, endpoint }` |
| POST | `/notify` | Service secret or paired device token | Send a push to all subscriptions for a runtime. Body: `{ runtimeId, title?, body?, tag?, sessionId?, type?, data?, actions? }`. `data` is a flat object of strings/numbers/booleans (max 2 KB) passed to the service worker as `notification.data`; `actions` is up to two `{ action, title }` buttons. Factory approvals use `data.kind: "factory.approval"` with Approve / Open. |
| GET | `/pending` | None | Fetch the payload that triggered a background push. Query: `?endpoint=...` |
| GET | `/health` | None | Health check |

## Authentication

- **`/subscribe`** requires a valid Clerk session token in the `Authorization: Bearer <clerk_jwt>` header. Set `CLERK_JWKS_URL` to your Clerk instance's JWKS endpoint (e.g. `https://your-domain.clerk.accounts.dev/.well-known/jwks.json`).
- **`/notify`** requires `Authorization: Bearer <NOTIFY_SECRET>`. Only the Allternit cloud API (or another trusted service) should hold this secret. Runtimes should call the cloud API's notify proxy, not this worker directly.

## Local development

```bash
cd services/remote-control-push
pnpm install
pnpm run dev
```

Create a `.dev.vars` file (do not commit):

```
VAPID_JWK={"kty":"EC","crv":"P-256","x":"...","y":"...","d":"..."}
VAPID_PUBLIC_KEY=...
NOTIFY_SECRET=dev-notify-secret
CLERK_JWKS_URL=https://your-domain.clerk.accounts.dev/.well-known/jwks.json
```

## Generate VAPID keys

```bash
npx web-push generate-vapid-keys
```

Add the keys to Wrangler secrets (do not commit keys):

```bash
pnpm exec wrangler secret put VAPID_PUBLIC_KEY
pnpm exec wrangler secret put VAPID_JWK
pnpm exec wrangler secret put NOTIFY_SECRET
pnpm exec wrangler secret put CLERK_JWKS_URL
```

Or set them in the Cloudflare dashboard under Workers & Pages > allternit-remote-control-push > Settings > Variables.

## KV namespace

A KV namespace is already bound as `REMOTE_CONTROL_PUSH_KV` in `wrangler.toml`. If you need to recreate it:

```bash
pnpm exec wrangler kv:namespace create "REMOTE_CONTROL_PUSH_KV"
# paste the id/preview_id into wrangler.toml
```

Subscriptions are stored with a 90-day TTL and are garbage-collected when the push service returns `404` or `410`.

## Deploy

```bash
pnpm run deploy
```

CI deploys automatically on pushes to `main` that touch `services/remote-control-push/**`.

## Surface configuration

Set the worker URL in the surface environment:

```
VITE_REMOTE_CONTROL_PUSH_URL=https://push.fabrictransport.allternit.com
```

For backward compatibility the platform wrapper also reads `NEXT_PUBLIC_ALLTERNIT_PUSH_WORKER_URL`, but `VITE_REMOTE_CONTROL_PUSH_URL` is preferred.

If unset, push notifications are disabled and the bell toggle is hidden.
