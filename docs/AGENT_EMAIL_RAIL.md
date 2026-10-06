# Agent Email Rail (mailflare)

Real internet email for platform agents: every agent can own a working address
(`<agent>@agents.<your-domain>`), receive mail programmatically, and send mail to
humans through a human-approval gate. Each allternit installation deploys its own
mailflare worker to the **installing user's own Cloudflare account** — there is no
shared allternit-hosted mail infrastructure.

## Components

| Piece | Location | Role |
|-------|----------|------|
| mailflare worker | `services/mailflare/` | Vendored fork of [hieunc229/mailflare](https://github.com/hieunc229/mailflare). Cloudflare Worker + D1 + R2 + Queues. Email Routing terminates inbound; outbound uses Cloudflare Email Sending (default) or Resend (`EMAIL_TRANSPORT=resend`). |
| Installer | `services/mailflare/setup.sh` | Per-user setup: token/zone checks, D1/R2/queue provisioning, deploy, Email Routing DNS, admin bootstrap, integration key, smoke test. |
| Unified connector installer | `scripts/install-connectors.sh` | One-command setup for Allternit Mail, Gmail, and Google Drive. See [`docs/learnings/CONNECTOR_SETUP.md`](./learnings/CONNECTOR_SETUP.md). |
| Backend integration | `cmd/allternit-api/src/agent_email_routes.rs`, `mailflare_client.rs` | Provisioning (`provision_email`), inbound webhook bridge, approval-gated outbound, status. |
| Review gate | `factory/engine/src/orchestration/mail/` + `POST /api/factory/mail/decide` | The existing Factory mail review flow releases or rejects pending outbound email. No separate approval UX. |
| UI | `surfaces/ai.allternit.com` | Identity channels step, email-rail status line, `external email` badges in Mail Monitor / agent activity. |
| Runtime client | `cmd/gizzi-code/src/cli/agent-email-client.ts` | `sendAgentEmail()`, `getAgentEmailStatus()`; `gizzi mail send-external` / `gizzi mail email-status`. |

## Flows

**Inbound**: Cloudflare Email Routing → worker `email()` handler → raw MIME to R2 →
queue → parse + store (D1) → HMAC-SHA256-signed webhook (`message.inbound`) →
`POST /api/v1/agent-email/inbound` (signature verified against
`ALLTERNIT_MAILFLARE_WEBHOOK_SECRET`) → Factory mail thread `mail:email-in-<agent>` →
visible in Mail Monitor and the FTS mail index. Mailflare retries failed webhook
deliveries with backoff (5 attempts).

**Outbound**: agent calls `POST /api/v1/agent-email/send` (or
`gizzi mail send-external`) → ownership + `email_send_enabled` + hard-ban checks →
pending row + mailflare `pending_approval` job (idempotency key `agent-email:<uuid>`)
→ `ReviewRequested` on thread `mail:email-out-<uuid>` → human approves via
`/api/factory/mail/decide` (or the review card UI) → mailflare sends through the
configured transport (Cloudflare Email Sending or Resend) → receipt with provider
message id written to the ledger. Rejection marks the job failed; nothing is sent.

## Environment

Written by the installer into the platform `.env`:

- `ALLTERNIT_MAILFLARE_URL` — deployed worker URL.
- `ALLTERNIT_MAILFLARE_ADMIN_KEY` — admin-scope mailflare API key (integration credential; treat as a root secret for the mail rail).
- `ALLTERNIT_BOT_EMAIL_DOMAIN` — agent email domain, e.g. `agents.example.com`.
- `ALLTERNIT_MAILFLARE_WEBHOOK_SECRET` — HMAC secret for the inbound webhook (returned by mailflare when the webhook is registered).

Without `ALLTERNIT_MAILFLARE_URL`/`ALLTERNIT_MAILFLARE_ADMIN_KEY`, email
provisioning falls back to the legacy mint-only behavior (identity provider `commrails`) <!-- old-names: keep (stored data value) --> and
`/api/v1/agent-email/send` returns `501 mailflare_not_configured`.

### Outbound transport

Mailflare can send outbound mail through either provider:

- **`EMAIL_TRANSPORT=cloudflare`** (default): uses the Cloudflare Email Sending
  `send_email` binding. Requires the Workers Paid plan and the Email Sending
  entitlement on the account.
- **`EMAIL_TRANSPORT=resend`**: uses the [Resend](https://resend.com) API. Works on
  the free Workers plan; Resend's free tier is 100 emails/day. The installer creates
  the Resend domain, adds its SPF/DKIM records to Cloudflare DNS, and polls until
  Resend verifies the domain.

The transport is set in `services/mailflare/wrangler.jsonc` (`EMAIL_TRANSPORT` var)
and, for Resend, the `RESEND_API_KEY` Worker secret.

## Reputation management (read before enabling agents to send)

Sender reputation is the scarce resource. One misbehaving agent can get the whole
agent subdomain junked; if you ignored the subdomain advice, your root domain too.

- **Always use a dedicated subdomain** (default `agents.<domain>`). The installer
  sets this up, including SPF/DKIM via Cloudflare (and Resend, when selected) and a
  DMARC record. Never let agents send from your human mail domain.
- **Warm up.** New domains/subdomains have no reputation. Keep volume low for the
  first weeks (tens of messages/day, not hundreds) and ramp gradually.
- **DMARC**: installer creates `_dmarc.<subdomain>` with `p=quarantine`. Move to
  `p=reject` once you trust the stream.
- **Rate limits are enforced worker-side**: 30 sends/minute per API key
  (`SEND_RATE_LIMIT` binding in `services/mailflare/wrangler.jsonc`). Per-agent
  keys are mailbox-scoped, so a compromised key can only send from its own address.
- **The approval gate is on by default** (`REQUIRE_SEND_APPROVAL=true`). Outbound
  mail does not leave until a human approves it. Turning this off
  (`REQUIRE_SEND_APPROVAL=false` in worker vars) means agents send unsupervised —
  only do this with hard daily caps and allowlisted recipients.
- **Idempotency**: sends carry `Idempotency-Key: agent-email:<uuid>`; retries and
  double-clicks cannot produce duplicate mail.
- **Monitor**: mailflare `audit_logs` (every send/approve/reject),
  `webhook_deliveries` (inbound delivery health), and the allternit ledger receipts.
  Watch bounce/spam signals in your Cloudflare Email Sending dashboard.

## Automated signups (agents registering for services)

Agents can receive verification emails and magic links — the inbound path delivers
them to the agent's Factory mail thread, and full bodies are readable via
`GET /api/v1/messages/[id]` with the agent's key. Caveats:

- Many services' ToS prohibit automated accounts; a signup spree can burn the
  subdomain's reputation even when technically successful. Keep the approval gate
  on for external recipients and review signup mail like any other outbound.
- Email verification is only one leg: CAPTCHAs, phone verification, and JS-gated
  forms are separate problems this rail does not solve.

## Operations

- **Provision**: `POST /api/v1/agents/:id/identity/email` (idempotent; re-run
  returns the existing channel).
- **Diagnose**: `GET /api/v1/agent-email/status` (configured / domain / worker
  reachable) or `gizzi mail email-status`.
- **Revoke an agent's mail**: deleting an agent best-effort deletes its mailflare
  mailbox (routing rule removed, mailbox disabled; stored messages retained).
  Note: mailflare API-key revocation is currently session-auth only — mailbox
  deletion is the effective revocation path for agent keys.
- **Re-run the installer**: `services/mailflare/setup.sh` is idempotent and detects
  prior state via `services/mailflare/.env.install`. For the full first-party
  connector stack, use `scripts/install-connectors.sh` instead.
- **Operator dashboard**: the mailflare web UI remains available to the operator
  admin created at install time (registration closes after first-run).

## Known limitations

- No catch-all addresses: each agent address must exist as a mailbox (the platform
  creates them at provisioning).
- Per-agent API keys cannot be revoked via the admin integration key (mailflare
  key revocation is dashboard-session only); disable = delete mailbox.
- Inbound webhook payloads carry a truncated body (4 KB); full bodies via the v1
  messages API.
- mailflare is effectively single-operator (one admin user); multi-operator
  dashboard delegation is out of scope.
