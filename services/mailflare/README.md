# allternit agent email (mailflare fork)

The allternit **agent email rail**: a self-hosted mail service that gives every agent a
real mailbox on your own domain, with scoped API keys, approval-gated outbound mail, and
inbound webhooks — running entirely on **your own Cloudflare account**.

Outbound mail can use either **Cloudflare Email Sending** (default, requires the Workers
Paid plan) or **Resend** (free tier 100 emails/day, works on the free Workers plan). The
installer automates either path, including Resend domain verification and DNS records.

Forked from [`hieunc229/mailflare`](https://github.com/hieunc229/mailflare) (MIT-style
license; see [LICENSE](./LICENSE)). The fork is vendored at `services/mailflare` and adds
the agent-facing surface: per-mailbox scoped API keys, an `admin` scope, an outbound
approval queue, idempotent sends, per-key rate limits, durable webhook delivery with
retries, and mailbox deletion — plus `setup.sh`, a per-user installer that deploys
everything to the installing user's own Cloudflare account.

> The upstream one-click "Deploy to Cloudflare" flow and the dashboard self-updater were
> removed in this fork. Deploy with `./setup.sh` (recommended) or manually with wrangler,
> and update by pulling this monorepo.

## Architecture

```
                         your Cloudflare account
 ┌──────────────────────────────────────────────────────────────────┐
 │                                                                  │
 │  inbound                          Worker: allternit-agent-mail   │
 │  ────────                         ┌───────────────────────────┐  │
 │  Email Routing ──▶ email handler ─▶│ INBOUND_QUEUE (Queue)     │  │
 │  rule per mailbox                 │   └─▶ parse + store       │  │
 │  (agents.<domain>)                │        ├─▶ D1  (metadata) │  │
 │                                   │        └─▶ R2  (raw MIME, │  │
 │                                   │            attachments)   │  │
 │                                   │   └─▶ webhook fan-out ────┼──┼──▶ POST your
 │                                   │        (queued retries,   │  │    allternit host:
 │                                   │         HMAC-signed)      │  │    /api/v1/agent-email/inbound
 │  outbound                         │                           │  │
 │  ────────                         │  POST /api/v1/send        │  │
 │  agent ──▶ scoped API key ───────▶│   └─▶ outbound_jobs (D1)  │  │
 │                                   │        status:            │  │
 │                                   │        pending_approval   │  │
 │  human / platform ──▶ approve ───▶│   └─▶ OUTBOUND_QUEUE ─────┼──┼──▶ Cloudflare Email
 │  POST /api/v1/outbound/:id/approve│        (send_email        │  │    Sending  OR  Resend
 │                                   │         binding)          │  │
 │                                   │                           │  │
 │  dashboard (Next.js/OpenNext) ◀──▶│  REALTIME Durable Object  │  │
 │                                   │  (WebSocket new-mail push)│  │
 │                                   └───────────────────────────┘  │
 │  cron: DatabaseBackupWorkflow (D1 export, optional)              │
 └──────────────────────────────────────────────────────────────────┘
```

Everything is declared in `wrangler.jsonc`: D1 `allternit-agent-mail`, R2 bucket
`allternit-agent-mail-raw`, queues `allternit-agent-mail-inbound` /
`allternit-agent-mail-outbound`, the `REALTIME` Durable Object, and the
`LOGIN_RATE_LIMIT` / `SEND_RATE_LIMIT` rate-limit bindings. The D1 `database_id` is
**per-account** and is written into `wrangler.jsonc` by the installer — never commit one.

## Customer domains (mx.allternit.com)

