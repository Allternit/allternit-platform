# Task: managed connector auth — allternit-api + gizzi + docs (WP2, WP3 relay part, WP6, WP7)

Repo: allternit platform. Branch feat/managed-auth-api. Another executor builds the cloud-api side (WP1/WP4) on another branch at the same time; code against the contract below, do not touch cmd/allternit-cloud-api.

Build:
- WP2: auth kind "managed" in cmd/allternit-api/src/connector_routes.rs (dispatch ~L2069): uses GET /api/v1/managed-auth/configs and POST /api/v1/managed-auth/id-jag on cloud-api, then the RFC 7523 JWT-bearer grant at the vendor token_endpoint (port the steps from cmd/gizzi-code/src/runtime/services/mcp/xaa.ts). Tokens stored in connector_connections like other kinds; re-grant on expiry/401 with no UI; no silent fallback to oauth2 (return a plain error the UI can show with "Connect with sign-in instead"). connectors.meta.json: support an optional "managed" block; mark NO vendor as managed unless its docs confirm third-party issuer support (cite URL). Add the "Custom MCP server" managed path if connector_routes has a custom-server concept; otherwise note it.
- WP3 relay: POST /webhooks/mcp-edge/connectors in cmd/allternit-api/src/mcp_edge_relay.rs serving the same tool union as mcp_user_proxy.rs for the x-allternit-owner user (refactor the dispatch into a shared fn; keep the per-turn HMAC path unchanged). Tools needing confirmation are refused with a plain JSON-RPC error naming the connector setting; never auto-confirm.
- WP6: gizzi IdP mode "allternit" in cmd/gizzi-code/src/runtime/services/mcp (xaa.ts / xaaIdpLogin.ts / auth.ts): when signed in to Allternit, get the ID-JAG via the local allternit-api (add a small local endpoint in allternit-api that proxies to POST /api/v1/managed-auth/id-jag for a given MCP server, if gizzi can't reach cloud-api itself) and run the existing JWT-bearer step; default on when signed in; GIZZI_ENABLE_XAA keeps gating the generic external-IdP mode. Typecheck with the repo's tsrs setup.
- WP7 docs in surfaces/docs (Mintlify): user guides "Use your Allternit connectors in other AI apps" (URL https://mcp.allternit.com/mcp/connectors; per-platform add steps ONLY where you verified the vendor's current wording — otherwise a generic "add a custom MCP connector with this URL" line) and "Managed authorization for your organization"; dev guides "Allternit as an ID-JAG issuer" and "Trusting outside issuers" (use the contract below). Add to nav. python3 surfaces/docs/scripts/check_links.py → 0 problems. MDX: no bare <word> in prose, quote ": " in frontmatter.
- Smoke-boot allternit-api (route overlap crash-loops it) after adding routes.
- Update docs/dependency-map/features.json with the new feature and run --validate.

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
- When done write docs/MANAGED_AUTH_API_NOTES.md (what you built, files, endpoints, tests run + results, deviations, anything not verified) and THEN create the sentinel file docs/MANAGED_AUTH_API.sentinel (empty). The sentinel is the completion signal: create it last.

## What this can break
connector_routes.rs / mcp_user_proxy.rs / mcp_edge_relay.rs feed: Bot Mode threads [live], Interactive answers [live], MCP Apps and events [live], Mail triage [live], Morning brief [live], Setup checklist [live]; journeys "Answer with a live data card" (step 5), "Approve an MCP app's action" (step 1). Existing connector kinds and the per-turn proxy must behave exactly as before.
