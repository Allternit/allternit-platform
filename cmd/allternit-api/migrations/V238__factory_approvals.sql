-- Allternit Factory approvals (SPEC §9 "Approvals"). When a Factory node waits
-- on a person (a manual wait-gate, or a judge NEEDS_HUMAN verdict) the owner
-- can approve or reject it from the app, a push notification, or a channel
-- bound to their own account. The decision itself is a Gate event in the work
-- ledger; these rows only track the request, where it was sent, and who
-- answered first.

-- One request per waiting node: idempotent on (dag_id, node_id, gate), where
-- gate is `wait:<gate_id>` or `judge:<verdict ts>`.
CREATE TABLE IF NOT EXISTS factory_approvals (
    id              TEXT PRIMARY KEY,
    owner           TEXT NOT NULL,
    dag_id          TEXT NOT NULL,
    node_id         TEXT NOT NULL,
    gate            TEXT NOT NULL,
    bot_id          TEXT,
    title           TEXT NOT NULL,
    summary         TEXT NOT NULL DEFAULT '',
    evidence_ref    TEXT,
    risk            TEXT NOT NULL DEFAULT 'normal' CHECK (risk IN ('normal', 'high')),
    risk_reason     TEXT,
    surfaces_json   TEXT NOT NULL DEFAULT '["app"]',
    state           TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'approved', 'rejected', 'expired')),
    resolved_by     TEXT,
    resolved_via    TEXT,
    resolved_at     TEXT,
    provenance_json TEXT,
    note            TEXT,
    expires_at      TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    UNIQUE (dag_id, node_id, gate)
);
CREATE INDEX IF NOT EXISTS idx_factory_approvals_owner ON factory_approvals(owner, state, created_at);

-- One-time reply codes, one per approval and channel. Only a hash of the code
-- is stored. A code is single use, expires, and is burned after too many
-- wrong attempts.
CREATE TABLE IF NOT EXISTS factory_approval_codes (
    approval_id TEXT NOT NULL,
    channel     TEXT NOT NULL,
    account_id  TEXT,
    code_hash   TEXT NOT NULL,
    attempts    INTEGER NOT NULL DEFAULT 0,
    expires_at  TEXT NOT NULL,
    used_at     TEXT,
    created_at  TEXT NOT NULL,
    PRIMARY KEY (approval_id, channel)
);

-- The request messages that went out, so they can be edited (or followed up)
-- to say who answered and where once the approval is resolved.
CREATE TABLE IF NOT EXISTS factory_approval_messages (
    approval_id TEXT NOT NULL,
    channel     TEXT NOT NULL,
    account_id  TEXT,
    chat_id     TEXT,
    message_id  TEXT,
    state       TEXT NOT NULL DEFAULT 'sent',
    detail      TEXT,
    created_at  TEXT NOT NULL,
    PRIMARY KEY (approval_id, channel)
);

-- Every refused approval answer (wrong sender, forwarded, bad or used code,
-- expired, high risk on a channel, not the owner) with the reason.
CREATE TABLE IF NOT EXISTS factory_approval_refusals (
    id          TEXT PRIMARY KEY,
    approval_id TEXT,
    channel     TEXT NOT NULL,
    account_id  TEXT,
    sender      TEXT,
    message_id  TEXT,
    reason      TEXT NOT NULL,
    created_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_factory_approval_refusals_approval ON factory_approval_refusals(approval_id);

-- The owner's own verified identity on a connected channel account, so an
-- approval reply is accepted only from the owner. Telegram uses the managed
-- pairing (`provider_account_bindings.tg_owner_user_id`) and email the
-- account email; Slack and SMS (and Telegram bots connected by token) are
-- verified here: the owner gets a code in the app and sends `verify <code>`
-- from their own Slack user / phone to the bot.
CREATE TABLE IF NOT EXISTS factory_owner_identities (
    account_id  TEXT NOT NULL,
    channel     TEXT NOT NULL,
    owner       TEXT NOT NULL,
    identity    TEXT NOT NULL,
    verified_at TEXT NOT NULL,
    PRIMARY KEY (account_id, channel)
);
CREATE INDEX IF NOT EXISTS idx_factory_owner_identities_owner ON factory_owner_identities(owner);