A domain that isn't a zone in this Cloudflare account (a customer's `acme.com`) is a
`relay` domain (`domains.transport`, migration `0012_relay_domains.sql`). Its mail runs
through Allternit's mail host and `services/mail-relay`: `PUT/GET/DELETE
/api/v1/relay/domains/:host` (admin scope) add it, check its DNS and remove it; the relay
calls `POST /api/v1/relay/inbound` and `GET /api/v1/relay/recipient`; outgoing mail from a
relay domain is sent by the relay, DKIM-signed with the domain's key. Set `MAIL_RELAY_URL`
and `MAIL_RELAY_SECRET` (`wrangler secret put MAIL_RELAY_SECRET`).

## Approval-gated outbound

This fork never lets an agent send mail directly to the internet by default
(`REQUIRE_SEND_APPROVAL=true`):

1. `POST /api/v1/send` validates the request, enforces the per-key rate limit and
   mailbox scope, stores the message, and creates an `outbound_jobs` row with status
   `pending_approval`. Response: `{ messageId, jobId, status: "pending_approval" }`.
2. A human (or the allternit platform's decide step) reviews and calls
   `POST /api/v1/outbound/<jobId>/approve` — or `/reject` to drop it.
3. Approval moves the job to `queued` and hands it to `OUTBOUND_QUEUE`; the queue
   consumer delivers it through the Cloudflare Email Sending binding and records the
   outcome (`sent` / `failed`) on both the job and the message.

Sending the same `Idempotency-Key` header twice replays the original response instead of
double-sending. Set `REQUIRE_SEND_APPROVAL=false` to restore synchronous sending.

## API surface

Authentication is either a dashboard session (cookie or session Bearer token from
`POST /api/auth/login`) or an API key (`Authorization: Bearer ep_...`). API keys carry
scopes — `send`, `read`, `admin` — and can be restricted to specific mailboxes
(`mailboxIds`); mailbox-scoped keys get `403` outside their mailboxes.

### v1 agent API (API key auth)

| Route | Scope | Notes |
|-------|-------|-------|
| `POST /api/v1/send` | `send` | Body: `from`, `to`, `subject`, `text`/`html`, `mailboxId`, optional `attachments` (base64, ≤10 files). Honors `Idempotency-Key` header. Rate-limited per key (30 req/min). |
| `GET /api/v1/messages` | `read` | List messages; `?mailboxId=`, `?direction=`, `?limit=` (≤100). |
| `GET /api/v1/messages/[id]` | `read` | Full message with body; attachment entries include signed `downloadUrl`s. |
| `GET /api/v1/messages/[id]/attachments/[attachmentId]` | signed URL | Downloads attachment content from R2 via the `token`/`expires` query params from the message read. |
| `POST /api/v1/outbound/[id]/approve` | `send` | Approve a `pending_approval` job → `queued`. `409` if not pending. |
| `POST /api/v1/outbound/[id]/reject` | `send` | Reject a pending job → `failed` (`rejected`), nothing is sent. |

### Management API (session auth unless noted)

| Route | Notes |
|-------|-------|
| `GET /api/setup/status` | Unauthenticated first-run probe (`hasAdminAccount`, `primaryDomain`). |
| `POST /api/auth/register` | First-run only; creates the admin account + primary domain and **closes registration**. Body: `domain`, `username`, `password`, `resetEmail`. |
| `POST /api/auth/login` | Body: `email`, `password` → session token. Rate-limited (20 req/min). |
| `GET/POST /api/api-keys` | Create keys with `name`, `scopes`, optional `mailboxIds`; the full key is returned **once**. |
| `DELETE /api/api-keys/[id]` | Revoke (soft delete; the key stops authenticating immediately). |
| `GET/POST /api/mailboxes` | Also accepts an `admin`-scope Bearer key. `POST` creates the Cloudflare Email Routing rule for the address automatically. |
| `DELETE /api/mailboxes/[id]` | `admin` scope; removes the routing rule(s) and disables the mailbox (messages are kept). |
| `GET/POST /api/domains` | `GET` accepts an `admin` key (`?includeDns=true` for a DNS status summary); `POST` (session) adds a domain and calls Cloudflare. |
| `GET/POST /api/webhooks` | Register `message.inbound` / `message.outbound` / `message.failed` endpoints. The per-webhook signing `secret` is returned at creation. |
| `DELETE /api/webhooks/[id]` | Remove a webhook. |
| `GET /api/webhooks/[id]/deliveries` | Delivery history (status, attempts) for a webhook. |

Webhook deliveries are durable: each event is persisted and POSTed via the inbound queue
with HMAC-SHA256 signature headers — `X-Email-Platform-Signature` (hex of the raw body,
keyed with the webhook secret) and `X-Email-Platform-Event`. Failures retry with
exponential backoff (30s → 60s → 120s → 240s, 5 attempts max) before being marked
`failed`.

## Setup (recommended): `./setup.sh`

```bash
cd services/mailflare
./setup.sh
```

The interactive installer deploys to **your own Cloudflare account**. It is idempotent
and safe to re-run: on startup it shows what a previous run already completed (D1
provisioned? worker live? admin registered? — read from the gitignored `.env.install`,
`wrangler.jsonc`, and `/api/setup/status`), offers to resume, and skips completed steps.
`--reconfigure` starts over. It will:

1. Check prerequisites (Node ≥ 18, npm, wrangler, optional jq). If wrangler isn't
   authenticated it runs `npx wrangler login` for you (opens a browser) and waits —
   note that wrangler OAuth covers deploys and D1/R2/Queues only, **not** the Email
   Routing / DNS APIs, which need the scoped token below.
2. Collect a scoped API token, account ID, zone domain, agent subdomain
   (default `agents.<zone>`), worker name (default `allternit-agent-mail`), and
   outbound transport (`cloudflare` or `resend`).
3. Verify the token is `active`, resolve the zone ID, then **probe the token's
   permissions empirically** with read-only calls — token verification alone does not
   prove permissions, and scoped-token introspection is not available (9109). If
   anything is missing, the installer prints the exact missing groups plus the full
   required list, deep-links the token page, and re-probes when you've saved. The
   required groups are smaller for `resend` because outbound sending leaves Cloudflare.
4. Provision D1 / R2 / both queues (check-before-create), write the per-install
   `database_id` into `wrangler.jsonc`, and set `CF_TOKEN` / `CF_ACCOUNT_ID` as Worker
   secrets (never echoed).
5. Run remote D1 migrations and deploy the Worker.
6. Enable Email Routing + MX/SPF/DKIM records for inbound mail. For `cloudflare`
   transport, enable the Cloudflare Email Sending subdomain and poll until its DNS
   records are in place. For `resend` transport, create the Resend domain, add its
   SPF/DKIM records to Cloudflare DNS, and poll until Resend marks the domain
   verified. Creates a `_dmarc.agents.<zone>` record in both cases (or prints it if
   the token lacks Zone DNS Edit).
7. Wait for the Worker, register the first admin, and create the
   `allternit-integration` admin-scope API key — printed **once**. Registration rolls
   back cleanly on failure, so a Cloudflare error never blocks a retry.
8. Merge `ALLTERNIT_MAILFLARE_URL`, `ALLTERNIT_MAILFLARE_ADMIN_KEY`,
   `ALLTERNIT_BOT_EMAIL_DOMAIN`, and `ALLTERNIT_MAILFLARE_WEBHOOK_SECRET` into the
   repo-root `.env` (never overwrites non-empty values without confirmation).
9. Optionally run a smoke test: create `smoketest@agents.<zone>`, send a gated outbound,
   assert `pending_approval`, approve it, then clean up (mailbox deleted, key revoked).
   It can also register the `message.inbound` webhook pointing at your allternit host.

Non-interactive/CI runs: `./setup.sh --non-interactive` never prompts and fails fast
with a list of missing variables. Required: `CF_TOKEN`, `CF_ACCOUNT_ID`,
`MAILFLARE_ZONE`, `ADMIN_PASSWORD` (values saved in `.env.install` count). Optional:
`AGENT_SUBDOMAIN`, `WORKER_NAME`, `MAILFLARE_WORKER_URL`, `ALLTERNIT_HOST`,
`ADMIN_USERNAME`, `ADMIN_RECOVERY_EMAIL` (required on a first-run register),
`EMAIL_TRANSPORT` (`cloudflare` or `resend`; defaults to `cloudflare`),
`RESEND_API_KEY` (required when `EMAIL_TRANSPORT=resend`). Other flags:
`--skip-smoke-test`, `--reconfigure`, `--help`.

### Two things Cloudflare makes un-automatable

Both are detected by the installer, which deep-links the exact dashboard page and
waits/polls until you've finished — you never see a raw API error:

1. **Creating the scoped API token.** Tokens can only be created in the dashboard at
   <https://dash.cloudflare.com/profile/api-tokens>. Getting the permission groups
   exactly right on the first try is unusual; the installer's empirical preflight
   (step 3) tells you precisely what to add, and you edit the *same* token — no
   re-entry into the installer.
2. **One-time R2 activation.** The first R2 operation on an account fails with API code
   10042 until R2 is enabled in the dashboard (this may require adding a payment
   method). When the installer hits this it prints
   `https://dash.cloudflare.com/<your-account-id>/r2` and polls every 15 seconds (up to
   10 minutes) until R2 is enabled, then continues automatically.

