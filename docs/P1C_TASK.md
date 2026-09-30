# P1c — make a user's MCP connectors actually run in chat (platform repo; branch is based on p1-mcp-apps-host-backend)
Read docs/MCP_APPS_HOST_PHASE_1A_NOTES.md (esp. §1 trace and §5 "Not done") first.
Problem: `mcp_connectors` (allternit-api) are never registered with gizzi-code, where chat tools execute, so connector tools
and `mcp_app` emission never happen in a real chat turn.

## Design (decided): per-user MCP proxy in allternit-api — connector tokens never leave allternit-api
1. `POST /mcp/user-proxy` (streamable HTTP MCP server) in allternit-api. Auth: a short-lived (≤15 min), single-session,
   HMAC-signed proxy token bound to {user_id, session_id}; NOT the user's Clerk token. `tools/list` = union of the user's
   enabled connectors' tools, namespaced `<connector_name_id>__<tool>` (keep each tool's `_meta`, annotations, schemas; drop
   tools whose `_meta.ui.visibility` excludes "model"). `tools/call` routes to the right connector with its stored credentials
   via the existing streamable-HTTP client. Pass through `resources/read` for `ui://` URIs.
2. When allternit-api starts/relays a chat turn to gizzi (v1_routes.rs agent_chat_bridge), mint the proxy token and register
   ONE MCP server entry for that gizzi session pointing at the proxy (use gizzi's existing per-session/per-request MCP config
   mechanism; find it — do not write user tokens into project-level gizzi config or any file on disk). Make `mcp_app`
   emission resolve the namespaced tool back to {connectorId, tool}.
3. gizzi-code `runtime/server/routes/agent-compat.ts` mirror (NOTES §5.2): emit `mcp_app` the same way when the proxy entry is present.
4. OAuth refresh (§5.4): use stored refresh_token/expires_in before calls; on refresh failure return `connector_unauthorized`.
5. Legacy SSE-only servers (§5.3): fall back to the existing SSE transport when streamable HTTP initialize gets 404/405.
6. `mcp_routes.rs` `mcp_oauth_callback`: send `redirect_uri` in the token exchange (RFC 6749 §4.1.3), and encrypt tokens at rest
   in `mcp_oauth_sessions.tokens` / connector credentials with the existing secret-encryption mechanism (find it; migration
   written, NOT applied; read path must accept old plaintext rows).
7. SSRF: connect to the IP that was validated (pin the resolved address) to close the DNS-rebinding race noted in §5.5.

## Tests
Proxy token (expiry, wrong session, tamper), namespacing round-trip, visibility filtering, cross-user isolation, refresh path,
SSE fallback, encrypted-token round-trip + plaintext back-compat, end-to-end: fake connector server → proxy → tools/call →
`mcp_app` frame. `cargo check -p allternit-api` + relevant tests; gizzi `bun test` for touched files.

## Rules
- Only this worktree/branch; commit there. No push/merge/deploy/prod migrations. Never log tokens.
- Time-box ~90 min; smallest complete tested version; list cuts.
- Deliverable docs/P1C_NOTES.md (files, commands + results, cuts). Last line exactly: `status: done`
