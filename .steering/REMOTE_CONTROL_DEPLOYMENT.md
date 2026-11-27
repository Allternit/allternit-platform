# Allternit Remote Control — Real-World Deployment Guide

This document describes how to take the hybrid Remote Control feature from a local dev setup to production on Cloudflare with real domains.

## Architecture

- **`remotecontrol.allternit.com`** — Cloudflare Pages site serving the standalone Remote Control dashboard (PWA).
- **`push.remotecontrol.allternit.com`** — Cloudflare Worker that stores push subscriptions in KV and sends Web Push notifications.
- **`ai.allternit.com/remote-control`** — Platform hub route inside the existing Allternit SPA.
- **Desktop app** — Calls `window.allternit.shell.openRemoteControl()` when available; otherwise opens `remotecontrol.allternit.com` in a browser tab.

## Prerequisites

- Cloudflare account with access to `allternit.com`.
- `allternit.com` DNS managed by Cloudflare.
- `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` stored as GitHub secrets.
- `wrangler` CLI authenticated locally (`npx wrangler login`).

## 1. Generate VAPID keys

Web Push requires a VAPID key pair. Generate it once and store the private key as a JWK.

```bash
cd services/remote-control-push
npx web-push generate-vapid-keys
```

Convert the output to JWK format. The worker expects `VAPID_JWK` to contain `kty`, `crv`, `x`, `y`, `d`.

Example JWK:

```json
{"kty":"EC","crv":"P-256","x":"...","y":"...","d":"..."}
```

`VAPID_PUBLIC_KEY` is the base64 string printed by `web-push generate-vapid-keys`.

## 2. Create the Cloudflare Pages project

```bash
npx wrangler pages project create allternit-remote-control --production-branch=main
```

Then add the custom domain:

```bash
npx wrangler pages domain add allternit-remote-control remotecontrol.allternit.com
```

Or configure it in the Cloudflare dashboard under **Pages > allternit-remote-control > Custom domains**.

## 3. Create the KV namespace

```bash
cd services/remote-control-push
npx wrangler kv:namespace create "REMOTE_CONTROL_PUSH_KV"
```

Copy the returned namespace ID into `wrangler.toml`:

```toml
[[kv_namespaces]]
binding = "REMOTE_CONTROL_PUSH_KV"
id = "<your-kv-namespace-id>"
```

Commit the updated `wrangler.toml`.

## 4. Configure push worker secrets

Set the VAPID credentials as encrypted secrets:

```bash
cd services/remote-control-push
npx wrangler secret put VAPID_JWK
# paste the JWK JSON on a single line

npx wrangler secret put VAPID_PUBLIC_KEY
# paste the public key string
```

Verify with:

```bash
npx wrangler secret list
```

## 5. Deploy the push worker

```bash
cd services/remote-control-push
npx wrangler deploy
```

This creates the Worker and the custom domain `push.remotecontrol.allternit.com` (configured in `wrangler.toml` via `[[routes]]`).

Verify the deployment:

```bash
curl https://push.remotecontrol.allternit.com/health
curl https://push.remotecontrol.allternit.com/vapid-public-key
```

## 6. Configure DNS

Cloudflare normally adds the required DNS records automatically when you attach a custom domain to Pages or a Worker route. Confirm in the Cloudflare dashboard:

- `remotecontrol.allternit.com` CNAME/AAAA record points to Pages.
- `push.remotecontrol.allternit.com` CNAME/AAAA record points to the Worker.

If they are missing, add them manually:

| Type | Name | Target |
|------|------|--------|
| CNAME | `remotecontrol` | `<pages-subdomain>.pages.dev` |
| CNAME | `push` | `<worker-subdomain>.workers.dev` |

## 7. Configure the dashboard build

The GitHub Actions workflow `.github/workflows/deploy-remote-control-cloudflare.yml` builds the dashboard with these environment variables:

```yaml
VITE_ALLTERNIT_API_URL: 'https://ai.allternit.com'
VITE_REMOTE_CONTROL_PUSH_URL: 'https://push.fabrictransport.allternit.com'
```

If the API origin changes, update the workflow.

## 8. Verify the PWA

1. Open `https://remotecontrol.allternit.com` in Chrome/Safari on a mobile device.
2. Confirm the browser offers **Add to Home Screen**.
3. Confirm the installed app opens in standalone display mode.
4. Confirm push notification subscription works (requires HTTPS and a real subscription endpoint).

## 9. Verify the platform hub

1. Open `https://ai.allternit.com/remote-control` while signed in.
2. Confirm the hub loads and lists paired runtimes.
3. Click **Open Dashboard** and confirm it opens `remotecontrol.allternit.com`.

## 10. Local development verification

The push worker can be run locally:

```bash
cd services/remote-control-push
# create .dev.vars with VAPID_JWK and VAPID_PUBLIC_KEY
npx wrangler dev --local --port 8787
```

Test endpoints:

```bash
curl http://localhost:8787/health
curl http://localhost:8787/vapid-public-key
curl -X POST http://localhost:8787/subscribe -H 'Content-Type: application/json' -d '{"runtimeId":"rt-1","endpoint":"https://fcm.googleapis.com/fcm/send/test","keys":{"p256dh":"...","auth":"..."}}'
curl -X POST http://localhost:8787/notify -H 'Content-Type: application/json' -d '{"runtimeId":"rt-1","title":"Test","body":"Hello"}'
```

## Troubleshooting

- **Blank hub page in dev** — Make sure Vite's HTML transform pipeline runs for `/remote-control`. The `remoteControlRoutePlugin` in `vite.config.ts` rewrites the request to `/index.html` so the React refresh preamble is injected.
- **Light mode on standalone dashboard** — The standalone entry uses a separate `RemoteControlThemeStore` that defaults to `dark`. If it appears light, clear site data for `remotecontrol.allternit.com`.
- **Push subscription fails** — Check that the dashboard origin is allowed by the Worker's CORS policy (`allowedOrigins` in `services/remote-control-push/src/index.ts`).
- **Service worker cannot fetch `/pending`** — If the dashboard and push worker are on different origins, update `remote-control-service-worker.js` to call the full push worker URL instead of a relative path.
