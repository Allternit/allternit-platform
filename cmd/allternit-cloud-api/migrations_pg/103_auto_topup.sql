-- 103_auto_topup.sql
--
-- Automatic top-up of the cloud wallet (`services/auto_topup.rs`,
-- `routes/billing_auto_topup.rs`).
--
--   * billing_auto_topup: one row per person with auto top-up settings or a
--     saved card. When enabled and the wallet balance drops under
--     threshold_usd after a debit, cloud-api charges the saved card
--     off-session for amount_usd (one of the credit pack sizes). status:
--     'ok' (idle), 'pending' (a charge is in flight; no second one starts),
--     'failed' (the last charge failed; enabled is turned off), 'disabled'
--     (turned off by the person). month_spent_usd counts successful top-ups in
--     month_key ('YYYY-MM', UTC) against monthly_cap_usd.
--     stripe_payment_method_id / card_brand / card_last4 come from a setup-mode
--     Checkout, a credit pack bought with setup_future_usage, or the Stripe
--     customer's saved cards.
--   * billing_auto_topup_attempts: one row per off-session charge. Created
--     'pending' before the PaymentIntent (payment_intent_id is filled in when
--     Stripe answers), then 'succeeded' or 'failed' from the API answer or the
--     payment_intent.* webhook. The partial unique index allows only one
--     pending attempt per person.
--   * customer_emails.kind gains 'auto_topup_failed' (ref = attempt id).
--
-- Amounts are numeric(12,2); queries cast to float8 for the f64 code paths.
-- Additive only, idempotent (safe to re-run).

CREATE TABLE IF NOT EXISTS public.billing_auto_topup (
    user_id text PRIMARY KEY,
    enabled boolean NOT NULL DEFAULT false,
    threshold_usd numeric(12,2) NOT NULL DEFAULT 5,
    amount_usd numeric(12,2) NOT NULL DEFAULT 25,
    monthly_cap_usd numeric(12,2) NOT NULL DEFAULT 200,
    stripe_customer_id text,
    stripe_payment_method_id text,
    card_brand text,
    card_last4 text,
    status text NOT NULL DEFAULT 'ok' CHECK (status IN ('ok', 'pending', 'failed', 'disabled')),
    last_attempt_at timestamptz,
    last_error text,
    month_spent_usd numeric(12,2) NOT NULL DEFAULT 0,
    month_key text NOT NULL DEFAULT '',
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS public.billing_auto_topup_attempts (
    id text PRIMARY KEY,
    user_id text NOT NULL,
    payment_intent_id text UNIQUE,
    amount_usd numeric(12,2) NOT NULL,
    status text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'succeeded', 'failed')),
    error text,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_billing_auto_topup_attempts_user
    ON public.billing_auto_topup_attempts (user_id, created_at DESC);
CREATE UNIQUE INDEX IF NOT EXISTS uq_billing_auto_topup_attempts_one_pending
    ON public.billing_auto_topup_attempts (user_id) WHERE status = 'pending';

ALTER TABLE public.customer_emails DROP CONSTRAINT IF EXISTS customer_emails_kind_check;
ALTER TABLE public.customer_emails ADD CONSTRAINT customer_emails_kind_check
    CHECK (kind IN ('welcome', 'plan_started', 'computer_ready', 'auto_topup_failed'));