### Required Cloudflare token permissions

Verified against a live install — create the token at
<https://dash.cloudflare.com/profile/api-tokens>.

**For `EMAIL_TRANSPORT=cloudflare` (default):**

- **Account:** Email Sending **Edit**, Email Routing Addresses **Edit**, DNS Settings **Edit**
- **Zone (your domain):** Zone **Read**, DNS **Edit**, DNS Settings **Edit**,
  Zone Settings **Edit**, Email Routing Rules **Edit**

**For `EMAIL_TRANSPORT=resend`:**

- **Zone (your domain):** Zone **Read**, DNS **Edit**, Email Routing Rules **Edit**

Resend handles outbound delivery, so no Account-level Email Sending permission or
Workers Paid plan is required.

Scope the token to your account and the specific zone (or All zones). This is a
**runtime** token (stored as the `CF_TOKEN` Worker secret) — it is separate from
whatever wrangler uses to deploy.

## Manual deploy

```bash
npm install
npx wrangler login

# provision (names must match wrangler.jsonc)
npx wrangler d1 create allternit-agent-mail        # put the database_id into wrangler.jsonc
npx wrangler r2 bucket create allternit-agent-mail-raw
npx wrangler queues create allternit-agent-mail-inbound
npx wrangler queues create allternit-agent-mail-outbound

# runtime secrets
printf '%s' "$CF_TOKEN" | npx wrangler secret put CF_TOKEN
printf '%s' "$CF_ACCOUNT_ID" | npx wrangler secret put CF_ACCOUNT_ID
# If using resend:
# printf '%s' "$RESEND_API_KEY" | npx wrangler secret put RESEND_API_KEY

npm run db:migrate:remote
npm run deploy
```

