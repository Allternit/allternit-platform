-- V283: Office view (`src/floor_routes.rs`). Owner-wide event reads filter on
-- type and time; the share row remembers whether the live floor link is on.
CREATE INDEX IF NOT EXISTS idx_bot_events_type_time ON bot_events(event_type, occurred_at);

CREATE TABLE IF NOT EXISTS floor_share (
    owner TEXT PRIMARY KEY,
    enabled INTEGER NOT NULL DEFAULT 0,
    token TEXT,
    -- A disable the cloud has not confirmed yet; the push loop retries it.
    revoke_pending INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL
);
