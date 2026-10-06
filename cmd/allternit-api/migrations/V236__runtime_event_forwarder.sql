-- V236: runtime → cloud event forwarder (`src/runtime_events.rs`).
--
-- `runtime_user_events` is the owner-level ledger for events that have no bot
-- (Subscriptions logins, approvals from the chat permission store). It sits
-- beside `bot_events` and is read by the same forwarder and the same
-- `events_feed::classify`. `seq` is an INTEGER PRIMARY KEY so the forwarder's
-- cursor is stable; `idempotency_key` dedupes repeat writes per owner.
CREATE TABLE IF NOT EXISTS runtime_user_events (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    user_id TEXT NOT NULL,
    event_type TEXT NOT NULL,
    thread_id TEXT,
    payload TEXT NOT NULL,
    idempotency_key TEXT,
    occurred_at TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(user_id, idempotency_key)
);
CREATE INDEX IF NOT EXISTS idx_runtime_user_events_user ON runtime_user_events(user_id, seq);

-- Outbox cursors: the last ledger row forwarded to the cloud, per cloud
-- account (the owner this runtime is paired as) and per source table
-- (`bot_events` rowid, `runtime_user_events` seq). Advanced only after the
-- cloud answered 2xx, so a restart resumes where delivery stopped.
CREATE TABLE IF NOT EXISTS cloud_event_cursors (
    account_id TEXT NOT NULL,
    source TEXT NOT NULL,
    last_rowid INTEGER NOT NULL,
    updated_at TEXT NOT NULL,
    last_error TEXT,
    PRIMARY KEY (account_id, source)
);

-- Last seen health of each Subscriptions login (`subscription_sync.rs`), so
-- a change becomes one `subscription.login_needed` / `subscription.signed_in`
-- ledger entry instead of the UI polling `session_health`.
CREATE TABLE IF NOT EXISTS subscription_login_health (
    owner TEXT NOT NULL,
    login_id TEXT NOT NULL,
    provider TEXT,
    health TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (owner, login_id)
);