`npm run deploy` intentionally builds with OpenNext and uploads with wrangler — do not
replace it with `opennextjs-cloudflare deploy`: `worker.ts` additionally exports the
`RealtimeHub` Durable Object and the email/queue handlers. `npm run deploy:with-migrations`
combines both steps (requires the `database_id` in `wrangler.jsonc`).

Local development: `cp .dev.vars.example .dev.vars`, fill in `CF_TOKEN` /
`CF_ACCOUNT_ID`, then `npm run db:migrate:local` and `npm run dev`.

### Environment variables

| Variable | Where | Purpose |
|----------|-------|---------|
| `CF_TOKEN` | secret | Scoped Cloudflare API token used at runtime for Zone/Email Routing/Sending API calls. Token secret value only — no `Bearer` prefix. |
| `CF_ACCOUNT_ID` | secret | Cloudflare account ID. |
| `CF_EMAIL_WORKER_NAME` | var (`wrangler.jsonc`) | Worker name used when creating Email Routing rules. **Must match the deployed Worker name exactly.** |
| `REQUIRE_SEND_APPROVAL` | var (`wrangler.jsonc`) | `true` (default) gates outbound mail behind approval; `false` sends synchronously. |
| `EMAIL_TRANSPORT` | var (`wrangler.jsonc`) | `cloudflare` (default) or `resend`. Selects the outbound provider; the installer writes this into `wrangler.jsonc`. |
| `RESEND_API_KEY` | secret | Required when `EMAIL_TRANSPORT=resend`. Get it at <https://resend.com/api-keys>. |
| `RESEND_API_BASE` | var/secret | Optional Resend API override (default `https://api.resend.com`). |
| `TURNSTILE_SECRET_KEY` | secret | Optional Turnstile secret for login/register challenges. |
| `NEXT_PUBLIC_TURNSTILE_SITE_KEY` | build-time | Optional Turnstile site key paired with the secret. |
| `CF_AID` + `D1_DATABASE_ID` + `D1_BACKUP_TOKEN` | secrets | Optional; for the nightly D1 backup workflow. The `workflows` block in `wrangler.jsonc` ships commented out (accounts without the Workflows product fail to deploy otherwise) — only re-enable it together with these secrets on an account that has Workflows. |
| `CF_EMAIL` + `CF_API_KEY` | secrets | Legacy Global API Key auth — use only instead of `CF_TOKEN`. |

Platform side (repo-root `.env`, written by `setup.sh`): `ALLTERNIT_MAILFLARE_URL`,
`ALLTERNIT_MAILFLARE_ADMIN_KEY`, `ALLTERNIT_BOT_EMAIL_DOMAIN`,
`ALLTERNIT_MAILFLARE_WEBHOOK_SECRET`.

