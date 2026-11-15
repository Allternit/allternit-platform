# Task: managed connector auth — cloud-api (WP1, WP3 edge part, WP4)

Repo: allternit platform, crate cmd/allternit-cloud-api. Branch feat/managed-auth-cloud.

Build:
- WP1 Allternit issuer (ES256, fail closed, per-org issuer URL, metadata, JWKS, RFC 8693 /token for Clerk-session callers), managed_auth_configs + idjag_issuance_log migration (next number after the highest in migrations_pg; also check that prod migration handling per cloud-api conventions is followed — see existing migration headers), admin API, and the two runtime endpoints in the contract below.
- WP3 edge part: Target::Connectors in src/routes/mcp_edge.rs (route, resource, scope connectors:use, approval target "connectors", runtime path /webhooks/mcp-edge/connectors, RFC 9728 metadata), same relay/offline behaviour as bots.
- WP4 trusted issuers + edge JWT-bearer AS (/oauth2/edge/token, metadata, JWKS), replay table, verified-email + org-membership mapping (0 or >1 matches = invalid_grant), edge-issued tokens accepted by EdgeBackend::verify alongside Clerk, org consent via client_id "issuer:<iss>", PRM lists Clerk first then the edge AS.
- Reuse the hand-rolled JWT approach in src/auth/dataplane_jwt.rs (use p256/ES256; check Cargo.toml for an existing crate before adding one).
- Smoke: the crate builds and its tests pass; boot check that routes don't overlap (axum panics on duplicate routes at startup — add a router-construction test if none exists).

## Shared contract between executors (fixed by the orchestrator — do not change it alone)

Runtime → cloud-api (allternit-api on the user's computer asks cloud-api; auth = the SAME runtime device credential allternit-api already uses for its other cloud-api calls — find it, do not invent a new one):
- `GET  /api/v1/managed-auth/configs` → `{ "configs": [ { "org_id", "connector_id", "as_issuer", "token_endpoint", "resource", "scopes": [..] } ] }` — only enabled configs the user may use (member + role allowed). No secrets.
- `POST /api/v1/managed-auth/id-jag` body `{ "connector_id", "org_id"? }` → `{ "id_jag": "<jwt>", "token_endpoint", "client_id", "client_secret"?, "expires_in" }`. cloud-api does the role/email checks. It returns the vendor client credentials so the runtime can do the RFC 7523 grant (secret only over this authenticated channel, never logged). Error body `{ "error": { "code", "message" } }`, codes: `not_configured`, `not_member`, `role_not_allowed`, `email_unverified`, `idjag_not_configured`.

Browser/Desktop UI → cloud-api (Clerk session JWT, org admin = Clerk role `org:admin` in that org):
- `GET|PUT|DELETE /api/v1/orgs/:org/managed-auth[/:connector_id]` — PUT body `{ as_issuer, token_endpoint, resource, client_id, client_secret?, roles: [], scopes: [], enabled }`. GET never returns the secret (returns `has_secret: bool`). GET list also returns `issuer_url` (the org's Allternit issuer URL).
- `GET|POST|DELETE /api/v1/orgs/:org/trusted-issuers[/:id]` — POST body `{ issuer, jwks_uri?, kind: "claude"|"okta"|"other", label, targets: ["connectors","agents"] }`.
- Non-admin members may GET both (read-only); non-members 403.

Public:
- Allternit org issuer: `<ALLTERNIT_IDJAG_ISSUER_BASE>/oauth2/org/<clerk_org_id>` (+ `/.well-known/oauth-authorization-server`, `/.well-known/openid-configuration`, `/jwks.json`, `/token` RFC 8693 for Clerk-session callers). Default base `https://api.allternit.com`.
- Edge gateway MCP URL: `https://mcp.allternit.com/mcp/connectors` (resource `<MCP_PUBLIC_URL>/connectors`, scope `connectors:use`).
- Edge relay to runtime: `POST /webhooks/mcp-edge/connectors` (header `x-allternit-owner` = Clerk user id, signed relay as for bots).

## Rules
- Read docs/MANAGED_AUTH_MAP.md (the full spec) first. Your scope is ONLY the work packages listed below.
- eval "$(gizzi agents tree env)" first (own CARGO_TARGET_DIR). Cargo: `cargo check -p <crate>` while iterating, then run the tests you wrote for your crate once at the end. Never pkill cargo/rustc; never set RUSTC_WRAPPER.
- Tests only for tricky logic (token checks, refusals, mapping). No filler tests. Mock only edges.
- Commit on your branch with clear messages (end each with "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"). Push the branch. Do NOT open/merge a PR, deploy, publish runtimes, or run prod migrations.
- Lean diffs. Follow the existing code style of the file you edit. No secrets in logs.
- If something in the spec is wrong against the real code, choose the safe production option, and write it under "Deviations" in your notes.
- When done write docs/MANAGED_AUTH_CLOUD_NOTES.md (what you built, files, endpoints, tests run + results, deviations, anything not verified) and THEN create the sentinel file docs/MANAGED_AUTH_CLOUD.sentinel (empty). The sentinel is the completion signal: create it last.

## What this can break
mcp_edge.rs feeds: A2A public edge, Bots operate real phones [live], Mail triage [live], Setup checklist [live], Cloud phones, Team roster. Bot (/mcp/bots/:id) and agents (/mcp) targets must behave exactly as before — keep their existing tests green.
