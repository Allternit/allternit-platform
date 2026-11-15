# SPEC — Managed connector auth + Allternit connector gateway (2026-11-03)

Status: approved by Eoj 2026-11-03 ("do the suggestion": spec first, then Factory executors build all steps in one change).
Trigger: Claude "auto-connect plugins" (Boris Cherny, 2026-11-03) = MCP extension
`io.modelcontextprotocol/enterprise-managed-authorization` (ID-JAG, SEP-990).
Docs: https://modelcontextprotocol.io/extensions/auth/enterprise-managed-authorization ,
https://support.claude.com/en/articles/15537633 , draft-ietf-oauth-identity-assertion-authz-grant.

## Goal

1. **Allternit as issuer (outbound).** An Allternit org admin turns on managed authorization for a
   connector. Org members then connect to that connector's MCP server with no sign-in screen:
   Allternit signs an ID-JAG for the user, the connector's authorization server (AS) trusts the
   Allternit org issuer URL and returns an access token. Same model as "Claude-issued".
2. **Allternit as connector gateway (inbound).** `mcp.allternit.com/mcp/connectors` exposes all of
   a user's connected Allternit connectors as one MCP server to Claude, ChatGPT, Gemini, Codex,
   Claude Code, gizzi and other gateways. Normal OAuth (Clerk) for everyone; plus ID-JAG from a
   trusted issuer (a Claude org, Okta) so Claude Team/Enterprise members auto-connect.

All three surfaces (Desktop, ai.allternit.com incl. phone layout, m.allternit.com PWA) get the UI in
the same change; docs in the same change. iOS: see WP5.

## Facts from the code (verified 2026-11-03)

- Orgs = **Clerk organizations** (`organization_id`, migration 066). NOT `027_teams_app` (that is Microsoft Teams).
- cloud-api `src/routes/mcp_edge.rs`: `enum Target { Bot, Agents }`, `check_claims`, `EdgeBackend::verify` = Clerk only
  (`auth::clerk::verified_claims`), per-client approval `mcp_oauth_approvals (user_id, client_id, target)`,
  RFC 9728 metadata naming Clerk (`MCP_OAUTH_ISSUER`). Relay to runtime: `/webhooks/mcp-edge/{bots/:id,server}`
  handled in allternit-api `src/mcp_edge_relay.rs`.
- cloud-api `src/auth/dataplane_jwt.rs`: hand-rolled Ed25519 JWT + JWKS (`/api/v1/auth/dp-jwks`). Pattern to copy;
  **ID-JAG must use ES256** (vendor AS libraries rarely accept EdDSA).
- allternit-api `src/connector_routes.rs`: auth kinds `local_cli | api_key | oauth2 | device_flow` (dispatch ~L2069),
  tokens sealed by `token_crypto.rs` in `connector_connections`; meta in `assets/connectors.meta.json`.
- allternit-api `src/mcp_user_proxy.rs`: union of a user's enabled connectors as one streamable-HTTP MCP server,
  `<connector>__<tool>` namespacing, permission modes from `mcp_app_installs`, per-turn HMAC proxy token.
- gizzi `src/runtime/services/mcp/xaa.ts` + `xaaIdpLogin.ts`: full ID-JAG client (RFC 8693 + RFC 7523),
  off unless `GIZZI_ENABLE_XAA`; needs a browser IdP login for the id_token.

## Work packages

