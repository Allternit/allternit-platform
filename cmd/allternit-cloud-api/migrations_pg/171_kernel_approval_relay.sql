-- 171_kernel_approval_relay.sql
--
-- AllternitOS Stage 1, step 18 (handoffs/allternit-os-2026-11-27, CONTRACT.md
-- sections 4.3, 5, 6.4 and 8; PLAN.md decisions 3-4).
--
-- 1. kernel_approval_relay: one row per kernel approval request that a
--    person's Desktop or cloud computer (allternit-api) raised for its owner
--    and sent up to cloud-api (POST /api/v1/kernel/relay/requests, runtime
--    signature). cloud-api pushes it to the owner's phones, lists it in the
--    phone's feed and forwards the phone's decision back to that runtime's
--    own resolve route. The runtime verifies the person and signs the token;
--    no signing key is held for it here. A row past `deadline` reads as
--    expired (the device rejects on expiry, `on_expiry` is always reject), and
--    the sweep marks it.
--
-- 2. kernel_receipts: the cloud-api kernel's own receipt log (CONTRACT.md 5):
--    the relay's receipts and the embedded kernel's `check` receipts for chats
--    with no computer. One hash chain per (principal, tenant), Ed25519 signed
--    with the cloud kernel key (ALLTERNIT_KERNEL_SEED, cloud-api secret env).
--    Append-only: UPDATE and DELETE raise.
--
-- Additive and idempotent. Applied before the binary swap by
-- deploy-contabo.sh through the ops_applied_migrations ledger (prod runs with
-- ALLTERNIT_SKIP_MIGRATIONS=1); never edit after it ships.

CREATE TABLE IF NOT EXISTS kernel_approval_relay (
    request_id    TEXT NOT NULL,
    runtime_id    TEXT NOT NULL,
    owner_user_id TEXT NOT NULL,
    -- The CONTRACT 6.4 `kernel` object exactly as the inbox row carries it:
    -- {requestId, runId, assignee, actionClass, actionSummary, actionHash, deadline, humanOnly}.
    kernel        JSONB NOT NULL,
    attention_id  TEXT,
    -- pending | approved | denied | expired | closed (decided on the device itself)
    status        TEXT NOT NULL DEFAULT 'pending',
    deadline      TIMESTAMPTZ NOT NULL,
    pushed_at     TIMESTAMPTZ,
    resolved_at   TIMESTAMPTZ,
    resolved_by   TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (runtime_id, request_id)
);

CREATE INDEX IF NOT EXISTS kernel_approval_relay_owner_pending_idx
    ON kernel_approval_relay (owner_user_id, deadline)
    WHERE status = 'pending';

CREATE TABLE IF NOT EXISTS kernel_receipts (
    receipt_id      TEXT PRIMARY KEY,
    run_id          TEXT NOT NULL,
    principal       TEXT NOT NULL,
    tenant          TEXT NOT NULL DEFAULT 'default',
    idx             BIGINT NOT NULL,
    domain          TEXT NOT NULL,
    kind            TEXT NOT NULL,
    schema_id       TEXT NOT NULL,
    schema_version  TEXT NOT NULL,
    -- Canonical JSON text (sorted keys), the exact bytes that were hashed.
    payload         TEXT NOT NULL,
    decision        TEXT NOT NULL,
    prev_hash       TEXT NOT NULL,
    hash            TEXT NOT NULL,
    signature       TEXT NOT NULL,
    key_id          TEXT NOT NULL,
    signer_pub      TEXT NOT NULL,
    idempotency_key TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (principal, tenant, idx)
);

CREATE UNIQUE INDEX IF NOT EXISTS kernel_receipts_idempotency_idx
    ON kernel_receipts (idempotency_key)
    WHERE idempotency_key IS NOT NULL;

CREATE OR REPLACE FUNCTION kernel_receipts_append_only() RETURNS trigger AS $$
BEGIN
    RAISE EXCEPTION 'kernel_receipts is append-only';
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS kernel_receipts_no_update ON kernel_receipts;
CREATE TRIGGER kernel_receipts_no_update BEFORE UPDATE ON kernel_receipts
    FOR EACH ROW EXECUTE FUNCTION kernel_receipts_append_only();

DROP TRIGGER IF EXISTS kernel_receipts_no_delete ON kernel_receipts;
CREATE TRIGGER kernel_receipts_no_delete BEFORE DELETE ON kernel_receipts
    FOR EACH ROW EXECUTE FUNCTION kernel_receipts_append_only();
