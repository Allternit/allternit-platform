# TEAMS_NOTES — Microsoft Teams shared app (ao-teams)

Executor: ao-teams (Kimi). Migrations: **V217** (allternit-api sqlite) + **027** (cloud-api `migrations_pg`).
Plan: `Allternit Brain/Research/specs/channel-packs.md` v3/v3.1/v3.2, approved by Eoj 2026-10-02.
Context: Microsoft blocks new multi-tenant Azure Bot registrations since 2025-07-31, so the design is
**one single-tenant Azure Bot in Allternit's tenant + one multi-tenant Entra app**, with one Teams app
package installed per customer tenant. Whether a single-tenant bot can send **proactively** into another
tenant is UNVERIFIED — proactive send is a separate, flagged code path (`TEAMS_PROACTIVE_SEND`, off by
default). Reactive replies are the default.

## Files changed

New files (owned by this executor):
- `cmd/allternit-cloud-api/src/channels/mod.rs` — module declaration.
- `cmd/allternit-cloud-api/src/channels/teams_app.rs` — the whole cloud surface (below).
- `cmd/allternit-api/src/channel_teams_app.rs` — runtime delivery address + cloud transport + mention hook + tests.
- `cmd/allternit-api/migrations/V217__teams_app.sql` — `teams_app_connections` (owner → switchboard account row).
- `cmd/allternit-cloud-api/migrations_pg/027_teams_app.sql` — `teams_installs`, `teams_conversation_refs`,
  `teams_app_queue`, `teams_app_states`.
- `scripts/teams/build-app-package.mjs` — Teams app package (manifest.zip) generator, zero deps.

Shared files (add-only, minimal lines per CHANNELS_CONTRACTS):
- `cmd/allternit-cloud-api/src/lib.rs` — `pub mod channels;` + one `.merge(channels::teams_app::routes())`.
- `cmd/allternit-cloud-api/src/main.rs` — one `start_teams_app_worker(state.clone())` line.
- `cmd/allternit-cloud-api/src/db/migrations.rs` — one `migration!(27, "027_teams_app.sql")` line
  (note: the runner's `MIGRATIONS` list predates the 016–021 files; 027 is registered so it actually runs —
  the file itself is idempotent `CREATE TABLE IF NOT EXISTS`).
- `cmd/allternit-api/src/lib.rs` — one `pub mod channel_teams_app;` line.
- `cmd/allternit-api/src/main.rs` — one `.merge(allternit_api::channel_teams_app::teams_app_router())` line.

No changes to `channel_inbound.rs`, `channel_transports.rs` `PROVIDERS`/`build_transport`,
`route_inbound`, or any other executor's module. The existing per-user Teams transports
(outgoing-webhook HMAC / per-user Bot Framework creds) are untouched; the shared app rides the same
provider key `teams` and the same `teams_normalize`, so both lanes share threads.

## Verified API names (checked against vendor docs 2026-10-02)

- Teams app manifest schema **1.20** — `$schema` `https://developer.microsoft.com/json-schemas/teams/v1.20/MicrosoftTeams.schema.json`.
  https://learn.microsoft.com/en-us/microsoftteams/platform/resources/schema/manifest-schema
  (1.20 used in the official Microsoft Learn manifest tutorial, 2025-05).
- RSC application permissions `ChannelMessage.Read.Group` / `ChatMessage.Read.Chat` under
  `authorization.permissions.resourceSpecific` (manifest block). Same schema doc; gated behind `--rsc`.
- Bot Framework inbound JWT: OpenID metadata `GET https://login.botframework.com/v1/.well-known/openidconfiguration`,
  issuer `https://api.botframework.com`, `serviceurl` claim must match the activity's `serviceUrl`.
  https://learn.microsoft.com/en-us/azure/bot-service/rest-api/bot-framework-rest-connector-authentication
  (mirrors the already-merged `cmd/allternit-api/src/teams_auth.rs`, same constants).
- Bot token (single-tenant bot): `POST https://login.microsoftonline.com/{tenant-id}/oauth2/v2.0/token`,
  `grant_type=client_credentials`, `scope=https://api.botframework.com/.default`. Multi-tenant bots use the
  `botframework.com` tenant segment. https://learn.microsoft.com/en-us/azure/bot-service/rest-api/bot-framework-rest-connector-authentication
  (tenant-specific URL confirmed by connector docs: "for SingleTenant, the Token URL must be updated to the specific tenant ID").
- Bot Connector reply: `POST {serviceUrl}/v3/conversations/{conversationId}/activities` (activity id returned as `id`).
  https://learn.microsoft.com/en-us/azure/bot-service/rest-api/bot-framework-rest-connector-send-and-receive-activities
  (same URL shape already used by `TeamsTransport::post` in channel_transports.rs).
