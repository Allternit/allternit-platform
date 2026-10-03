-- V220: the vendor-bot MCP connector (`mcp.allternit.com/mcp/bots/<vendorBotId>`).
--
-- A vendor bot (Claude, ChatGPT, Grok ... bound to an Allternit bot through the
-- Agent Gateway) shares its directing bot's phone and acts through tools. These
-- tables hold which bot directs it, which threads the directing bot shared with
-- it, the OAuth clients that connected, and an audit row per tool call.

-- Which bot directs a vendor bot (whose phone and mailbox it uses).
CREATE TABLE IF NOT EXISTS vendor_bot_connectors (
    vendor_bot_id    TEXT PRIMARY KEY,
    owner            TEXT NOT NULL,
    directing_bot_id TEXT,
    created_at       TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at       TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_vendor_bot_connectors_owner ON vendor_bot_connectors(owner);

-- Threads the directing bot shared with the vendor bot ("their view"), beyond
-- the threads the vendor bot is itself attached to.
CREATE TABLE IF NOT EXISTS vendor_bot_thread_shares (
    vendor_bot_id TEXT NOT NULL,
    thread_id     TEXT NOT NULL,
    owner         TEXT NOT NULL,
    created_at    TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (vendor_bot_id, thread_id)
);

-- OAuth clients (a Claude or ChatGPT connector) that used the connector.
-- `client` is the OAuth client id from the access token; revoking sets
-- revoked_at and every later call from that client is refused.
CREATE TABLE IF NOT EXISTS vendor_connector_clients (
    id            TEXT PRIMARY KEY,
    vendor_bot_id TEXT NOT NULL,
    owner         TEXT NOT NULL,
    client        TEXT NOT NULL,
    first_seen_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    last_used_at  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    revoked_at    TEXT,
    UNIQUE (vendor_bot_id, client)
);
CREATE INDEX IF NOT EXISTS idx_vendor_connector_clients_bot ON vendor_connector_clients(vendor_bot_id);

-- One row per tool call: who, which tool, a hash of the arguments (never the
-- arguments themselves) and what happened.
CREATE TABLE IF NOT EXISTS vendor_bot_audit (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    owner            TEXT NOT NULL,
    vendor_bot_id    TEXT NOT NULL,
    directing_bot_id TEXT,
    client           TEXT,
    tool             TEXT NOT NULL,
    args_hash        TEXT NOT NULL,
    ok               INTEGER NOT NULL,
    result           TEXT NOT NULL DEFAULT '',
    created_at       TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_vendor_bot_audit_bot ON vendor_bot_audit(vendor_bot_id, id);
