-- 080_platform_billing.sql
--
-- Platform API P5 billing (backend):
--
-- * `platform_usage_events.amount_microusd`: the billed amount of a row whose
--   price isn't a fixed per-unit price (text conversation tokens: provider list
--   price + 15%, 0 on the project's own model key). NULL for fixed-price meters,
--   which are priced from the meter table at read time.
-- * `platform_usage_events.model`: the model a token row was used on.
-- * `platform_usage_events.stripe_reported_at`: when the Stripe meter-event
--   reporter sent the row (it runs only with ALLTERNIT_PLATFORM_STRIPE_METERING=1).
-- * `platform_projects.stripe_customer_id`: the Stripe customer a live project
--   bills to (set when a card is on file; nothing sets it yet).
-- * `platform_spend_alerts`: one row per project, calendar month and threshold
--   (50/80/100 % of the spend cap), so each `usage.threshold` webhook fires once.
--
-- Idempotent (IF NOT EXISTS). Apply by hand to prod (ALLTERNIT_SKIP_MIGRATIONS=1).

ALTER TABLE public.platform_usage_events ADD COLUMN IF NOT EXISTS amount_microusd bigint;
ALTER TABLE public.platform_usage_events ADD COLUMN IF NOT EXISTS model text;
ALTER TABLE public.platform_usage_events ADD COLUMN IF NOT EXISTS stripe_reported_at timestamp with time zone;

CREATE INDEX IF NOT EXISTS idx_platform_usage_events_unreported
    ON public.platform_usage_events (created_at) WHERE stripe_reported_at IS NULL;

ALTER TABLE public.platform_projects ADD COLUMN IF NOT EXISTS stripe_customer_id text;

CREATE TABLE IF NOT EXISTS public.platform_spend_alerts (
    project_id text NOT NULL REFERENCES public.platform_projects (id),
    period text NOT NULL,
    percent integer NOT NULL CHECK (percent IN (50, 80, 100)),
    spend_microusd bigint NOT NULL,
    cap_cents bigint NOT NULL,
    created_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (project_id, period, percent)
);
