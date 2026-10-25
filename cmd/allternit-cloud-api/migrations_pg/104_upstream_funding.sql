-- 104_upstream_funding.sql
--
-- Upstream funding coverage (`services/upstream_funding.rs`,
-- `routes/admin_funding.rs`).
--
--   * upstream_balances: one row per inference provider (openrouter, together,
--     fireworks, deepinfra, groq). balance_usd is the last known prepaid
--     balance at that provider. source 'api' = read from the provider
--     (OpenRouter GET /api/v1/credits); source 'manual' = the admin's last
--     entry (manual_balance_usd at manual_set_at) minus our metered upstream
--     spend since then (inference_usage joined to inference_pools).
--     reserve_days and floor_usd drive alerts (days left < reserve_days, or
--     balance < floor) and routing (a provider under its floor is skipped
--     when another provider serves the same model). A failed refresh keeps
--     the last balance and records last_error.
--   * upstream_balance_history: one row per refresh per provider (every
--     10 minutes), for the burn chart and audits.
--
-- Additive only, idempotent (safe to re-run).

CREATE TABLE IF NOT EXISTS public.upstream_balances (
    provider text PRIMARY KEY,
    balance_usd double precision,
    -- api | manual
    source text NOT NULL DEFAULT 'manual',
    reserve_days integer NOT NULL DEFAULT 3 CHECK (reserve_days >= 0),
    floor_usd double precision NOT NULL DEFAULT 0 CHECK (floor_usd >= 0),
    manual_balance_usd double precision,
    manual_set_at timestamp with time zone,
    burn_24h_usd double precision NOT NULL DEFAULT 0,
    last_error text,
    refreshed_at timestamp with time zone,
    updated_at timestamp with time zone NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS public.upstream_balance_history (
    id bigserial PRIMARY KEY,
    provider text NOT NULL,
    balance_usd double precision,
    burn_24h_usd double precision NOT NULL DEFAULT 0,
    source text NOT NULL,
    error text,
    recorded_at timestamp with time zone NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_upstream_balance_history_provider
    ON public.upstream_balance_history (provider, recorded_at DESC);
