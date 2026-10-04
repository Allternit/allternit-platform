-- Owner approval of one OAuth client for one MCP target (a vendor bot, or the agents server).
-- Clerk can't issue our own scopes, so the edge accepts a Clerk token (scope `profile`) only
-- when the owner approved that token's client (azp / client_id) here. (42 is user_files.)
CREATE TABLE IF NOT EXISTS mcp_oauth_approvals (
    id          TEXT PRIMARY KEY,
    user_id     TEXT NOT NULL,
    client_id   TEXT NOT NULL,
    target      TEXT NOT NULL,            -- 'bot:<vendorBotId>' or 'agents'
    label       TEXT NOT NULL DEFAULT '',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ,
    revoked_at  TIMESTAMPTZ
);
CREATE UNIQUE INDEX IF NOT EXISTS mcp_oauth_approvals_live ON mcp_oauth_approvals (user_id, client_id, target) WHERE revoked_at IS NULL;
