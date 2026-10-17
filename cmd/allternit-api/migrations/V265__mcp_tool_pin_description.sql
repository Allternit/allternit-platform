-- V265: remember the approved description of each pinned MCP tool so the inbox can
-- show "what changed" (old vs new) when a tool is held (`src/mcp_server_routes.rs`).
ALTER TABLE mcp_tool_pins ADD COLUMN description TEXT;
