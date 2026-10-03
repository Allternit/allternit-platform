# Bot email phase 2 notes: mailflare as one multi-tenant service

Design only — no code. Grounded in `services/mailflare` as it exists today
(per-user installer: `setup.sh` deploys the whole Worker stack into the
installing user's own Cloudflare account — D1, R2, two Queues, Email Routing
rules, one admin user; webhooks are per-user rows with a shared secret and a
URL; mailboxes and scoped keys are admin-scope API operations).

## Why change it

Today every operator who wants agent email runs the installer against their
own Cloudflare account, with their own zone, tokens, and D1. For the product
("email a bot, get an answer", sold to companies) that doesn't scale:

- Provisioning a bot mailbox would require the customer to own a domain and a
  Cloudflare account. Companies want `@ourproduct.allternit.com` addresses
  that work the moment they create a bot.
- The allternit-api holds one `ALLTERNIT_MAILFLARE_URL` + admin key pair, so
  every runtime today points at exactly one mailflare installation anyway.
- Reputation, DMARC alignment, bounce handling, and the approval gate are
  operationally identical for every tenant; running N copies buys nothing.

## Target shape

One mailflare deployment, operated by Allternit, on the `allternit.com`
zone (a dedicated subdomain for bot mail, e.g. `*.bots.allternit.com`):

- **Tenants, not installations.** A tenant row (the Allternit user) owns
  mailboxes, API keys, webhooks, and messages. The schema already scopes
  every table by `userId` — multi-tenancy is a deployment question, not a
  data-model question. What must change is everything that assumes the
  installing user is the platform: first-run registration, the seed admin,
  and `setup.sh` itself.
- **Admin becomes platform-internal.** Allternit-api is the only holder of
  the admin-scope key (as today). End users never get mailflare credentials;
  they get an agent with an email address. The mailflare dashboard can stay
  enabled as an operator backoffice, but it is not part of the product path.
- **Domains become data.** Keep the `domains` table: row per subdomain
  (`bots.allternit.com`), verified once via Cloudflare (it is our own zone,
  so verification is a DNS record we add ourselves). `email_domain_verified`
  on `agent_identity_channels` then mirrors a real, automatable check
  (zone-level SPF/DKIM/DMARC for the sending subdomain) instead of a flag
  only tests can set.

## Webhook per mailbox pointing at the owner's relay address

Today the per-user installer points the user's Email Routing at wherever
they configured their webhook. Multi-tenant, the flow inverts — the webhook
URL is chosen by allternit-api at provision time:

1. Bot mailbox creation (allternit-api, admin key): `POST /api/mailboxes`
   with the bot's local part. mailflare creates the mailbox, the routing
   rule, and a **webhook row** in one transaction:
   - `url`: the owner's relay address. For a cloud computer this is
     `https://api.allternit.com/channels/in/<key>` (the phase-1
     `channel_inbound.rs` email arm), obtained from cloud-api's
     `POST /api/v1/channel-inbound-routes {runtimeId, provider: "email"}`.
     For a Desktop runtime, the equivalent direct URL it exposes.
   - `secret`: random per webhook (already the shape today).
   - `events`: `["message.inbound"]` (bot mail doesn't need outbound/failed
     fanout to the relay; the API tracks those in `agent_email_outbound`).
2. mailflare signs each delivery with the webhook secret
   (`X-Email-Platform-Signature`, HMAC-SHA256 hex of the body — unchanged),
   and the runtime verifies with `ALLTERNIT_MAILFLARE_WEBHOOK_SECRET`...
   **except per-webhook secrets make that env model wrong**: with one
   mailflare serving many runtimes, each webhook row's secret is distinct,
   so the runtime cannot hold them all in env.
   Phase 2 decision: make the webhook secret **per-mailbox and delivered to
   the runtime through provisioning**, not through installer env. Concretely:
   allternit-api stores the sealed secret alongside the sealed mailbox key in
   `agent_identity_channels` (same `token_crypto::seal` treatment), and
   `verify_mailflare_signature` opens the channel's secret instead of reading
   one global env var. The global env stays as the fallback for the
   single-tenant deployments that still exist.

## Provisioning: creating a bot mailbox registers the route

Sequence (all steps exist today; what changes is who runs them):

1. User turns on email for a bot (wave-3 wizard) → allternit-api
   (admin key): `POST /api/mailboxes {domainId, localPart}` → mailbox +
   Cloudflare Email Routing rule for `botname@bots.allternit.com`.
2. allternit-api: `POST /api/api-keys` mints the mailbox-scoped
   `["send","read"]` key, sealed into `email_api_key_sealed` (today's path).
3. NEW: allternit-api asks cloud-api for the owner's runtime relay
   (`channel-inbound-routes`, provider `email`), then
   `POST /api/webhooks {url, events, mailboxId}` on mailflare. The webhook
   row is the single source of truth for "where this mailbox's mail goes".
4. mailflare delivery stays exactly as phase 1: queued, retried with
   backoff, HMAC-signed, `message.inbound` payload with threading headers and
   `authResults`. The relay queues and wakes the runtime; the runtime's
   `receive_inbound_email` is unchanged.

Deprovisioning reverses in order: delete the webhook (mail stops entering the
queue), delete the mailbox (routing rule removed), revoke the scoped key.

## What stays the same

- The approval gate (`REQUIRE_SEND_APPROVAL`) and the admin-scope
  skip-approval escape hatch from phase 1. In a multi-tenant deployment the
  human review UI is the Allternit product's, so the mailflare dashboard's
  own review endpoints matter less, but the gate itself is the safety story.
- The phase-1 loop guards, reply modes, caps, and kill switch — they live in
  allternit-api, per bot, and are tenant-agnostic.
- Idempotency keys and per-key send rate limits (phase 1), which keep
  mailflare safe when one deployment serves every tenant.

## Rollout

1. Stand up the shared deployment under the Allternit zone; run `setup.sh`
   once with Allternit-admin credentials (it becomes the operator backoffice,
   not a customer step).
2. Point the existing `ALLTERNIT_MAILFLARE_URL`/`ADMIN_KEY` env at it
   (no code change — same contract).
3. Per-webhook secrets + relay provisioning as above (small, additive).
4. Migrate existing per-user installs by re-creating their mailboxes on the
   shared deployment and flipping DNS; `setup.sh` gains a `--decommission`
   that deletes routing rules and the worker but keeps the raw-mail R2 bucket
   readable for the retention window.

## Open questions

- Sending identity: phase 1 replies come from the bot's own address on the
  shared subdomain. If a customer wants `bot@mail.customer.com`, that's a
  future "bring your own domain" row in `domains` with their SPF/DKIM —
  out of scope here.
- Webhook secrets per mailbox vs per relay: per mailbox is finer-grained but
  means a secret rotation per bot. Per relay is coarser but matches the
  relay's key lifecycle. Start per mailbox; revisit if rotation pain shows.