### WP1 — Allternit issuer (cloud-api)
- Key: ES256, PEM from env `ALLTERNIT_IDJAG_ES256_PEM`; unset ⇒ issuer endpoints 503 `idjag_not_configured` (fail closed, like dp-jwt). `kid` = b64url SHA-256 of the public key.
- Issuer URL per org: `https://api.allternit.com/oauth2/org/<clerk_org_id>` (base from env `ALLTERNIT_IDJAG_ISSUER_BASE`).
- `GET <issuer>/.well-known/oauth-authorization-server` and `/.well-known/openid-configuration` (issuer, jwks_uri, token_endpoint, `grant_types_supported` incl. token-exchange).
- `GET <issuer>/jwks.json`.
- `POST <issuer>/token` RFC 8693: `subject_token` = caller identity (Clerk session JWT from web/desktop, OR the runtime device credential + user id that allternit-api already uses to talk to cloud-api — reuse the existing mechanism, do not invent one), `requested_token_type=urn:ietf:params:oauth:token-type:id-jag`, `audience` = vendor AS issuer, `resource` = vendor MCP URL.
- Issue only if: user is a member of the org (Clerk), a `managed_auth_configs` row for (org, audience) is enabled, the member's org role is in its `roles`, and the user's primary email is **verified**.
- ID-JAG: header `typ: oauth-id-jag+jwt`, `alg: ES256`, `kid`; claims `iss, sub (Clerk user id), email, email_verified:true, aud, client_id (Allternit's client id at that vendor AS), resource, scope, jti, iat, exp (≤ 300 s)`.
- Migration (next number after 115): `managed_auth_configs (id, org_id, connector_id, as_issuer, token_endpoint, resource, client_id, client_secret_sealed, roles text[], scopes text[], enabled, created_by, created_at, updated_at)` and `idjag_issuance_log (jti PK, org_id, user_id, aud, resource, issued_at)`. Secrets sealed with the existing cloud-api secret sealing.
- Admin API (org admin = Clerk `org:admin` only): `GET|PUT|DELETE /api/v1/orgs/:org/managed-auth[/:connector]`.

### WP2 — "managed" connector auth kind (allternit-api)
- New auth kind `managed` in `connector_routes.rs` dispatch. Connect flow: fetch the org config, ask WP1 `/token` for an ID-JAG, then RFC 7523 JWT-bearer grant at the vendor `token_endpoint` (`grant_type=urn:ietf:params:oauth:grant-type:jwt-bearer`, `assertion=<ID-JAG>`, client auth from config). Port the steps from gizzi `xaa.ts` (`performCrossAppAccess`); do not shell out to gizzi.
- Store the access token in `connector_connections` as other kinds do. On 401/expiry: get a new ID-JAG and re-grant, no UI. Refresh token used if the AS returns one.
- `connectors.meta.json`: optional `managed: { as_issuer, token_endpoint, resource }` per connector. **Ship with no vendor marked managed** unless a vendor's docs confirm third-party-issuer support (cite the URL in a comment). Plus a "Custom MCP server" path where the admin enters as_issuer/token_endpoint/resource/client_id/secret.
- When a connector has both `oauth2` and an enabled managed config for the user's active org, `managed` wins; on managed failure show a plain error with a "Connect with sign-in instead" button (normal oauth2). No silent fallback.

### WP3 — Connector gateway on the edge (cloud-api + allternit-api)
- `Target::Connectors`: route `POST /mcp/connectors`, resource `<MCP_PUBLIC_URL>/connectors`, scope `connectors:use`, approval target `connectors`, runtime path `/webhooks/mcp-edge/connectors`, RFC 9728 metadata at `/.well-known/oauth-protected-resource/mcp/connectors`.
- allternit-api `mcp_edge_relay.rs`: handler for `/webhooks/mcp-edge/connectors` that serves the same tool union as `mcp_user_proxy.rs` for the `x-allternit-owner` user (refactor the dispatch into a shared fn; keep the HMAC per-turn path unchanged).
- Permission modes apply. A tool that needs confirmation is NOT run from an outside client: return a JSON-RPC error in plain words telling the user which connector setting allows it. Never auto-confirm.
- Same offline/wake behaviour and 25 s budget as bots.

### WP4 — Trusted issuers on the edge (cloud-api)
- Edge authorization server: `POST /oauth2/edge/token` (JWT-bearer grant) + its AS metadata + JWKS. Accept an ID-JAG only if: `iss` is in `mcp_trusted_issuers` for some org, signature checks against that issuer's JWKS (cache, honour `kid` rotation), `aud` = edge AS issuer, `resource` is an edge resource, `exp`/`iat` sane, `jti` not seen (replay table, TTL = exp), `email` present and verified (claim or Clerk lookup).
- User mapping: Clerk user with that **verified** email AND a member of the org that trusts `iss`. Zero or many matches ⇒ `invalid_grant`, never a guess.
- Issue an edge access token (ES256, our issuer, `aud` = resource, `scope`, `sub` = Clerk user id, ≤ 1 h). `EdgeBackend::verify` accepts edge-issued tokens and Clerk tokens; `check_claims` unchanged for Clerk.
- Org consent: when an admin adds a trusted issuer, the edge treats `client_id = issuer:<iss>` as approved for that org's members on the targets the admin picked (`agents`, `connectors`, bots stay per-user). Store as rows or a join on the issuer table — no per-user approval screen for this path.
- PRM `authorization_servers` lists Clerk first, then the edge AS.
- Migration: `mcp_trusted_issuers (id, org_id, issuer, jwks_uri, kind 'claude'|'okta'|'other', label, targets text[], created_by, created_at, revoked_at)`, `edge_jti_seen (jti PK, expires_at)`.
- Admin API (org admin): `GET|POST|DELETE /api/v1/orgs/:org/trusted-issuers`.
- Claude's exact claims/JWKS path are not public: implement against the MCP extension spec, keep issuer-specific mapping in one function, and document that Claude-issued is verified only once the Claude Connect-step guide is read.

