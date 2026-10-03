-- 031_voice_billing_periods.sql
--
-- Monthly Cloud Voice + bot phone overage billing (wave 2, x7). Ships OFF:
-- ALLTERNIT_VOICE_BILLING_MODE=off|dry_run|live (default off). Applied by hand
-- to prod, like 020/021/024/029.
--
-- voice_billing_periods : one row per (user, UTC month). Holds what was
--   computed (minutes over the included allowance, phone minutes, phone numbers,
--   amount) and, for dry_run/approved runs, the exact Stripe request bodies.
--   state: pending (computed, nothing sent), dry_run (computed + bodies stored,
--   no Stripe call), invoiced (Stripe invoice items created), failed (a send
--   failed; error + any refs kept, the admin may approve again).
--   UNIQUE (user_id, period) + Stripe idempotency keys make re-runs safe.

CREATE TABLE IF NOT EXISTS public.voice_billing_periods (
    id bigserial PRIMARY KEY,
    user_id text NOT NULL,
    period text NOT NULL CHECK (period ~ '^[0-9]{4}-(0[1-9]|1[0-2])$'),
    plan_id text NOT NULL,
    cloud_seconds bigint NOT NULL DEFAULT 0,
    included_seconds bigint NOT NULL DEFAULT 0,
    cloud_overage_min bigint NOT NULL DEFAULT 0,
    phone_seconds bigint NOT NULL DEFAULT 0,
    phone_min bigint NOT NULL DEFAULT 0,
    numbers integer NOT NULL DEFAULT 0,
    amount_cents bigint NOT NULL DEFAULT 0,
    state text NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending', 'dry_run', 'invoiced', 'failed')),
    stripe_customer_id text,
    stripe_subscription_id text,
    request_bodies jsonb NOT NULL DEFAULT '[]'::jsonb,
    stripe_refs jsonb NOT NULL DEFAULT '{}'::jsonb,
    error text,
    approved_by text,
    approved_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (user_id, period)
);
CREATE INDEX IF NOT EXISTS idx_voice_billing_periods_period
    ON public.voice_billing_periods (period, state);
