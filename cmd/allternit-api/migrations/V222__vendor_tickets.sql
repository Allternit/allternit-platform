-- V222: vendor task tickets, the directed-by relation, thread attribution and
-- the one-click local connector.
--
-- A ticket is a task a directing bot hands a vendor bot. The structured part
-- (instructions, allowed tools, the result) travels through the vendor-bot MCP
-- connector (`get_ticket`, `post_result`); the vendor's chat only gets a
-- one-line nudge on the lanes that have a connector.

CREATE TABLE IF NOT EXISTS vendor_tickets (
    owner            TEXT NOT NULL,
    id               TEXT NOT NULL,            -- "T-<n>", n counts per owner
    n                INTEGER NOT NULL,
    directing_bot_id TEXT,
    vendor_bot_id    TEXT NOT NULL,
    thread_id        TEXT NOT NULL,
    instructions     TEXT NOT NULL,
    allowed_tools    TEXT NOT NULL DEFAULT '[]',
    -- open -> sent -> done | expired | failed | cancelled
    status           TEXT NOT NULL DEFAULT 'open',
    lane             TEXT,                     -- local_app | website_connector | website_only | api_key
    result_json      TEXT,
    result_via       TEXT,                     -- connector | reply
    pending_reply    TEXT,                     -- the vendor's reply, kept for the deadline fallback
    error            TEXT,
    deadline_at      TEXT NOT NULL,
    created_at       TEXT NOT NULL,
    updated_at       TEXT NOT NULL,
    completed_at     TEXT,
    PRIMARY KEY (owner, id),
    UNIQUE (owner, n)
);
CREATE INDEX IF NOT EXISTS idx_vendor_tickets_bot ON vendor_tickets(owner, vendor_bot_id, status);

-- Which bot directs a vendor bot, set when it is deployed.
ALTER TABLE bot_execution_bindings ADD COLUMN directing_bot_id TEXT;

-- Which vendor bots took part in a thread (from tickets and connector tool calls).
CREATE TABLE IF NOT EXISTS vendor_bot_threads (
    owner         TEXT NOT NULL,
    vendor_bot_id TEXT NOT NULL,
    thread_id     TEXT NOT NULL,
    source        TEXT NOT NULL,               -- ticket | tool:<name>
    first_at      TEXT NOT NULL,
    last_at       TEXT NOT NULL,
    PRIMARY KEY (vendor_bot_id, thread_id)
);
CREATE INDEX IF NOT EXISTS idx_vendor_bot_threads_thread ON vendor_bot_threads(thread_id);

-- A vendor app on this computer that Allternit's local MCP server was written into.
CREATE TABLE IF NOT EXISTS vendor_local_connectors (
    owner         TEXT NOT NULL,
    app           TEXT NOT NULL,               -- claude_desktop | claude_code | codex | gemini_cli | hermes
    vendor_bot_id TEXT NOT NULL,
    server_name   TEXT NOT NULL,
    state         TEXT NOT NULL,               -- connected | unhealthy | removed
    config_path   TEXT,
    backup_path   TEXT,
    key_id        TEXT,                        -- the scoped access_tokens row
    error         TEXT,
    connected_at  TEXT NOT NULL,
    checked_at    TEXT,
    PRIMARY KEY (owner, app, vendor_bot_id)
);
