# MCP Apps host — Phase 1A notes (backend)

dag_859751 / node n_5937 · branch `p1-mcp-apps-host-backend`

## 1. Trace: where does a chat turn execute connector tools?

**Tools run in gizzi-code. allternit-api relays the turn and is the only producer of the browser stream that carries
tool frames, so `mcp_app` emission lives in allternit-api; the tool's raw result is handed over by gizzi-code.**

- Live route: `POST /api/agent-chat` → `cmd/allternit-api/src/v1_routes.rs:995` `agent_chat_bridge` (mounted
  `cmd/allternit-api/src/main.rs:1016` via `agent_chat_router`, behind Clerk auth → `Extension<AuthUser>`).
  It POSTs the prompt to gizzi (`{gizzi}/session/:id/message`, v1_routes.rs:~1465), subscribes to gizzi `/event` SSE
  (:~1362), and rewrites gizzi `message.part.updated` *tool* parts into `content_block_start` / `tool_result` /
  `tool_error` frames with `tool_frames_for_part` (v1_routes.rs:880, call site :~1683).
- **Not the live route:** `cmd/allternit-api/src/chat_routes.rs` + `gizzi_chat_stream.rs` — `chat_router()` has no caller
  (only `pub mod chat_routes` in lib.rs). Left untouched. gizzi-code's own `runtime/server/routes/agent-compat.ts`
  (`POST /agent-chat`, mirrors v1_routes) is a second producer for direct/desktop use; see "Not done".
- Tool build/execute: `cmd/gizzi-code/src/runtime/session/prompt.ts` (MCP tool wrap, ~:1348-1520) ←
  `MCP.toolCatalog()` `cmd/gizzi-code/src/runtime/tools/mcp/index.ts:610` → `convertMcpTool()` (:116, `client.callTool`).
  Clients come from gizzi's `Config.mcp` (`MCP.create()` :299, StreamableHTTP first then SSE); servers are registered with
  `POST /mcp` (`runtime/server/routes/mcp.ts:157` → `MCP.add`). Tool names the model sees: `mcp__<server>__<tool>`
  (`runtime/services/mcp/mcpStringUtils.ts`).
- Connector credentials live in allternit-api, not gizzi: `mcp_connectors` (V1 baseline) and `mcp_oauth_sessions.tokens`
  (written by `mcp_routes.rs::mcp_oauth_callback`). `mcp_dispatcher.rs` is the *inbound* `/mcp/server` registry (global,
  in-memory), not the chat path.
- **Gap found while tracing:** nothing copies `mcp_connectors` rows into gizzi's `Config.mcp` (grep: gizzi-code never reads
  `mcp_connectors`; allternit-api never calls gizzi `POST /mcp`). Emission therefore matches a gizzi MCP server to a
  connector by name (`normalize(server) == normalize(connector.name_id)`); see "Not done".
