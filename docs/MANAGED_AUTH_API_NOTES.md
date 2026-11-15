# Managed connector auth: allternit-api + gizzi + docs (WP2, WP3 relay, WP6, WP7)

Branch `feat/managed-auth-api`, Factory node n_4022 (dag_577250). Built against the shared contract in
`docs/MANAGED_AUTH_API_TASK.md`. `cmd/allternit-cloud-api` is untouched (the other executor's branch).

## What was built

### WP2: auth kind `managed` (allternit-api)

- **`src/managed_auth.rs` (new).** Cloud-api client for `GET /api/v1/managed-auth/configs` and
  `POST /api/v1/managed-auth/id-jag`, using the runtime device credential the `runtime-devices/me/*` calls
  already use (`Authorization: Bearer <device token>` from `relay_auth::process_secret()`, plus
  `x-allternit-runtime-id` when known, base `phone_sync::cloud_base()`). The RFC 7523 grant is ported from
  gizzi `xaa.ts` `exchangeJwtAuthGrant`: `grant_type=urn:ietf:params:oauth:grant-type:jwt-bearer`,
  `assertion`, `scope`, `client_secret_basic` (id/secret form-encoded first), `client_id` in the body
  for a public client. The token endpoint must be HTTPS, goes through the SSRF guard (`guarded_client`),
  and a missing `expires_in` becomes 3600 s. Cloud error codes map to plain wording. Vendor refusals only
  show `error` / `error_description`, never the body, and drop a description that echoes the assertion or
  the secret. The device token speaks for its paired owner only: any other caller is refused
  (`other_account` / `not_paired`) before anything is sent. Configs are cached for 60 s per owner. The
  automatic lookup before a plain connect has a 5 s timeout and backs off for 60 s after a failure, so a
  cloud outage doesn't slow normal connects.
- **Storage.** `store()` writes both copies together. (1) A `connector_connections` row
  (`auth_type='managed'`, sealed access/refresh tokens, `expires_at`, account key `managed:<org>`, made
  the default account, metadata `{managed:{org_id,resource,mcp_connector_id}}`). (2) An
  `mcp_connectors` row for the vendor MCP server (`resource`) plus an `mcp_oauth_sessions` row
  (`state='managed:<mcp id>'`, sealed tokens, metadata `{tokenEndpoint, resource, managed:{userId,
  connectorId, orgId, asIssuer, scopes}}`). The client secret is not stored on the runtime; each renewal
  fetches it with the new ID-JAG. `forget()` removes both on disconnect.
- **`connector_routes.rs`.**
  - `connect`: `via:"managed"` forces managed sign-in. With no `via`, an enabled config wins (checked
    before the curated/sidecar split, so it covers every catalog id). Any other `via` (e.g. `"oauth2"`
    from "Connect with sign-in instead") skips it.
  - `refresh` re-grants a stored `managed` row.
  - `execute`: re-grants on expiry (60 s early) and retries once on `401`. A managed connection on a
    sidecar-served connector answers `409 managed_use_mcp`, because its tools run through its MCP server.
  - `disconnect` forgets the managed rows. When every account is managed it skips the sidecar call.
  - Listing: each account now carries `auth_type`, and the optional meta `managed` hint is passed through.
  - A refusal answers `409 {error:"managed_auth_failed", code, message, connector, auth_type, fallback?}`.
    `fallback = {via:"oauth2", label:"Connect with sign-in instead"}` appears only when the connector has
    a normal sign-in. Nothing falls back by itself.
- **Re-grant with no UI (MCP path).** `mcp_apps.rs`: a session whose metadata has `managed` renews through
  `regrant_managed` (new ID-JAG + grant, both copies written), never through the refresh token.
  `mcp_user_proxy::with_session` re-grants once and retries when a managed connector answers 401 at
  `initialize`. A failed re-grant marks the connector unauthorized, so the dead token is never sent.
- **"Custom MCP server" managed path.** `connector_routes` has no custom-server concept. User-added MCP
  servers are `mcp_connectors` rows, signed in by `POST /api/v1/mcp/connectors/:id/oauth/start`
  (`mcp_directory_routes::start_connector_oauth`). That route now checks for a managed config whose
  `resource` matches the server URL (RFC 3986 normalised). If one exists, it grants with no browser and
  answers `{status:"connected", auth_type:"managed", org_id}` instead of `{authorize_url}`. A refusal is a
  `409` with the same body as above. `?via=oauth2` skips managed sign-in.
- **Local endpoints** (Clerk-authenticated like the rest of `/api/v1`):
  - `GET /api/v1/managed-auth/configs`, for the UI. Unpaired: `{configs:[], reason}`.
  - `POST /api/v1/managed-auth/id-jag {server_url | connector_id, org_id?}`, for gizzi. `Cache-Control:
    no-store`. `404` = not managed. `503` = can't tell right now (not paired, cloud down). `403`/`502` =
    a real refusal.
- **`assets/connectors.meta.json`.** The optional `managed: {as_issuer, token_endpoint, resource}` block is
  supported (HTTPS fields only) and shown as `managed` in the listing. **No vendor is marked managed.** I
  found no vendor documentation confirming that GitHub, Notion, Slack or Photon accept third-party ID-JAG
  issuers, so there is no URL to cite.

### WP3 relay (allternit-api)

- `POST /webhooks/mcp-edge/connectors` in `mcp_edge_relay.rs` (behind the same `RelayedAuth` signature
  layer as bots and the agents server) serves `mcp_user_proxy::dispatch_rpc(.., RpcCaller::Edge, ..)` for
  `x-allternit-owner`.
- The dispatch moved from `handle_rpc` into the shared `dispatch_rpc`. The per-turn HMAC path is unchanged:
  `handle_rpc` still verifies the proxy token, calls `dispatch_rpc` with `RpcCaller::Turn`, and keeps the
  same approval rule and the same `confirmation_required` message.
- An edge call never counts `_meta["allternit/approved"]`. A tool that needs confirmation answers JSON-RPC
  `-32001` with `data.code="confirmation_required"` and a plain `message`. The message names the setting
  ("set X's permission to "Ask before important changes" in Allternit (Apps)"), or for an important or
  destructive tool says it only runs from Allternit. Nothing is auto-confirmed.
- Edge tool listing cuts each connector off after 20 s (turn: 25 s) to fit cloud-api's 25 s budget.
  Offline and wake behaviour is cloud-api's, as for bots.

### WP6 (gizzi)

- `src/runtime/services/mcp/xaaAllternit.ts` (new): IdP mode `allternit`. On when gizzi is signed in to
  Allternit (`runtimeCredentials()` from `pins.ts`, now exported: `ALLTERNIT_API_URL` + token).
  `GIZZI_DISABLE_ALLTERNIT_XAA=1` turns it off. It asks the local allternit-api for the ID-JAG, refuses a
  non-HTTPS vendor token endpoint (loopback excepted), and runs `exchangeJwtAuthGrant`. `404`, `503` or an
  unreachable runtime return `null` (remembered 5 min / 60 s) and the normal OAuth flow runs. Any other
  failure throws.
- `xaa.ts`: `exchangeJwtAuthGrant` accepts a missing client secret (public client: `client_id` in the body).
- `auth.ts`:
  - `performMCPOAuthFlow` tries the allternit mode after the explicit `oauth.xaa` branch and before the
    consent flow.
  - `tokens()` runs a silent allternit grant for never-authed servers or expiring `allternitManaged`
    entries.
  - `hasMcpDiscoveryButNoToken` lets managed entries reconnect.
  - Tokens land in the normal OAuth slot with `allternitManaged: true`.
  - `GIZZI_ENABLE_XAA` still gates only the generic external-IdP mode.

### WP7 (docs, `surfaces/docs`)

- New: `guides/connectors-in-other-apps.mdx`, `guides/managed-authorization.mdx`,
  `security/allternit-idjag-issuer.mdx`, `security/trusting-outside-issuers.mdx`. Nav: the guides go after
  `guides/connectors` in Workspace workflows; the dev pages go at the end of Security & Enterprise.
- Updated: `guides/connectors.mdx` (managed connect + link), `api/connectors.mdx` (`managed` auth type,
  managed example, `managed_auth_failed` / `managed_use_mcp` codes).
- Per-platform steps only where the vendor's current page was read on 2026-11-03:
  - Claude: support article 11175166, "Customize > Connectors > + Add > Add custom connector"; Team/Enterprise
    "Organization settings > Connectors > Add > Custom > Web".
  - Claude Code: code.claude.com/docs/en/mcp, `claude mcp add --transport http`, then `/mcp`.
  - Codex: learn.chatgpt.com MCP page, `config.toml` `[mcp_servers.x] url = …`.
  - Gemini CLI: `docs/tools/mcp-server.md`, `httpUrl`, `/mcp auth <name>`.
  - ChatGPT: its help page returned 403 to automated fetches, so it gets only the generic "add a custom MCP
    connector with this URL" line.
- `python3 surfaces/docs/scripts/check_links.py`: 0 problems.

### Dependency map

`docs/dependency-map/features.json`: new `feature:managed-connector-auth` (building) with a dated decision.

## Tests

- **Rust, `cargo test -p allternit-api --lib`, filters managed_auth / mcp_user_proxy / mcp_edge_relay /
  connector_routes / mcp_directory_routes / mcp_routes / mcp_apps:** 129 passed, 0 failed. This includes
  the existing proxy, relay, directory and OAuth refresh suites, which confirm the per-turn path is
  unchanged. New tests:
  - `managed_auth`: the grant sends the assertion with Basic client auth and fills expiry; a refusal is
    plain and drops an echoed assertion or secret; plain-http token endpoints are refused before anything
    is sent; cloud codes map to plain words and the fallback appears only when asked for; the device
    credential only speaks for its owner; `store` writes both copies sealed, makes managed the default,
    and `forget` undoes it.
  - `mcp_user_proxy`: a managed connector is re-granted on expiry and on 401 with no UI (fake cloud + local
    AS); the edge serves the same union but can never approve (plain message naming the setting;
    destructive tools only from Allternit).
  - `mcp_edge_relay`: `/webhooks/mcp-edge/connectors` refuses unsigned calls and calls signed for another
    owner, and lists for the owner.
- **`cargo check -p allternit-api --lib`:** clean (no new warnings).
- **Smoke boot** (`target/debug/allternit-api`, port 18913, tree data dir): healthy, no route-overlap panic.
  `GET /api/v1/managed-auth/configs` and `POST /api/v1/managed-auth/id-jag` answer 401 with no token;
  `POST /webhooks/mcp-edge/connectors` answers 401 "missing signature". The server was stopped afterwards.
- **gizzi:** `bun test test/mcp/xaa-allternit.test.ts`: 4 passed (404/503 → normal sign-in, remembered;
  configured refusal throws; ID-JAG then jwt-bearer with Basic auth; non-HTTPS endpoint refused before the
  assertion is sent). `npm run typecheck` (typescript-7 tsc): exit 0.
- **Docs:** `python3 surfaces/docs/scripts/check_links.py`: 645 nav entries, 638 pages, 0 problems.

## Deviations

1. **Two token stores.** The spec says to store managed tokens in `connector_connections` like other kinds.
   The chat-turn proxy and the edge read only `mcp_connectors` + `mcp_oauth_sessions`, so a token kept only
   in `connector_connections` would never reach the tool union. Both are written together by one function
   (`managed_auth::store_for`) on every grant and re-grant.
2. **Client secret not kept on the runtime.** The contract returns the vendor secret with each ID-JAG, so
   every re-grant fetches it again. Storing it would add a second copy for no gain.
3. **Refresh tokens kept but not used for renewal.** Renewal always re-grants. That keeps the org's role and
   enabled checks in force at every renewal; a refresh token would bypass them.
4. **Custom MCP server path** lives in `mcp_directory_routes::start_connector_oauth`, not
   `connector_routes`, which has no custom-server concept. The response for a managed sign-in there is
   `{status:"connected"}` instead of `{authorize_url}`. The WP5 UI must handle it (and the `409` +
   `?via=oauth2` button).
5. **Local `/managed-auth/id-jag` returns the vendor client secret to gizzi** over loopback, to the
   authenticated paired owner only, `no-store`. gizzi runs the JWT-bearer step itself as WP6 asks. The
   alternative would be a local endpoint that does the whole grant.
6. **Deleting a managed MCP connector** (`DELETE /api/v1/mcp/connectors/:id`) also deletes the managed
   `connector_connections` row that points at it, so no index row is left saying "connected".

## Not verified

- **No live round trip against cloud-api.** Its `managed-auth` endpoints are being built on the other branch.
  Tests use a fake `Cloud` (the network edge) and a local test vendor AS.
- **No local end-to-end with a real third-party AS** (MCP conformance AS). The JWT-bearer leg is tested
  against a local axum AS that checks grant type, assertion and Basic auth.
- **No vendor marked managed.** No public vendor docs confirming outside-issuer support were found.
- **The WP5 UI isn't in this branch** (other executor / allternit-ai). It must call `via:"oauth2"` /
  `?via=oauth2` for the fallback button and render `auth_type:"managed"` accounts.
- **Validator, pre-existing:** `python3.11 scripts/dependency-map.py --validate` reports one existing error
  (`feature:voice` evidence `allternit-ai/src/components/voice/VoiceSpeaker.tsx`). The file exists on
  allternit-ai `origin/main`; the local allternit-ai checkout is behind. The new entry validates.

## Steering gate note

The steering commit gate answered STEER with an empty body on the first two commit attempts
(consults.log 04:12:06Z, 04:12:32Z), then let the commit through after the tests passed. I did not use
`STEER_GUARD_OFF`.

## For Eoj: shared checkout gate (not this node's work)

The session stop hook's git-discipline gate flagged the shared `allternit` checkout.
- `main` was 54 commits behind `origin/main`. It was clean, so I ran `git pull --ff-only` (done).
- 28 local branches are unmerged: ao/voice-engine-p1, ao/voice-session, docs-audit-3a..3d,
  factory/guest-driver, factory/safety-gaps, factory/sdk-v2, feat/driver-{flywheel,parallel,replay,safety,vision},
  feat/model-hosts, fix/phone-relay-route, resolve-926, salvage/shared-checkout-2026-10-21,
  session/cloud-w1-autotopup, session/cloud-w2-funding, session/cloud-w3-openlane,
  session/office-visual-quality-20261101, session/phone-pairing-handover, session/subs-tasks-route-director,
  session/subsfab-stream-correctness, test/personal-team-device-build, wip/subsfab-d16-fix-agent,
  wip/subsfab-toolbelt-fix-agent.
  They are other sessions' work. I did not delete or allowlist any; merge, delete or allowlist is your call.
