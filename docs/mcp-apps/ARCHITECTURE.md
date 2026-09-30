# MCP Apps — architecture (platform side)

How `allternit-api` and `gizzi-code` host MCP Apps (SEP-1865 UI resources) for a user's connectors.
Deploy and config are in [OPERATIONS.md](OPERATIONS.md). Paths are relative to `cmd/allternit-api/src/` unless noted.

## Pieces

| Piece | Route / file | Auth |
|---|---|---|
| Host bridge | `POST /api/mcp/apps` — `mcp_apps.rs` | Clerk session |
| Sandbox proxy | `POST\|GET /api/mcp/sandbox` — `mcp_apps.rs` | Clerk session |
| Per-user proxy | `POST\|DELETE /mcp/user-proxy` — `mcp_user_proxy.rs` | HMAC proxy token (not Clerk) |
| `mcp_app` frames | `mcp_apps.rs::app_frame_for_tool_part`, called from `v1_routes.rs` | the chat turn's user |
| Connector OAuth | `mcp_routes.rs`, `mcp_directory_routes.rs` | Clerk session; callback per `mcp_routes.rs` |

Tools run in `gizzi-code`. `allternit-api` relays the chat turn (`POST /api/agent-chat`, `v1_routes.rs::agent_chat_bridge`)
and is the producer of the browser stream, so it emits the `mcp_app` frame. Connector URLs and OAuth tokens live in
`allternit-api` tables `mcp_connectors` and `mcp_oauth_sessions`, never in gizzi.

## Host bridge (`/api/mcp/apps`)

A rendered app asks the host to talk to the connector that produced it. Only five methods are forwarded:
`tools/list`, `tools/call`, `resources/list`, `resources/read`, `resources/templates/list` (`ALLOWED_ACTIONS`).
Anything else (including `prompts/list`) is refused with 403 `action_not_allowed`. The forwarded request is rebuilt from
validated fields; nothing else the app sent is passed through.

- The connector is loaded with `WHERE id = ? AND user_id = ?` and must be enabled. Foreign, disabled and missing look the same (404).
- The token is the latest authenticated `mcp_oauth_sessions` row, sent only as `Authorization: Bearer`.
- `tools/call` lists the connector's tools first. It refuses a tool that is not listed (404) or whose `_meta.ui.visibility`
  lacks `"app"` (403).
- One `info!` line per call: user, connector, action, tool, status, outcome, elapsed. No arguments, results or credentials.
- The client advertises `capabilities.extensions["io.modelcontextprotocol/ui"]` with `text/html;profile=mcp-app` on `initialize`.

## Sandbox proxy (`/api/mcp/sandbox`, `mcp-sandbox.gizziio.com`)

`POST /api/mcp/sandbox` returns an HTML document that embeds the app in a nested iframe with
`sandbox="allow-scripts allow-forms allow-popups"` (no `allow-same-origin`, so the app has an opaque origin). The app
document starts with a CSP `<meta>`: `default-src 'none'` plus only the origins in `_meta.ui.csp`. The proxy relays
`postMessage` both ways and announces `mcp-sandbox-ready` and `ui/notifications/sandbox-proxy-ready`. `GET` is an availability probe.

A separate static sandbox origin, `mcp-sandbox.gizziio.com`, is deployed from a Pages project (see OPERATIONS.md). Its
files are not in this repo, so this doc does not describe them.

## Per-user proxy (`/mcp/user-proxy`)

gizzi needs the user's connector tools without holding the user's tokens. For each chat turn `allternit-api` gives gizzi
one MCP server entry: this proxy.

- **Registration.** `proxy_registration()` returns `{server: "allternit-connectors", url, sessionId, token}`, or nothing if the
  user has no enabled connector. `agent_chat_bridge` sends it as top-level `mcpProxy` on the gizzi message payload, not in
  `metadata`, so it is not persisted. gizzi (`tools/mcp/user-proxy.ts`) keeps it in memory keyed by session and drops it at
  the end of the turn. `ALLTERNIT_MCP_PROXY_URL` overrides the URL gizzi uses.
- **Token binding.** `v1.<payload>.<hmac-sha256>`, claims `{uid, sid, exp}`, valid 15 minutes (`TOKEN_TTL_SECS`). Every request must
  also send `X-Allternit-Session` equal to `sid`. Bad signature, expiry, malformed token or wrong session all give 401. The
  route is public in `main.rs` because the handler does this check. The signing secret is `ALLTERNIT_MCP_PROXY_SECRET`, or random per process.
- **Methods.** `initialize`, `ping`, `tools/list`, `tools/call`, `resources/read`. Notifications get 202. Others get `-32601`.
- **Namespacing.** `tools/list` is the union over the user's enabled connectors, each tool renamed `<connector_prefix>__<tool>`.
  The prefix is the connector's `name_id`, normalised so it has no `__` and no leading or trailing `_`. Tools whose
  `_meta.ui.visibility` omits `"model"` are dropped. `_meta["allternit/connector"] = {id, name}` is added to each tool.
  Each connector has a 25 s timeout; a slow or failing connector is skipped.