### WP5 — UI (allternit-ai: Desktop + phone layout + PWA; iOS)
- Settings › Connectors:
  - Org admins: **Managed authorization** panel per connector (and "Custom MCP server"): pick roles, scopes, client id/secret; shows the org **issuer URL** with copy; Save & turn on / off.
  - Org admins: **Outside AI apps** panel: list/add/remove trusted issuers (Claude org issuer URL, Okta), pick targets.
  - Everyone: **Use your connectors in other apps** card: the URL `https://mcp.allternit.com/mcp/connectors` with copy, plus short per-platform steps (Claude: Settings › Connectors › Add custom connector; ChatGPT: Apps › Developer mode › Create; Codex: `codex mcp add allternit --url …`; Claude Code: `claude mcp add --transport http allternit …`; Gemini; gizzi: built in). Verify each step's wording against the vendor's current docs; omit a platform whose steps are not verified.
- Same React source for Desktop, ai.allternit.com phone layout (PhoneShell, phone.css) and m.allternit.com. Follow existing Settings look; white + `--neutral-fill`, no tan; click to act, no hover menus.
- Signed-out / no-org / non-admin states handled (non-admins see read-only state; no-org users see only the "other apps" card).
- iOS: no web parity goal (Eoj). Add only the "Use your connectors in other apps" card (URL + copy) to the native Settings › Connectors screen if one exists; otherwise skip and note it.

### WP6 — gizzi
- New IdP mode `allternit`: when the user is signed in to Allternit and a connector's MCP server advertises the extension, gizzi gets the ID-JAG straight from WP1 `/token` (no browser IdP login) and runs the existing JWT-bearer step in `xaa.ts`. Enabled by default when signed in; `GIZZI_ENABLE_XAA` keeps gating the generic external-IdP mode.

### WP7 — Docs
- `allternit/surfaces/docs` (Mintlify): user guide "Connect your connectors to other AI apps" and "Managed authorization for your organization"; dev guide "Allternit as an ID-JAG issuer" (endpoints, claims, key rotation) and "Trusting outside issuers". Add to nav. `python3 surfaces/docs/scripts/check_links.py` → 0 problems. MDX: no bare `<word>` in prose.

## Security checklist (every executor)
- Never map a user by unverified email. Never pick one of several matches.
- Bind `aud`, `resource`, `jti`; short `exp`; log every issuance and every accepted assertion.
- Fail closed when keys/env are missing.
- Org admin checks server-side on every admin endpoint.
- No secret in logs, URLs or client bundles.

## Rules for executors
- Code only; the orchestrator builds/checks once at the end. Lean diffs.
- One branch per repo: `feat/managed-connector-auth` (allternit platform) and `feat/managed-connector-auth` (allternit-ai). Worktrees from current origin/main; never commit in a shared checkout.
- Do not merge, deploy, publish runtimes, or run prod migrations. Eoj approves merge and deploy.
- Smoke-boot allternit-api if routes change (route overlap crash-loops it).

## Acceptance
- Unit tests: ID-JAG mint (claims, typ, exp), refusal cases (non-member, role not allowed, unverified email, disabled config), edge JWT-bearer accept/refuse (unknown iss, bad sig, wrong aud, replayed jti, 0/2 email matches), `Target::Connectors` routing + metadata.
- Local end-to-end: allternit-api connects a test MCP server whose AS trusts the local Allternit issuer (use the MCP conformance example AS or a small test AS) with no browser.
- Claude/ChatGPT/Codex reach `/mcp/connectors` with OAuth and list tools (live check after Eoj-approved deploy).
- Docs build clean; UI checked at desktop and phone widths.
