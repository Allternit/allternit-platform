-- 021_customer_emails.sql
--
-- Customer lifecycle emails and the sign-up / paid-customer record behind the
-- admin Customers page and the daily summary.
--
-- One row per email we owe a customer: kind 'welcome' (Clerk user.created),
-- 'plan_started' (first active grant for a Stripe subscription, ref = the
-- subscription id) or 'computer_ready' (the paid cloud computer paired for the
-- first time, ref = the instance id). UNIQUE (user_id, kind, ref) makes every
-- trigger idempotent across webhook redeliveries.
--
-- status: 'sent', 'failed' (send error, see error), or 'skipped' (existed
-- before these emails were turned on; never sent).

CREATE TABLE IF NOT EXISTS public.customer_emails (
    id bigserial PRIMARY KEY,
    user_id text NOT NULL,
    kind text NOT NULL CHECK (kind IN ('welcome', 'plan_started', 'computer_ready')),
    ref text NOT NULL DEFAULT '',
    email text,
    status text NOT NULL CHECK (status IN ('sent', 'failed', 'skipped')),
    error text,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (user_id, kind, ref)
);
CREATE INDEX IF NOT EXISTS idx_customer_emails_created ON public.customer_emails (created_at);

-- Only people from now on get these emails (Eoj, 2026-10-01): mark what
-- already exists as handled.
INSERT INTO public.customer_emails (user_id, kind, ref, status)
SELECT user_id, 'plan_started', stripe_subscription_id, 'skipped'
FROM public.billing_subscriptions
ON CONFLICT (user_id, kind, ref) DO NOTHING;

INSERT INTO public.customer_emails (user_id, kind, ref, status)
SELECT user_id, 'computer_ready', id, 'skipped'
FROM public.provisioned_instances
WHERE device_id IS NOT NULL
ON CONFLICT (user_id, kind, ref) DO NOTHING;
