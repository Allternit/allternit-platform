# MCP Apps Host — Map (P1)  · dag_859751 / node n_5937

Goal: Allternit becomes a spec-compliant **MCP Apps host** (SEP-1865, `io.modelcontextprotocol/ui`,
spec: https://github.com/modelcontextprotocol/ext-apps/blob/main/specification/2026-01-26/apps.mdx),
the same standard ChatGPT and Claude implement. Any third-party MCP server with `ui://` resources
must render + work inside Allternit chat unchanged. Naming (decided by Eoj): package = "Allternit Plugin",
UI = "MCP App". The old name "Mini-app" is retired in user-facing copy.

## Already exists (allternit-ai repo)
- `src/lib/ai/mcp/apps.ts` — extension id, mime `text/html;profile=mcp-app`, `_meta.ui.resourceUri`,
  visibility, CSP/permissions types, `McpAppBridgeRequest` (tools/list|call, resources/list|read|templates/list).
- `src/components/ai-elements/McpAppFrame.tsx` — AppBridge from `@modelcontextprotocol/ext-apps/app-bridge`,
  inline/fullscreen/pip, currently `srcDoc` iframe (NOT the spec's separate-origin sandbox proxy).
- `src/lib/ai/mcp/app-bridge-api.ts` → POST `/api/mcp/apps` ; `src/lib/ai/mcp/sandbox-client.ts` → POST `/api/mcp/sandbox`.
- Stream parts `mcp_app`, `mcp_app_message`, `mcp_app_update_context` (`rust-stream-adapter.ts`, `ui-parts.types.ts`),
  rendered by `UnifiedMessageRenderer.tsx`. Fixtures: `src/lib/ai/mcp/fixtures/fixture-server.ts`, `e2e.test.ts`.
- Legacy/parallel: ACI mini-apps `src/views/aci/AciMiniAppsView|AciMiniAppFrameView|MiniAppReviewConsoleView`,
  capsule `MiniappManifest` (entry a2ui|html|component), `window.allternit.invokeTool` capsule protocol.

## Missing (verified by grep 2026-09-29)
1. No handler for `/api/mcp/apps` or `/api/mcp/sandbox` anywhere (web /api → gateway → allternit-api).
2. Nothing server-side emits `mcp_app` parts when a tool with `_meta.ui.resourceUri` is called.
3. Rust MCP client (`mcp/mcp-client`, `mcp/core`) has stdio + SSE only — no streamable HTTP.
   gizzi-code's TS client (`cmd/gizzi-code/src/runtime/services/mcp/client.ts`) has it.
4. Host does not advertise `capabilities.extensions["io.modelcontextprotocol/ui"]` on initialize.
5. No separate-origin sandbox proxy (spec §"Sandbox proxy": different origin, allow-scripts+allow-same-origin,
   forwards non-`ui/notifications/sandbox-*` messages, enforces `_meta.ui.csp`).
6. Two frames (McpAppFrame vs AciMiniAppFrameView) and two protocols.

Connector auth already exists: `cmd/allternit-api/src/mcp_routes.rs` (`/mcp/connectors`, OAuth PKCE, tables
`mcp_connectors`, `mcp_oauth_sessions`). Architecture rule: allternit-api calls gizzi-code over HTTP for model
work; find where chat tool calls for connected MCP servers actually execute before choosing where emission lives.
