-- 084_platform_payment_methods.sql
--
-- Platform API launch (Eoj 2026-10-08): card required, no free usage.
--
-- * `platform_projects.stripe_subscription_id`, `billing_status`,
--   `payment_method_added_at`: set by the Stripe webhook when the project's
--   Checkout (Pay as you go or Growth) completes. `billing_status` is NULL
--   until then; `active` / `past_due` (Stripe still retrying) allow billable
--   work, `unpaid` / `canceled` refuse it with 402 payment_method_required.
--   `stripe_customer_id` (080) is set at the same time.
-- * `api_keys.terms_accepted_at`, `terms_version`: when the console user who
--   created a project key accepted the developer terms and AUP, and which
--   version.
-- * `platform_credit_grants`: the Growth plan's $300 monthly usage credit, one
--   Stripe credit grant per paid subscription invoice (the invoice id dedupes).
--
-- Idempotent (IF NOT EXISTS). Apply by hand to prod (ALLTERNIT_SKIP_MIGRATIONS=1).

ALTER TABLE public.platform_projects ADD COLUMN IF NOT EXISTS stripe_subscription_id text;
ALTER TABLE public.platform_projects ADD COLUMN IF NOT EXISTS billing_status text;
ALTER TABLE public.platform_projects ADD COLUMN IF NOT EXISTS payment_method_added_at timestamp with time zone;

CREATE INDEX IF NOT EXISTS idx_platform_projects_stripe_subscription
    ON public.platform_projects (stripe_subscription_id) WHERE stripe_subscription_id IS NOT NULL;

ALTER TABLE public.api_keys ADD COLUMN IF NOT EXISTS terms_accepted_at timestamp with time zone;
ALTER TABLE public.api_keys ADD COLUMN IF NOT EXISTS terms_version text;

CREATE TABLE IF NOT EXISTS public.platform_credit_grants (
    invoice_id text NOT NULL PRIMARY KEY,
    project_id text NOT NULL REFERENCES public.platform_projects (id),
    stripe_customer_id text NOT NULL,
    stripe_credit_grant_id text NOT NULL,
    amount_cents bigint NOT NULL,
    expires_at timestamp with time zone,
    created_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP
);
