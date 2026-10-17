-- V264: MCP tool pinning for app bots (`src/mcp_server_routes.rs`).
--
-- One row per (owner, server, tool): the sha256 of the tool's name, description,
-- input schema and annotations as last approved. A tool whose hash no longer
-- matches, or that was never pinned, is held out of the bot's tool list until the
-- owner approves it. The first sight of a server pins what it lists (trust on first
-- use); only later changes and additions are held.
CREATE TABLE IF NOT EXISTS mcp_tool_pins (
    owner TEXT NOT NULL,
    server_id TEXT NOT NULL,
    tool TEXT NOT NULL,
    hash TEXT NOT NULL,
    approved_at TEXT NOT NULL,
    PRIMARY KEY (owner, server_id, tool)
);