- Adaptive Cards in Teams: bots accept Adaptive Card attachments (`application/vnd.microsoft.card.adaptive`);
  the reply uses card version `1.4`. https://learn.microsoft.com/en-us/microsoftteams/platform/task-modules-and-cards/cards/cards-reference#adaptive-card
- Graph app catalog: `POST https://graph.microsoft.com/v1.0/appCatalogs/teamsApps` with `Content-Type: application/zip` (zip = the manifest package).
  https://learn.microsoft.com/en-us/graph/api/teamsapp-publish
- Graph per-user install: `POST https://graph.microsoft.com/v1.0/users/{user-id}/teamwork/installedApps` with body
  `{"teamsApp@odata.bind": "https://graph.microsoft.com/v1.0/appCatalogs/teamsApps/{teams-app-id}"}`.
  https://learn.microsoft.com/en-us/graph/api/userteamwork-post-installedapps
- Entra multi-tenant sign-in: `https://login.microsoftonline.com/common/oauth2/v2.0/authorize` with `response_type=code`,
  delegated `scope=openid profile`; code exchange at the `common` token endpoint; id_token issuer
  `https://login.microsoftonline.com/{tenant}/v2.0`, keys at `https://login.microsoftonline.com/common/discovery/v2.0/keys`.
  https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-auth-code-flow

## Endpoints (exact JSON)

### cloud-api — `channels/teams_app.rs`

`POST /channels/teams/messages` — the Azure Bot messaging endpoint (public; the JWT is the credential).
Validates the Bot Framework JWT at the edge, upserts the conversation reference (serviceUrl is refreshed on
every activity), queues routable activities (`message`, `messageUpdate`, `messageDelete`, `messageReaction`)
for the owning user's runtime. Always acks 200 `{"ok":true}` after the edge check.
Errors: `503 {"error":"teams_not_configured"}` (env unset), `400 {"error":"invalid_json"}`, `401 {"error":"missing_token"|"invalid_token"}`.

`POST /api/v1/channels/teams/send` — runtime/browser, Clerk session or runtime device token.
```json
// request
{ "conversationId": "19:abc@thread.tacv2", "tenantId": "optional",
  "text": "hi", "botName": "optional — Adaptive Card header",
  "replyToId": "optional", "proactive": false }
// 200
{ "id": "1700000000999", "ok": true }
// proactive while TEAMS_PROACTIVE_SEND is unset:
403 { "error": "teams_proactive_disabled", "message": "Proactive send needs the two-tenant spike; it stays off until then." }
// unknown conversation for this user: 404 conversation reference not found
```
Auth note: `resolve_caller` accepts the Clerk session / `allternit_*` API token first, then a
`Bearer allternit_runtime_…` device token (verified locally via `runtime_device_for_token`), so the
Desktop can reply in the background.

`POST /api/v1/channels/teams/register` — `{ "runtimeId": "rt_…" }` binds all of the caller's Teams
installs to that runtime (ownership-checked like `create_route`). `200 {"ok":true,"installs":n}`.
Inbound for a tenant with no registered runtime retries with backoff for 24h, then dies — messages are
not lost while the runtime is away.

`POST /api/v1/channels/teams/connect` — `200 {"url": "https://login.microsoftonline.com/common/oauth2/v2.0/authorize?…",
"state": "…", "expiresInSeconds": 600}`. State row in `teams_app_states` (10 min TTL).

`GET /channels/teams/callback?code=…&state=…` — exchanges the code, validates the id_token
(RS256, Entra v2 issuer, audience = APP_ID, 5-min skew), upserts `teams_installs(tenant → user)`,
then redirects (303) to `{ALLTERNIT_APP_URL}/settings/channels?teams=connected&tenant=…`
(errors: `?teams=error&reason=expired_state|token_exchange|no_id_token|bad_id_token|no_tenant`).

`POST /api/v1/channels/teams/catalog-upload` — `{ "graphToken": "admin delegated Graph token",
"packageBase64": "<manifest.zip>" }` → proxies `POST graph.microsoft.com/v1.0/appCatalogs/teamsApps`
(`application/zip`). Returns `{ "graphStatus": 200, "graphBody": {…Graph response…} }`.

`POST /api/v1/channels/teams/install-app` — `{ "graphToken": "…", "userId": "me|{entra user id}",
"appId": "optional, defaults to APP_ID" }` → proxies
`POST graph.microsoft.com/v1.0/users/{userId}/teamwork/installedApps` with
`{"teamsApp@odata.bind": "https://graph.microsoft.com/v1.0/appCatalogs/teamsApps/{appId}"}`.

Env: `APP_ID`, `APP_PASSWORD` (+ `TENANT_ID` for the single-tenant token endpoint; unset = `botframework.com`),
`TEAMS_PROACTIVE_SEND` (default off), `ALLTERNIT_APP_URL` (default `https://app.allternit.com`),
`ALLTERNIT_CLOUD_API_URL` (public base for the callback redirect_uri).

