-- AllternitOS Stage 1, step 7: the authority kernel's own store (CONTRACT.md sections 3.1, 3.2, 5, 8).
-- Additive only. The nine existing approval stores keep their tables; plan step 11 makes them
-- write through `approval_requests`.

-- One row per ApprovalRequestV1. `owner_user_id` is the authenticated account that raised it
-- (the transport identity); `principal` is the actor in the body that wants to act.
CREATE TABLE IF NOT EXISTS approval_requests (
    request_id      TEXT PRIMARY KEY,
    owner_user_id   TEXT NOT NULL,
    caller_kind     TEXT NOT NULL CHECK (caller_kind IN ('run', 'human', 'service')),
    source_store    TEXT NOT NULL,
    source_ref      TEXT,
    run_id          TEXT NOT NULL,
    principal       TEXT NOT NULL,
    assignee        TEXT NOT NULL,
    action_class    TEXT NOT NULL,
    action_summary  TEXT NOT NULL CHECK (length(action_summary) BETWEEN 1 AND 280),
    -- Canonical (RFC 8785) JSON of the exact action. The UI shows this, never requester text.
    action_payload  TEXT NOT NULL,
    action_hash     TEXT NOT NULL CHECK (action_hash LIKE 'sha256:%' AND length(action_hash) = 71),
    deadline        TEXT NOT NULL,
    on_expiry       TEXT NOT NULL DEFAULT 'reject' CHECK (on_expiry = 'reject'),
    attention_id    TEXT NOT NULL,
    check_id        TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT 'open'
                    CHECK (status IN ('open', 'approved', 'denied', 'expired', 'withdrawn')),
    -- Token TTL asked for by the requester, capped at 900 s (CONTRACT.md 12.1).
    token_ttl_secs  INTEGER NOT NULL DEFAULT 300 CHECK (token_ttl_secs BETWEEN 1 AND 900),
    token_id        TEXT UNIQUE,
    created_at      TEXT NOT NULL,
    resolved_at     TEXT,
    resolved_by     TEXT,
    resolve_note    TEXT,
    evidence_refs   TEXT,
    extensions      TEXT
);

CREATE INDEX IF NOT EXISTS idx_approval_requests_owner_status
    ON approval_requests (owner_user_id, status);
CREATE INDEX IF NOT EXISTS idx_approval_requests_run
    ON approval_requests (run_id);

-- Single use: one row per spent token, primary key token_id. A second redeem fails on the key.
CREATE TABLE IF NOT EXISTS approval_token_spends (
    token_id          TEXT PRIMARY KEY,
    request_id        TEXT NOT NULL REFERENCES approval_requests (request_id),
    action_hash       TEXT NOT NULL CHECK (action_hash LIKE 'sha256:%' AND length(action_hash) = 71),
    redeemed_by       TEXT NOT NULL,
    spend_receipt_id  TEXT NOT NULL,
    spent_at          TEXT NOT NULL
);

CREATE TRIGGER IF NOT EXISTS approval_token_spends_no_update
BEFORE UPDATE ON approval_token_spends
BEGIN
    SELECT RAISE(ABORT, 'approval_token_spends is append-only');
END;

CREATE TRIGGER IF NOT EXISTS approval_token_spends_no_delete
BEFORE DELETE ON approval_token_spends
BEGIN
    SELECT RAISE(ABORT, 'approval_token_spends is append-only');
END;

-- One append-only hash chain per (principal, tenant).
-- Chain scope is (principal, tenant) (CONTRACT.md 11.2); run_id stays as a column.
-- hash = sha256 over canonical {prev_hash, kind, principal, tenant, payload_hash, decision, created_at, run_id, idx, domain, schema_id, schema_version};
-- key_id / signer_pub name the key that signed the row, so a rotated key reads "signed by a previous key", not "tampered";
-- prev_hash is `sha256:` + 64 zeros at idx 0. `kind` mirrors payload.kind (CONTRACT.md 11.2).
CREATE TABLE IF NOT EXISTS kernel_receipts (
    receipt_id       TEXT PRIMARY KEY,
    run_id           TEXT NOT NULL,
    principal        TEXT NOT NULL,
    tenant           TEXT NOT NULL,
    idx              INTEGER NOT NULL CHECK (idx >= 0),
    domain           TEXT NOT NULL,
    kind             TEXT NOT NULL,
    schema_id        TEXT NOT NULL,
    schema_version   TEXT NOT NULL,
    payload          TEXT NOT NULL,
    payload_hash     TEXT NOT NULL CHECK (payload_hash LIKE 'sha256:%' AND length(payload_hash) = 71),
    decision         TEXT NOT NULL,
    prev_hash        TEXT NOT NULL CHECK (prev_hash LIKE 'sha256:%' AND length(prev_hash) = 71),
    hash             TEXT NOT NULL UNIQUE CHECK (hash LIKE 'sha256:%' AND length(hash) = 71),
    signature        TEXT NOT NULL,
    key_id           TEXT NOT NULL,
    signer_pub       TEXT NOT NULL,
    idempotency_key  TEXT UNIQUE,
    created_at       TEXT NOT NULL,
    UNIQUE (principal, tenant, idx)
);

CREATE INDEX IF NOT EXISTS idx_kernel_receipts_kind
    ON kernel_receipts (kind, created_at);

CREATE TRIGGER IF NOT EXISTS kernel_receipts_no_update
BEFORE UPDATE ON kernel_receipts
BEGIN
    SELECT RAISE(ABORT, 'kernel_receipts is append-only');
END;

CREATE TRIGGER IF NOT EXISTS kernel_receipts_no_delete
BEFORE DELETE ON kernel_receipts
BEGIN
    SELECT RAISE(ABORT, 'kernel_receipts is append-only');
END;

-- Plan step 11a: the old approval stores write through approval_requests. Additive only.
-- A kernel row points back at its old row with (source_store, source_ref).
CREATE INDEX IF NOT EXISTS idx_approval_requests_source
    ON approval_requests (source_store, source_ref);

-- factory_approvals (V238) keeps the kernel request id; NULL for rows made before step 11a
-- (they get one lazily on their first resolve).
ALTER TABLE factory_approvals ADD COLUMN kernel_request_id TEXT;

-- Plan step 11a, second run: gateway_approvals (V199) keeps the kernel request id and, once a
-- person approved, the approve token (single use, bound to the request's action hash, spent by
-- the held send / phone hold / vendor send). NULL for rows made before step 11a. cowork_approvals
-- and task_grants keep no column: their kernel rows are found by (source_store, source_ref).
ALTER TABLE gateway_approvals ADD COLUMN kernel_request_id TEXT;
ALTER TABLE gateway_approvals ADD COLUMN kernel_token TEXT;