- Gateway (item 7): `services/gateway/service/src/main.py` loads `infrastructure/gateway/gateway_registry.json`, whose
  `/api` prefix routes to `allternit-api`; `/api/mcp/apps` and `/api/mcp/sandbox` resolve exactly like
  `/api/v1/connectors` (checked by resolving all four paths against the registry's longest-prefix rules). No registry
  change needed. (`infrastructure/0-infra/gateway/gateway_registry.json` has no `/api` catch-all and routes neither
  these nor `/api/v1/connectors`; it is not the file the gateway loads.)

## 2. What was built (task items → code)

| # | Item | Where |
|---|------|-------|
| 2 | Streamable HTTP transport (spec 2025-06-18: POST JSON-RPC, JSON *or* SSE response, `Mcp-Session-Id`, `MCP-Protocol-Version`, 202 notifications, DELETE on close, 404-with-session → re-init error) | `mcp/mcp-client/src/transport/streamable_http.rs` (+ same in `mcp/core/src/transport/streamable_http.rs`, which has the extra `send`/`receive` trait methods); `TransportType::StreamableHttp`, `TransportConfig::StreamableHttp`, registry `"streamable_http"` |
| 3 | `capabilities.extensions["io.modelcontextprotocol/ui"] = {"mimeTypes":["text/html;profile=mcp-app"]}` on `initialize` | Rust: `ClientCapabilities::with_mcp_apps()` used by `McpClient::initialize` in both crates (covers `services/gateway/routing`'s bridge too) and `mcp_dispatcher.rs::client_capabilities()`. gizzi-code (where chat tools run): new `runtime/tools/mcp/apps.ts` `MCP_APPS_CLIENT_CAPABILITIES`, applied to all three `new Client(...)` in `runtime/tools/mcp/index.ts` and to both copies of `services/mcp/client.ts` (`runtime` + `cli/ui/ink-app`, incl. the SDK-control client). The `gizzi mcp debug` connect probe in `cli/commands/mcp.ts` is not a chat path and is unchanged. |
| 4 | `POST /api/mcp/apps` | `cmd/allternit-api/src/mcp_apps.rs` (`app_bridge`), mounted `main.rs` `.nest("/api", mcp_apps_router())` inside the Clerk-authenticated stack. Only `tools/list`, `tools/call`, `resources/list`, `resources/read`, `resources/templates/list` (anything else, incl. `prompts/list`, → 403 `action_not_allowed`). Forwarded params are rebuilt from validated fields only. Connector looked up with `WHERE id=? AND user_id=?` (foreign, disabled and missing all → the same 404). Token = `access_token` of the latest authenticated `mcp_oauth_sessions` row, sent only as `Authorization: Bearer`. `tools/call` lists the connector's tools first and refuses a tool whose `_meta.ui.visibility` lacks `"app"` (403) or that is not listed (404). One `info!` per call: user, connector, action, tool name, status, outcome, elapsed — no arguments, results, or credentials. SSRF guard on the (user-supplied) connector URL: http(s) only, no loopback/private/link-local/CGNAT targets unless `ALLTERNIT_MCP_ALLOW_PRIVATE_CONNECTORS=1`. |
| 5 | `POST /api/mcp/sandbox` (+ `GET` availability probe the client calls) | `mcp_apps.rs` `sandbox_page`/`sandbox_status`. Returns the HTML document `sandbox-client.ts` writes into its iframe: embeds the app in a nested `sandbox="allow-scripts allow-forms allow-popups"` iframe (opaque origin) under a CSP `<meta>` that is the first element of the app document; the CSP is default-deny plus only the origins in `_meta.ui.csp` (each entry validated — no `;`, `,`, quotes, keywords, `data:`/`blob:`, lone `*`); `allow` is derived from `permissions`, not from the browser-sent string; relays `postMessage` both ways except `ui/notifications/sandbox-*`; posts `mcp-sandbox-ready` (`toolCallId`) and `ui/notifications/sandbox-proxy-ready`. `resources/read` results pass through untouched, so `_meta.ui` stays intact. |
| 6 | `mcp_app` emission; app-only tools hidden from the model | Emission: `mcp_apps.rs::app_frame_for_tool_part`, called from `v1_routes.rs` right after a `tool_result` frame. gizzi-code tags a completed tool part with `state.metadata.mcp = {server, tool, result:{content,structuredContent,_meta,isError}}` only for tools that declare a `ui://` resource (`runtime/session/prompt.ts` + `mcpAppMetadata` in `apps.ts`; results >256 KB are flagged, not stored). allternit-api resolves the user's connector, lists its tools, reads the `ui://` resource, and emits the frame in the exact shape `rust-stream-adapter.ts::buildMcpAppPart` requires (`toolCallId, toolName, connectorId, connectorName, resourceUri, html, title` + `description, allow, prefersBorder, tool, toolInput, toolResult, csp, permissions, domain`). Best effort (20 s cap; any failure = no app, chat continues). Hiding: `MCP.toolCatalog()` and both `fetchToolsForClient` copies drop tools whose `_meta.ui.visibility` omits `"model"`. |
| 7 | Gateway routing | Verified, no change (see §1). |
| 8 | Integration test against a real MCP Apps server over streamable HTTP | `mcp_apps.rs` `mod tests`: an in-process axum MCP Apps server (sessions, `Mcp-Session-Id`/`MCP-Protocol-Version` enforcement, bearer auth, extension-advertisement check on `initialize`, SSE reply with a leading notification, `_meta.ui` tools incl. an app-only and a model-only tool, `ui://` resource with CSP/permissions) driven through the real SQLite tables, `run_bridge`, the axum routes, and `app_frame_for_tool_part`: list → call → resource read → emission. |

Extra fixes made while there (same change): `ServerCapabilities`' `PromptsCapability`/`ResourcesCapability`/`ToolsCapability`/`RootsCapability`
failed to parse the ubiquitous `"resources": {}` (`missing field 'subscribe'`), which made `McpClient::initialize` fail against any
real MCP Apps server — now `#[serde(default)]` (regression test `client_initializes_when_servers_declare_empty_capability_objects`);
`MCP_PROTOCOL_VERSION` 2024-11-05 → 2025-06-18; `Tool`/`ToolResult` keep `_meta` and `structuredContent`; `McpClient::request` returns raw results;
`mcp_routes.rs` no longer logs the OAuth callback query (it carried the authorization code and state);
`mcp/core` tests that did not compile at baseline were repaired (`policy/client.rs` unit tests, `tests/stdio_integration_test.rs`
rewritten for the `spawn(StdioConfig)` API, five stale doc examples).

## 3. Files changed

Rust: `Cargo.lock`, `cmd/allternit-api/{Cargo.toml, src/lib.rs, src/main.rs, src/mcp_apps.rs (new), src/mcp_dispatcher.rs, src/mcp_routes.rs, src/v1_routes.rs}`,
`mcp/mcp-client/{Cargo.toml, src/lib.rs, src/protocol/types.rs, src/registry/mod.rs, src/transport/mod.rs, src/transport/streamable_http.rs (new)}`,
`mcp/core/{Cargo.toml, src/lib.rs, src/gateway_integration.rs, src/policy/client.rs, src/policy/mod.rs, src/protocol/types.rs, src/registry/mod.rs, src/transport/mod.rs, src/transport/streamable_http.rs (new), tests/stdio_integration_test.rs}`.
TypeScript (gizzi-code): `src/runtime/tools/mcp/{apps.ts (new), index.ts}`, `src/runtime/session/prompt.ts`,
`src/runtime/services/mcp/client.ts`, `src/cli/ui/ink-app/services/mcp/client.ts`, `test/mcp/apps.test.ts (new)`.
Docs: this file, `docs/MCP_APPS_HOST_MAP.md`, `docs/MCP_APPS_HOST_PHASE_1A_TASK.md`.
No DB migration was needed (existing tables), no GitHub Actions change, `/mcp/server` behaviour unchanged (its only change is the
extra `capabilities` it sends when it attaches remote servers; its tests still pass), web repo untouched.

## 4. Tests run (all from the worktree root unless noted)

```
cargo check -p allternit-api -p allternit-tools-gateway          → Finished `dev` profile … (no errors; pre-existing warnings only)
cargo test  -p mcp-client                                        → 18 passed (lib) + 1 passed (doc)
cargo test  -p mcp                                               → 64 passed (lib) + 8 passed (tests/stdio_integration_test.rs) + 21 passed (doc)
cargo test  -p allternit-api --lib mcp_                          → test result: ok. 46 passed; 0 failed   (22 are mcp_apps::tests::*, incl. the 6 integration tests)
cd cmd/gizzi-code && bun test test/mcp/apps.test.ts              → 5 pass, 0 fail
cd cmd/gizzi-code && bun test test/mcp/catalog.test.ts           → 2 pass, 0 fail
cd cmd/gizzi-code && bunx tsc --noEmit                           → 52 errors, none in any file touched here (all pre-existing: missing `@allternit/replies-*`, IntelliTaskScreen.tsx, packages/sdk/scripts, …)
```

Pre-existing failure, not caused by this change: `cmd/gizzi-code/test/mcp/headers.test.ts` (3 tests) fails identically on the
untouched base commit (checked in a throw-away worktree of `HEAD`) — it expects `Authorization: Bearer test-token` but the
environment injects a bundled-server token. Bun's `mock.module` also leaks across files, so run the `test/mcp/*` files one per process.

## 5. Not done, and why

1. **`mcp_connectors` → gizzi `Config.mcp` sync.** Nothing registers a user's connectors with gizzi, so today a connector's tools only
   run in chat if a gizzi MCP server is separately configured under the same name as the connector's `name_id` (that is what
   emission matches on). Wiring the sync means pushing per-user bearer tokens to a gizzi instance whose MCP state is per project
   directory, not per user — a tenancy/secret-handling decision for Eoj, so I did not guess at it.
2. **gizzi-code's own `/agent-chat` mirror (`runtime/server/routes/agent-compat.ts`)** does not emit `mcp_app`: it has no allternit
   connector identity to hand the bridge. Same decision as (1).
3. **Legacy SSE-only connector servers.** The bridge and emission speak streamable HTTP only; an SSE-only server answers the
   `initialize` POST with a 4xx and the caller gets `connector_unreachable`/`connector_http_error`.
4. **OAuth refresh.** The stored `expires_in`/`refresh_token` are not used; an expired token surfaces as `connector_unauthorized`
   ("reconnect it"). There is no refresh path in `mcp_routes.rs` to reuse.
5. **Outer sandbox frame is not a separate origin.** `allternit-ai/src/lib/ai/mcp/sandbox-client.ts` `document.write`s the proxy
   into an `allow-same-origin` iframe (web repo; not changed here — map item 5). The app itself is still isolated: the proxy runs it
   in an opaque-origin nested iframe. The SSRF guard resolves DNS at check time, so a DNS-rebinding race between check and connect
   remains possible.
6. `cmd/allternit-api/src/chat_routes.rs` + `gizzi_chat_stream.rs` (unmounted, dead) were left as found.

status: done
