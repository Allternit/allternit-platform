-- P5 MCP App commerce (Stripe TEST MODE ONLY). Written, not applied to any
-- shared or prod DB: it runs when the API next starts against a DB.
-- All amounts are minor units (cents); currency is a lowercase ISO-4217 code.

-- One Stripe Connect Express account per MCP App developer app.
CREATE TABLE IF NOT EXISTS commerce_connected_accounts (
    app_id             TEXT PRIMARY KEY,
    owner_user_id      TEXT NOT NULL,
    stripe_account_id  TEXT NOT NULL UNIQUE,
    details_submitted  INTEGER NOT NULL DEFAULT 0,
    charges_enabled    INTEGER NOT NULL DEFAULT 0,
    payouts_enabled    INTEGER NOT NULL DEFAULT 0,
    created_at         TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at         TEXT NOT NULL DEFAULT (datetime('now'))
);

-- The host's own validated copy of an app's checkout session. Totals here are
-- recomputed by the host; nothing the View sends is stored as authoritative.
CREATE TABLE IF NOT EXISTS commerce_checkout_sessions (
    id                 TEXT PRIMARY KEY,       -- the app's checkout_session_id
    app_id             TEXT NOT NULL,
    buyer_user_id      TEXT NOT NULL,
    merchant_account   TEXT NOT NULL,          -- Stripe acct_...
    currency           TEXT NOT NULL,
    subtotal_minor     INTEGER NOT NULL,
    tax_minor          INTEGER NOT NULL,
    total_minor        INTEGER NOT NULL,
    application_fee_minor INTEGER NOT NULL,
    session_json       TEXT NOT NULL,
    status             TEXT NOT NULL DEFAULT 'open',  -- open | paid | expired
    payment_intent_id  TEXT,
    created_at         TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (app_id, id)
);

-- Orders ledger: one row per paid checkout session.
CREATE TABLE IF NOT EXISTS commerce_orders (
    id                     TEXT PRIMARY KEY,
    checkout_session_id    TEXT NOT NULL UNIQUE,
    app_id                 TEXT NOT NULL,
    buyer_user_id          TEXT NOT NULL,
    merchant_account       TEXT NOT NULL,
    payment_intent_id      TEXT NOT NULL UNIQUE,
    currency               TEXT NOT NULL,
    total_minor            INTEGER NOT NULL,
    application_fee_minor  INTEGER NOT NULL,
    -- paid: charged, app not yet told | fulfilled: app's complete_checkout ok
    -- fulfillment_failed: charged, app call failed (retryable) | refunded | disputed
    status                 TEXT NOT NULL DEFAULT 'paid',
    app_result_json        TEXT,
    fulfillment_error      TEXT,
    refund_id              TEXT,
    refunded_by            TEXT,
    created_at             TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at             TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_commerce_orders_buyer ON commerce_orders (buyer_user_id);
CREATE INDEX IF NOT EXISTS idx_commerce_orders_app ON commerce_orders (app_id);

-- Dispute webhook log (logging only; no automatic action).
CREATE TABLE IF NOT EXISTS commerce_disputes (
    id                 TEXT PRIMARY KEY,       -- Stripe dispute id
    payment_intent_id  TEXT,
    charge_id          TEXT,
    amount_minor       INTEGER,
    currency           TEXT,
    reason             TEXT,
    status             TEXT,
    last_event_id      TEXT NOT NULL,
    raw_json           TEXT NOT NULL,
    created_at         TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at         TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Webhook replay guard: a Stripe event id is processed at most once.
CREATE TABLE IF NOT EXISTS commerce_webhook_events (
    event_id     TEXT PRIMARY KEY,
    event_type   TEXT NOT NULL,
    received_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
