-- Subscription Fabric forwarder (SURFACES_PLAN §3 step 1; HARDENING D15/D16).
--
-- The gateway runs only on the user's Sessions computer (D15). allternit-api
-- reaches it through the computer's guest port and adds the gateway token
-- server-side, so neither gizzi nor the UI ever holds it.
CREATE TABLE IF NOT EXISTS subs_gateway_bindings (
    user_id      TEXT PRIMARY KEY,
    computer_id  TEXT NOT NULL,
    guest_port   INTEGER NOT NULL DEFAULT 7788,
    token_sealed TEXT NOT NULL,
    created_at   TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at   TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- D16 (1): provider-terms disclosure, acknowledged per user + provider +
-- disclosure version. A new version needs a new acknowledgement.
CREATE TABLE IF NOT EXISTS subs_disclosure_acks (
    user_id         TEXT NOT NULL,
    provider        TEXT NOT NULL,
    version         INTEGER NOT NULL,
    acknowledged_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (user_id, provider, version)
);

-- D16 (2): every fabric task starts from one human action (a send, an
-- approval-card confirm). An action id is minted at that surface, is
-- single-use, and expires; the forwarder consumes it when the task is
-- submitted and stamps it into the task's initiated_by. A retry of the same
-- submission (same idempotency_key) may reuse it.
CREATE TABLE IF NOT EXISTS subs_human_actions (
    action_id       TEXT PRIMARY KEY,
    user_id         TEXT NOT NULL,
    surface         TEXT NOT NULL,
    created_at      TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at      TEXT NOT NULL,
    consumed_at     TEXT,
    idempotency_key TEXT
);
CREATE INDEX IF NOT EXISTS idx_subs_human_actions_user ON subs_human_actions(user_id);