## Deliverability ops

Agent mail is a reputational liability — run it like one:

- **Subdomain isolation.** All agent mail lives on `agents.<your-domain>`, never the
  root domain. A dedicated subdomain protects the root domain's sender reputation.
- **One spamming agent can burn the subdomain.** Use per-agent mailboxes with
  mailbox-scoped keys (the per-key `SEND_RATE_LIMIT` contains the blast radius) and keep
  the approval gate on. Revoking a compromised key is one `DELETE /api/api-keys/[id]`.
- **Warm up.** Keep volume low for the first few weeks (tens of messages/day), ramp
  slowly, and avoid bursts — the rate limiter is a safety net, not a sending plan.
- **DMARC.** The installer creates `_dmarc.agents.<zone>` with `p=quarantine`. Keep it
  there until you're confident nothing legitimate fails alignment; tighten to `p=reject`
  later. SPF/DKIM come from the Email Routing / sending-subdomain DNS records.
- **Volume discipline.** Agents should batch and summarize rather than reply-all at
  machine speed. Watch `message.failed` webhooks and the dashboard for bounce patterns.

## Troubleshooting

**Cloudflare API 403, code 9109: Invalid access token.**
Verify the token:

```bash
curl "https://api.cloudflare.com/client/v4/user/tokens/verify" \
  -H "Authorization: Bearer <CF_TOKEN>"
```

The response must contain `"success": true` and `"status": "active"`. Set `CF_TOKEN` to
the token secret value only — no `Bearer` prefix, not the token ID, and not a Global API
Key (for a Global API Key use `CF_EMAIL` + `CF_API_KEY` instead). Also check token
expiry, `not_before`, and client-IP restrictions, and redeploy after changing secrets.

**Cloudflare API 403 on `/zones/{zone_id}/email/routing/dns`, code 10000 (or 2036 on
`/email/sending/subdomains`).**
The token is missing permission groups — 10000 points at the zone-level Email Routing /
DNS groups, 2036 at the account-level Email Sending group. Run `./setup.sh` and let the
permission preflight print the exact diff, or compare against the verified required list
above.

**Deploy fails with "Some triggers failed to deploy … /workflows/allternit-agent-mail-database-backup".**
The account doesn't have the Cloudflare Workflows product. The `workflows` block in
`wrangler.jsonc` ships **commented out** for this reason — keep it commented (the nightly
D1 backup is optional). The installer detects this specific failure and prints the same
guidance. Similarly, deploy fails if the R2 bucket doesn't exist yet; the installer's R2
step (including the 10042 activation polling) must complete before deploy.

**R2 API code 10042 "Please enable R2 through the Cloudflare Dashboard".**
One-time account activation: open `https://dash.cloudflare.com/<account-id>/r2`, enable
R2 (may require a payment method). `./setup.sh` polls for this automatically.

**D1 error 7404: database could not be found.**
The `database_id` in `wrangler.jsonc` belongs to a different Cloudflare account. D1 IDs
are account-specific — delete the stale ID and re-run `./setup.sh` (or
`npx wrangler d1 create allternit-agent-mail` and paste in the new ID). Never commit a
`database_id` to the repo.

**Inbound mail isn't arriving / routing rules point at the wrong worker.**
`CF_EMAIL_WORKER_NAME` must exactly match the deployed Worker name — the app fails with
a clear setup error when it's missing rather than silently defaulting. If you rename the
Worker, update all three places: `name` and `services[].service`
(`WORKER_SELF_REFERENCE`) in `wrangler.jsonc`, and the `CF_EMAIL_WORKER_NAME` var.
`./setup.sh` keeps them in sync for you.

**Registration returns 403 "Registration is closed".**
An admin account already exists — first-run registration is permanently closed after the
first account. Log in instead (or use `GET /api/setup/status` to check). On re-runs the
installer detects this and asks for the existing admin credentials.

**Login/register returns "Verification failed".**
Turnstile is configured but the challenge token is missing/invalid. Either complete the
Turnstile widget in the dashboard, or unset `TURNSTILE_SECRET_KEY` (verification is
skipped when no secret is configured).

## License

Forked from [`hieunc229/mailflare`](https://github.com/hieunc229/mailflare). See
[LICENSE](./LICENSE); upstream attribution retained per the license terms.
