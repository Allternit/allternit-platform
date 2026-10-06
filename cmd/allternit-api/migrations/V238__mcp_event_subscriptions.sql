-- V237: MCP Events client subscriptions (`src/mcp_events_client.rs`).
--
-- Allternit subscribes, as an MCP Events client, to events from the external
-- MCP servers a user connected (`mcp_connectors`). An event wakes the bot the
-- user chose (a Rails ticket, as an inbound webhook trigger does).
--
-- id            = mcp_protocol::events::subscription_id(owner, connector url,
--                 name, arguments): subscribing again with the same identity
--                 updates this row instead of adding one.
-- secret        = the `whsec_` secret given to the server (sealed with
--                 token_crypto). Rotated on every refresh.
-- status        = pending | active | error | needs_reauth | ended
--                 (ended = removed here, cloud removal still to confirm).
-- error_code    = the server's -32011..-32015 code when it refused.
-- had_auth      = the connector had a credential when subscribed: losing it
--                 later means the OAuth grant was revoked.
CREATE TABLE IF NOT EXISTS mcp_event_subscriptions (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    connector_id TEXT NOT NULL,
    name TEXT NOT NULL,
    arguments TEXT NOT NULL DEFAULT '{}',
    bot_id TEXT NOT NULL,
    execution_mode TEXT NOT NULL DEFAULT 'REQUIRE_APPROVAL',
    secret TEXT NOT NULL,
    callback_url TEXT,
    status TEXT NOT NULL DEFAULT 'pending',
    error_code INTEGER,
    error_reason TEXT,
    error_message TEXT,
    had_auth INTEGER NOT NULL DEFAULT 0,
    refresh_before TEXT,
    cursor TEXT,
    last_event_at TEXT,
    last_event_id TEXT,
    event_count INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_mcp_event_subscriptions_connector ON mcp_event_subscriptions(user_id, connector_id);
CREATE INDEX IF NOT EXISTS idx_mcp_event_subscriptions_status ON mcp_event_subscriptions(status, refresh_before);
