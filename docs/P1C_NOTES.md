# P1c — per-user MCP connector proxy (notes)

## What was built
- `cmd/allternit-api/src/mcp_user_proxy.rs` (new): `POST /mcp/user-proxy` streamable-HTTP MCP server. HMAC-signed proxy token
  (≤15 min, bound to {user_id, session_id}, checked against `X-Allternit-Session`); `tools/list` = union of the user's connectors,
  namespaced `<connector_name_id>__<tool>`, `_meta`/annotations/schemas kept, app-only tools dropped; `tools/call` routes with stored
  credentials; `resources/read` serves only `ui://` resources a tool declared. `proxy_registration()` mints the per-turn entry.
- `v1_routes.rs` `agent_chat_bridge`: adds the entry as top-level `mcpProxy` on the gizzi message payload (not `metadata`, so it is
  never persisted). Nothing is written to gizzi project config or disk.
- `mcp_apps.rs`: namespaced tool → {connector, tool} resolution for `mcp_app` emission; OAuth refresh before calls
  (`connector_unauthorized` on failure); legacy SSE fallback on 404/405 initialize; SSRF: validated IP is pinned into the
  reqwest/transport client.
- `mcp_routes.rs`: `redirect_uri` sent in the token exchange; tokens and client secrets sealed with `token_crypto`
  (`enc:v1:`), read path accepts legacy plaintext; `seal_legacy_mcp_secrets()` is the one-shot in-place migration (idempotent,
  NOT invoked/applied).
- `mcp/mcp-client`, `mcp/core`: transport pin + `_meta` support.
- gizzi-code: `tools/mcp/user-proxy.ts` (turn-scoped, in-memory registration, keyed by session, released at end of turn),
  `tools/mcp/index.ts` `toolCatalog(extraClients)` (proxy is listed but never recorded in shared MCP state),
  `tools/mcp/apps.ts` (`buildMcpAppFrame`), `session/prompt.ts`, `routes/agent-compat.ts` (mirror emits `mcp_app`).

## Commands and results
- `cargo check -p allternit-api` — ok (warnings pre-existing).
- `cargo test -p allternit-api mcp_` — all pass, incl. token expiry/wrong-session/tamper, namespacing round-trip, visibility
  filtering, cross-user isolation, refresh + failed refresh, SSE fallback, sealed/plaintext round-trip, e2e fake connector → proxy
  → tools/call → `mcp_app` frame, redirect_uri, SSRF pinning.
- `bun test test/mcp/user-proxy.test.ts` — 11 pass. `apps`, `bundled`, `catalog`, `oauth-browser` files pass individually.

## Known test-harness issues (not product bugs)
- `bun test test/mcp` as one process: `headers.test.ts` `mock.module`s the MCP SDK transports and leaks into `apps`/`user-proxy`
  tests (7 failures). Run per file. `headers.test.ts` fails on its own too: it receives the bundled allternit server's local token
  instead of its fixture; it does not touch code changed here (only `toolCatalog` changed in index.ts).

## Cuts
- Encryption migration is a function, not wired to a startup/CLI step or applied to any DB.
- No live e2e against a real gizzi process; covered by the fake-proxy tests on each side.

status: done
