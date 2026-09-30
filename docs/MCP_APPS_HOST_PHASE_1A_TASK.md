# Phase 1A (backend, allternit-platform repo) — MCP Apps host server side
Read docs/MCP_APPS_HOST_MAP.md first. Also read the web-side contracts (read-only, sibling checkout):
`../allternit-ai/src/lib/ai/mcp/{apps.ts,app-bridge-api.ts,sandbox-client.ts}`, `../allternit-ai/src/lib/ai/rust-stream-adapter.ts`,
`../allternit-ai/src/lib/ai/ui-parts.types.ts`. Match those request/response shapes exactly; do not change the web repo.

## Do
1. Trace: where does a chat turn execute tools from a user's connected MCP connector (allternit-api vs gizzi-code)?
   Write the answer (file:line) at the top of docs/MCP_APPS_HOST_PHASE_1A_NOTES.md before coding.
2. Streamable HTTP transport for the Rust MCP client (spec 2025-06-18: POST JSON-RPC, JSON or SSE response,
   `Mcp-Session-Id`, `MCP-Protocol-Version` header), alongside existing stdio/SSE. Unit tests.
3. Advertise `capabilities.extensions["io.modelcontextprotocol/ui"] = {"mimeTypes":["text/html;profile=mcp-app"]}`
   in the host's MCP `initialize` (every MCP client path that serves chat, Rust and gizzi-code if that's where tools run).
4. `POST /api/mcp/apps` route (auth: existing Clerk user auth): proxy `McpAppBridgeRequest` actions to the named
   connector using the user's stored connector credentials/OAuth token. Only allow tools/list, tools/call,
   resources/list, resources/read, resources/templates/list. Reject a `tools/call` for a tool whose
   `_meta.ui.visibility` excludes `"app"`. Connector must belong to the caller (no cross-user access). Log each call.
5. `POST /api/mcp/sandbox` — implement whatever sandbox-client.ts expects (read it); if it needs the resource HTML +
   CSP, return it from `resources/read` of the connector with `_meta.ui` intact.
6. Emission: when the model calls a tool whose definition has `_meta.ui.resourceUri` (or legacy
   `_meta["openai/outputTemplate"]`), emit an `mcp_app` stream part in the exact shape rust-stream-adapter.ts parses
   (connectorId, tool name, input, result incl. structuredContent + _meta, resourceUri). Tools with visibility `["app"]`
   only must be hidden from the model's tool list.
7. Make sure the gateway routes `/api/mcp/apps` and `/api/mcp/sandbox` to allternit-api (check how `/api/v1/connectors` is routed).
8. Integration test against a real MCP Apps server over streamable HTTP (e.g. an ext-apps example like
   `basic-server-vanillajs`, or a minimal Rust/TS test server in the test dir): list → call → resource read → emission.

## Constraints
- Work only in this worktree, branch `p1-mcp-apps-host-backend`. Commit there; do NOT push, merge, deploy, or run prod migrations.
  If a DB migration is needed, write it but do not apply it to any shared/prod DB.
- No GitHub Actions changes. Keep existing `/mcp/server` behaviour intact.
- `cargo check` + the relevant crate tests must pass; report exact commands + output tail.
- Never log tokens or connector secrets.

## Deliverable
docs/MCP_APPS_HOST_PHASE_1A_NOTES.md: the trace answer, files changed, test commands + results, anything not done and why.
Last line exactly: `status: done`
