-- The member card's "Renews" date: Stripe's current_period_end, mirrored on
-- every customer.subscription.created/updated event. NULL until the next event
-- for subscriptions that existed before this column.
ALTER TABLE billing_subscriptions ADD COLUMN IF NOT EXISTS current_period_end TIMESTAMPTZ;
