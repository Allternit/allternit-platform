-- 106_billing_disputes.sql
--
-- Chargebacks (Stripe `charge.dispute.created` / `charge.dispute.closed`,
-- routes/billing_webhooks.rs + services/billing_disputes.rs).
--
--   * billing_disputes: one row per Stripe dispute. user_id is NULL when the
--     disputed charge could not be traced to a person (recorded so the team
--     can look, and so Stripe gets a 200 instead of retrying forever).
--     held_usd is what was taken out of the cloud wallet when the dispute
--     opened (never more than the charge); hold_transaction_id /
--     release_transaction_id are the credit_transactions keys
--     (`stripe-{event_id}`) of the hold and of the give-back after a win.
--
-- The hold is a negative credit_transactions row with source 'dispute_hold'
-- and the give-back a positive row with source 'dispute_release'. The wallet
-- balance may go negative while a dispute is open so the credits cannot be
-- spent twice; user_credits.balance_usd has no CHECK (>= 0) constraint.
--
-- Additive only, idempotent (safe to re-run).

CREATE TABLE IF NOT EXISTS public.billing_disputes (
    stripe_dispute_id text PRIMARY KEY,
    user_id text,
    stripe_charge_id text,
    stripe_payment_intent_id text,
    stripe_customer_id text,
    amount_cents bigint NOT NULL DEFAULT 0,
    currency text,
    held_usd double precision NOT NULL DEFAULT 0,
    hold_transaction_id text,
    release_transaction_id text,
    status text NOT NULL DEFAULT 'open'
        CHECK (status IN ('open', 'unmatched', 'won', 'lost', 'closed')),
    stripe_status text,
    reason text,
    opened_event_id text,
    closed_event_id text,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_billing_disputes_user
    ON public.billing_disputes (user_id, created_at DESC);
