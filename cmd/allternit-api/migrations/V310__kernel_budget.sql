-- AllternitOS Stage 1, step 12: budget reservations (CONTRACT.md 2.6, 3.4).
-- The limits stay where they are (agents.config.spendLimit.monthlyUsd, spend_limits.rs); this table is
-- only the reserve -> commit/release ledger the kernel counts against them. Additive only.
CREATE TABLE IF NOT EXISTS kernel_budget_reservations (
    reservation_id    TEXT PRIMARY KEY,
    owner_user_id     TEXT NOT NULL,
    tenant            TEXT NOT NULL,
    run_id            TEXT NOT NULL,
    principal         TEXT NOT NULL,
    scope             TEXT NOT NULL CHECK (scope IN ('run', 'day', 'month')),
    currency          TEXT NOT NULL,
    -- Amounts in millionths of the currency unit (decimal strings are parsed, never floats).
    reserved_micros   INTEGER NOT NULL CHECK (reserved_micros >= 0),
    committed_micros  INTEGER,
    status            TEXT NOT NULL DEFAULT 'reserved' CHECK (status IN ('reserved', 'committed', 'released', 'expired')),
    expires_at        TEXT NOT NULL,
    created_at        TEXT NOT NULL,
    resolved_at       TEXT,
    placement_id      TEXT,
    resource_id       TEXT,
    usage             TEXT,
    receipt_id        TEXT NOT NULL,
    idempotency_key   TEXT
);
CREATE INDEX IF NOT EXISTS idx_kernel_budget_owner ON kernel_budget_reservations (owner_user_id, principal, currency, status, created_at);
