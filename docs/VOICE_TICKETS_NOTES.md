# Track G notes: voice tickets + voice minutes metering

**Status: done, pushed, not merged (no PR, per task).** Migration 029 must be run by hand in prod before/with the deploy.

## Commits / branch
- Branch `ao/voice-tickets` (pushed), rebased on origin/main after the last fetch:
  - `feat(cloud-api): voice tickets + Cloud Voice minutes metering (pg 029)`
  - a final commit with these docs/sentinel only.

## Migration
`029_voice_usage.sql` (029 free on origin/main and all origin/ao/* branches at rebase time; 024/025/028 taken, 022 now landed).

## Files
- `cmd/allternit-cloud-api/migrations_pg/029_voice_usage.sql`
- `cmd/allternit-cloud-api/src/services/voice_usage.rs`: pub pricing consts, `record`, `month_usage`, `plan_for_user`, month math
- `cmd/allternit-cloud-api/src/routes/voice_tickets.rs`: 4 endpoints + tests
- registration lines: `services/mod.rs`, `routes/mod.rs`, `lib.rs` (public_runtime_routes group)
- `cmd/allternit-cloud-api/.env.example`, `surfaces/docs/api/voice-tickets.mdx`, `surfaces/docs/docs.json` nav (after `api/voice`)

## Decisions worth knowing
- Plan source: `billing_subscriptions` (active/trialing), admins = ultra, else free: the same lookup `GET /api/v1/me/usage` uses. `QuotaService.plan_tier_id` is a runtime-quota tier that nothing syncs from billing, so it was not used. No second plan source added.
- Plus/Super/Ultra all get 100 included min and may go into overage; Free gets 0 and 402 `no-cloud-minutes`.
- Secret and worker token must be >= 32 chars to count as configured (same rule as the billing secrets); otherwise 503 `cloud-unavailable`.
- Routes `/api/v1/voice/tickets*` and `/api/v1/voice/usage*` were not served by anything else; full-router test passes (no overlap panic). joe-07 can reuse `voice_usage::{record, PHONE_*}` for phone (`engine="phone"`).
- No Stripe calls anywhere.

## Tests (local PG 16 on :54329, `TEST_DATABASE_URL`)
- `cargo test -p allternit-cloud-api --lib voice`: 17 passed (ticket good/tampered/expired/wrong secret/malformed, single-use redeem + purge, worker bearer auth, idempotent record, month boundary, 402, 503 unconfigured, month endpoint, full router builds and serves the routes).
- Full lib: 430 passed, 1 failed: `services::contabo_runtime_service::tests::provision_creates_container_and_instance_record` needs a `docker` binary absent on this machine; unrelated to this change.

## Boot check
Built the real binary and booted it against an empty local PG: all migrations (incl. 029) applied, both tables exist, no panic; `POST /voice/tickets/redeem` with the worker bearer returned `401 invalid-ticket` for a junk ticket. (Unauthenticated curl probes returned a generic 400 from the middleware stack, not tested further; the router test covers 401/503.)

## Prod env vars to set
- `ALLTERNIT_VOICE_TICKET_SECRET` (>= 32 chars, shared with the voice service)
- `ALLTERNIT_VOICE_CLOUD_WS_URL` (e.g. `wss://voice.allternit.com/v1/voice/session`)
- `ALLTERNIT_VOICE_WORKER_TOKEN` (>= 32 chars, voice service bearer)

## SQL to run by hand in prod (as postgres)
```sql
CREATE TABLE IF NOT EXISTS public.voice_usage (
    id bigserial PRIMARY KEY,
    user_id text NOT NULL,
    engine text NOT NULL CHECK (engine IN ('cloud', 'phone')),
    seconds integer NOT NULL CHECK (seconds >= 0),
    ref text NOT NULL,
    occurred_at timestamptz NOT NULL DEFAULT now(),
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (engine, ref)
);
CREATE INDEX IF NOT EXISTS idx_voice_usage_user_time
    ON public.voice_usage (user_id, occurred_at);

CREATE TABLE IF NOT EXISTS public.voice_tickets_used (
    nonce text PRIMARY KEY,
    user_id text NOT NULL,
    expires_at timestamptz NOT NULL,
    used_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_voice_tickets_used_expires
    ON public.voice_tickets_used (expires_at);
```
Grant the app role access if it isn't the owner: `GRANT SELECT, INSERT, DELETE ON public.voice_usage, public.voice_tickets_used TO <app_role>; GRANT USAGE ON SEQUENCE public.voice_usage_id_seq TO <app_role>;`

## Cleanup
`.voice-target-g` removed; temp Postgres removed.

## Not mine
The Stop hook's git-discipline gate complained about the shared `allternit` checkout (main 22 behind, stale `ao/c-surfaces-docs-platform`). The task forbids running git there, so I left it for the orchestrator.