### allternit-api — `channel_teams_app.rs`

`POST /webhooks/teams-app` — delivery address for cloud-relayed activities (worker inserts into
`teams_app_queue`; the cloud relays here over the runtime relay with trusted headers
`x-allternit-user-id` + `x-allternit-channel-queued-at`). Not public: only the relay can set the user
header. Requires `x-allternit-user-id`, else `400 {"error":"missing_user_header"}`; `400 {"error":"invalid_json"}`.
Acks 200 immediately and runs turns in a spawned task (bot turns can outlive any delivery timeout).

The runtime-side connection: `ensure_account` upserts `teams_app_connections` (V217) and a
`provider_account_bindings` row (`vendor 'teams'`, `auth_type 'channel_oauth'`,
`external_account_id 'teams-app-shared'`, **no secret**) — the same switchboard the Messaging UI and
`set_bot_channel` already drive, so "which bot answers this Teams chat" needs no new UI machinery.

Reply path: `TeamsAppCloudTransport.post` → `POST {cloudApiUrl}/api/v1/channels/teams/send` with
`Authorization: Bearer $ALLTERNIT_CLOUD_TOKEN` (the host app provides the runtime's device token) and
`x-allternit-user-id`. Returns the Connector activity id as the receipt remote id.

## Two-tenant spike steps (for Eoj, before enabling TEAMS_PROACTIVE_SEND)

Purpose: prove or kill the assumption that the single-tenant bot in Allternit's tenant can reach a
conversation reference stored in a *second* (customer) tenant.

1. Prerequisites: an Azure Bot (`SingleTenant`, tenant = Allternit's) + multi-tenant Entra app; a second
   Entra tenant you control (e.g. a dev M365 tenant) where the manifest.zip from
   `scripts/teams/build-app-package.mjs` is uploaded (`--rsc` not needed for the spike).
2. Deploy cloud-api with `APP_ID`/`APP_PASSWORD`/`TENANT_ID` set; do NOT set `TEAMS_PROACTIVE_SEND`.
3. In the second tenant: admin installs the app for one test user (Graph `users/{id}/teamwork/installedApps`,
   or the catalog-upload + install-app routes with an admin delegated token).
4. The test user runs the connect flow (`POST /api/v1/channels/teams/connect` → sign in → callback). Check
   `teams_installs` has their tenant mapped to their Allternit user id.
5. The user's Desktop/runtime registers (`POST /api/v1/channels/teams/register`). Post any message to the
   bot in Teams → it should arrive as a thread on the runtime (reactive path — this validates inbound end-to-end).
6. **Proactive test**: with the conversation reference now stored, call
   `POST /api/v1/channels/teams/send` with `"proactive": true` and a fresh conversation id taken from a
   second chat where the bot was @-mentioned once. Watch the Bot Connector response:
   - `200/201` with an activity id → proactive works; flip the doc claim, set `TEAMS_PROACTIVE_SEND=1`.
   - `403`/`401` from `https://smba.trafficmanager.net/…` (tenant mismatch) → proactive is dead for this
     topology; keep the flag off and note that proactive requires either an RSC-gated Graph lane
     (`POST /chats/{id}/messages`) or a multi-tenant bot (blocked by Microsoft).
7. Record the outcome in `Allternit Brain/Research/specs/channel-packs.md` v3.2 notes.

## Tests

Commands (shared build cache per CHANNELS_CONTRACTS — the .gw-target lock is held by parallel
executors, so builds can wait):

```
CARGO_TARGET_DIR=/Users/joe/Desktop/allternit-workspace/.gw-target cargo test -p allternit-cloud-api --lib channels::teams_app
CARGO_TARGET_DIR=/Users/joe/Desktop/allternit-workspace/.gw-target cargo test -p allternit-api --lib channel_teams_app
node scripts/teams/build-app-package.mjs --app-id 72f988bf-86f1-41af-91ab-2d7cd011db47 --rsc --out /tmp/teams-pkg
```

Results: _filled after the runs; see git history of this file._

## What's left

- Thread-UI manual sends on shared-app threads: `channel_gateway::send` resolves the transport via
  `transport_for` → no per-user secret exists for the shared app, so those currently fail with
  "teams is not configured". The inbound reply path (dispatch_events → TeamsAppCloudTransport) is the
  supported one; wiring UI sends through the cloud transport needs owner-aware construction in the
  shared `transport_for` (deliberately not touched here).
- The config/connect wizard UI (wave 7 per CHANNELS_MAP) — the API surface it needs is all here.
- Configurable-tab channel → bot binding UI: the settings page at `${appUrl}/teams/configure` must call
  the existing switchboard; no new backend needed.
- Real brand icons in the package (placeholders today).
- Catalog upload via the admin routes is proxied with the admin's own delegated token; unattended
  upload with app permissions (client credentials) was deliberately not built (least privilege).

status: done
