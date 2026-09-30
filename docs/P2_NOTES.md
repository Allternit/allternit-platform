# P2 — Allternit Agents MCP server: notes

## Files changed
- `cmd/allternit-api/src/mcp_agents.rs` (new) — OAuth protected-resource metadata, `WWW-Authenticate` challenge layer, OAuth access-token verification (JWKS, issuer, exp, `aud` == `MCP_PUBLIC_URL`, scope `agents:read`), six read-only tools, `ui://` resource, tests.
- `cmd/allternit-api/assets/run-status.v1.html` (new) — MCP App (vanilla JS, `ui/*` postMessage bridge, `hostContext.styles` variables, white + `--neutral-fill`).
- `cmd/allternit-api/src/mcp_server_routes.rs` — `initialize` now returns `instructions`, `resources` capability, and negotiates protocol version (2025-06-18 / 2025-03-26); adds `resources/list`, `resources/read`, `resources/templates/list`; new tools in `tools/list`/`tools/call`; OAuth callers are limited to the agent tools.
- `cmd/allternit-api/src/auth.rs` — split `verify_token` into `verify_token_claims` + `user_from_claims` (behavior unchanged); `is_clerk_issuer`; test helper with custom claims.
- `cmd/allternit-api/src/main.rs`, `src/lib.rs` — mount well-known routes (public) and the challenge layer.
- `docs/P2_SUBMISSION_PACKAGE.md` (new).

## Behavior
- Tokens without `sid` from a Clerk issuer are treated as OAuth access tokens: verified for `aud` and `agents:read`, then restricted to the six read-only tools (shell/file/http tools are NOT exposed to them). Session tokens and other existing auth modes are unchanged and also get the new tools.
- Bad aud/invalid → 401 `invalid_token`; missing scope → 403 `insufficient_scope`; no/invalid token at the middleware → 401. All carry `WWW-Authenticate: Bearer resource_metadata="…"`.
- Every query filters by the caller's `user_id` (agents, `agent_runs`).

## Tests
- `cargo check -p allternit-api --tests` — passes (existing warnings only).
- `cargo test -p allternit-api --lib mcp_` — 38 passed, 0 failed. New coverage: JWT good / wrong aud / missing aud / missing scope / expired / wrong issuer / unknown key, tools/list shape and annotations, resources list/read of the UI, caller scoping against a migrated temp DB, metadata document.

## Cut / not done
- No HTTP-level test of the 401/403 header responses or of `initialize`/`resources/*` through the router (logic covered at function level).
- `mcp.allternit.com/mcp` → `/mcp/server` edge routing is not in this repo; deployment item for Eoj.
- Batch JSON-RPC and `Mcp-Session-Id` remain unsupported (as before).
- MCP App not rendered in a real host (ChatGPT/Claude); only string/structure-checked.
- Not pushed, merged, or deployed.

## Out of scope: git-discipline stop hook
The Stop hook reports the shared main checkout is 88 commits behind origin and lists ~65 stale unmerged branches. That is outside this worktree; I did not pull, merge, or delete anything there. Eoj should decide about those branches.

status: done
