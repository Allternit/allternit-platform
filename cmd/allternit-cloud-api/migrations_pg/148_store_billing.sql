-- Store purchases: Apple In-App Purchase and Google Play Billing
-- (handoffs/store-purchases-2026-11-17/PLAN.md, D1, D2, D4, D13).
-- Additive only: old code keeps running on this schema.

-- D1: a store plan is a billing_subscriptions row keyed
-- `apple:<originalTransactionId>` or `google:<sha256 of purchase token>`, so
-- every reader of this table works unchanged. `provider` says who bills it;
-- existing rows are Stripe.
ALTER TABLE billing_subscriptions ADD COLUMN IF NOT EXISTS provider TEXT NOT NULL DEFAULT 'stripe';
-- The store product the plan is on now (it changes on an upgrade inside the
-- Apple subscription group or a Play replacement).
ALTER TABLE billing_subscriptions ADD COLUMN IF NOT EXISTS store_product_id TEXT;
-- `production` or `sandbox` for store rows; NULL for Stripe.
ALTER TABLE billing_subscriptions ADD COLUMN IF NOT EXISTS store_environment TEXT;
-- D13: when the server last re-read this store plan from the store (at most
-- once per 10 minutes per row, after its period end has passed).
ALTER TABLE billing_subscriptions ADD COLUMN IF NOT EXISTS store_checked_at TIMESTAMPTZ;

CREATE INDEX IF NOT EXISTS idx_billing_subscriptions_user_open
    ON billing_subscriptions (user_id)
    WHERE status IN ('active', 'trialing', 'past_due', 'unpaid');

-- D4: one store account token per user. iOS passes it as `appAccountToken`;
-- a transaction whose token maps to another user is refused. The org is the
-- one captured at the latest preflight (pack credits go to the fabric ledger
-- when that bridge is on, D6).
CREATE TABLE IF NOT EXISTS billing_store_accounts (
    user_id TEXT PRIMARY KEY,
    account_token UUID NOT NULL UNIQUE,
    organization_id TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- D2: the store ledger, one row per store transaction (a plan purchase, each
-- plan renewal, each pack). It is the idempotency key, the refund reference
-- and the Play claim record. Finance records: no FK to users, so an erased
-- account keeps its money trail.
CREATE TABLE IF NOT EXISTS billing_store_transactions (
    id BIGSERIAL PRIMARY KEY,
    provider TEXT NOT NULL CHECK (provider IN ('apple', 'google')),
    -- Apple transactionId, or the Play orderId.
    transaction_id TEXT NOT NULL,
    -- Apple originalTransactionId (the plan's first transaction).
    original_transaction_id TEXT,
    -- SHA-256 hex of the Play purchase token (the token itself is never stored).
    purchase_token_sha256 TEXT,
    -- NULL while a Play notification is unclaimed (D4).
    user_id TEXT,
    product_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('plan', 'pack')),
    -- Server catalog id: plus, super, ultra, credits_10, ...
    item_id TEXT NOT NULL,
    environment TEXT NOT NULL CHECK (environment IN ('production', 'sandbox')),
    -- pending, granted, unclaimed, sandbox_not_granted, refunded
    state TEXT NOT NULL DEFAULT 'pending',
    credits_granted_usd DOUBLE PRECISION NOT NULL DEFAULT 0,
    -- The wallet / fabric key the credits were granted under:
    -- apple-<transactionId> or google-<orderId>.
    grant_key TEXT,
    granted_at TIMESTAMPTZ,
    refunded_at TIMESTAMPTZ,
    refund_reversed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (provider, transaction_id)
);

CREATE INDEX IF NOT EXISTS idx_billing_store_transactions_user
    ON billing_store_transactions (user_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_billing_store_transactions_original
    ON billing_store_transactions (provider, original_transaction_id)
    WHERE original_transaction_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_billing_store_transactions_token
    ON billing_store_transactions (purchase_token_sha256)
    WHERE purchase_token_sha256 IS NOT NULL;