- **Reads.** `resources/read` serves only `ui://` resources a listed tool declared.
- **Permission gate (model-initiated calls).** An install's permission mode (`mcp_app_installs`: `always_ask`,
  `ask_before_changes` (default), `ask_before_important_changes`) applies to the model's calls as well as a View's, with the
  web host's rules (`requiresConfirmation` / `isImportantTool` in `install-permission.ts`), using the annotations from the
  connector's own `tools/list`. No install, an unknown mode string or missing annotations fall to the stricter side
  (default mode; not read-only = a change). `tools/list` sets `_meta["allternit/requiresConfirmation"]` (true/false) on every
  tool, overwriting whatever the connector sent. `tools/call` on a marked tool answers `confirmation_required` unless
  `params._meta["allternit/approved"] === true`; the flag is not forwarded to the connector. gizzi (`McpUserProxy.gate`)
  asks the user through its ordinary permission system, permission class `mcp_app` (in `PermissionNext.ALWAYS_ASK`: asked in
  every mode including yolo/bypass, never remembered by "always"), showing app name, tool title and arguments (cut at 1000
  characters with an "N more characters not shown" marker), and adds the flag to that one call only after approval; a
  refusal is the tool error "The user declined this tool call". Why the flag is enough: the request is already authorised by
  the proxy token, which is HMAC-bound to {user, session} and lives only in the gizzi process's memory for the turn — not in
  the model's context, env or stored messages — and the flag travels in JSON-RPC `_meta`, which the model does not write (it
  supplies `arguments` only; an approval placed in `arguments` is ignored). The mode is read per request, so changing an
  install takes effect on the next call.
- gizzi hides app-only tools from the model as well (`MCP.toolCatalog()`), and the proxy entry is listed but never written to shared MCP state.

## `mcp_app` frames

After a completed tool part, `app_frame_for_tool_part` resolves the namespaced tool to `{connector, tool}`, reads the
tool's `ui://` resource (max 2 MiB), and emits `{type: "mcp_app", toolCallId, connectorId, resourceUri, html, ...}` plus CSP
and permissions from `_meta.ui`. It is best effort with a 20 s cap: any failure means no app and the chat continues.

- **v1 chat:** `v1_routes.rs` emits it right after the `tool_result` frame.
- **agent-compat mirror:** gizzi's own `POST /agent-chat` (`runtime/server/routes/agent-compat.ts`) emits the same frame
  when a `mcpProxy` entry is registered (`McpUserProxy.appFrameForPart`, which calls `buildMcpAppFrame` in `tools/mcp/apps.ts`).
  It is built asynchronously and the finish frame waits for it.
- `chat_routes.rs` and `gizzi_chat_stream.rs` are not mounted and produce nothing.

## SSRF model

Connector URLs are user-supplied and the host attaches the user's token to requests sent to them.

1. **Validate.** `validate_connector_url`: http(s) only. Refuses loopback, private, link-local, unspecified and CGNAT
   addresses (and IPv4-mapped IPv6, IPv6 loopback, ULA, link-local). A hostname must resolve, and one bad address among
   several is enough to refuse. The dev flag `ALLTERNIT_MCP_ALLOW_PRIVATE_CONNECTORS` skips the check.
2. **Pin.** The first validated address is pinned into the reqwest / transport client (`resolve`), so a second lookup cannot redirect.
3. **No redirects.** The streamable HTTP and SSE transports and `guarded_client` use `redirect::Policy::none()`.

The same guard covers OAuth discovery and token-endpoint calls (`guarded_client`). The directory has a stricter guard
(`mcp_directory_guard.rs`): https and default port only, no IP literals; see [DIRECTORY_REVIEW.md](DIRECTORY_REVIEW.md).
Transport is streamable HTTP first; a 404 or 405 on `initialize` falls back to legacy SSE.

## Token sealing

`mcp_oauth_sessions.tokens` and `mcp_connectors.oauth_client_secret` are sealed by `token_crypto` (AES-256-GCM, prefix
`enc:v1:`) when a key is configured; with no key they are stored as `plain:` and the boundary is explicit. The read path
opens sealed values and passes legacy unprefixed plaintext through. Expired access tokens are refreshed before a call
(60 s skew); a failed refresh gives `connector_unauthorized`. Existing plaintext rows stay plaintext until `seal_legacy_mcp_secrets()` is run (OPERATIONS.md).

## Auth roles

- **Clerk is the auth server** for Allternit users and, for the Agents MCP server (`/mcp/server`), for OAuth clients. It
  issues JWTs; `mcp_agents.rs` accepts an OAuth access token only with a matching `aud` (`MCP_PUBLIC_URL`) and scope
  `agents:read`, and then exposes six read-only agent tools (`list_agents`, `get_agent`, `list_runs`, `get_run`,
  `get_run_result`, `render_run_status`). Session tokens keep their existing access.
- **Connector OAuth** uses PKCE S256 and the `resource` parameter. When the connector's auth server supports client ID
  metadata documents, our client is `<ALLTERNIT_PUBLIC_BASE_URL>/oauth/client.json`.
- **Directory reviewers** are admins of the Clerk org in `ALLTERNIT_DIRECTORY_REVIEW_ORG_ID`. Unset means nobody can review.
- **Commerce operators** are user ids in `ALLTERNIT_COMMERCE_OPERATOR_USER_IDS`.

## Not done

- Nothing syncs `mcp_connectors` into a gizzi `Config.mcp`; the proxy is the only path.
- No live end-to-end run of the proxy against a real gizzi process; both sides are tested against fakes. That includes the model-call gate: the approval card is raised through gizzi's existing permission events, but no live Allternit chat UI run has shown it.
- The Agents MCP App has not been rendered in a real third-party host.
