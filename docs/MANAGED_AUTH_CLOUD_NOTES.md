# Managed connector auth — cloud-api notes (WP1, WP3 edge part, WP4)

Factory node n_4022 (dag_577250). Branch `feat/managed-auth-cloud`. Spec: `docs/MANAGED_AUTH_MAP.md`.

## What was built

### WP1: Allternit as an ID-JAG issuer
- ES256 signing/verification, hand-rolled on `aws-lc-rs` like `dataplane_jwt.rs` (no new crate). Key: PEM in `ALLTERNIT_IDJAG_ES256_PEM` (PKCS#8 or SEC1, `\n` escapes accepted). `kid` = b64url SHA-256 of the public point. Unset or unparsable key ⇒ every issuer and edge-AS route answers 503 `{"error":{"code":"idjag_not_configured"}}`.
- Issuer per Clerk org: `<ALLTERNIT_IDJAG_ISSUER_BASE>/oauth2/org/<org>` (default base `https://api.allternit.com`).
- Issuance rule (`managed_auth::decide`): member of the org (checked first, so non-members learn nothing about configs) → enabled `managed_auth_configs` row → member role in `roles` (`admin` ≡ `org:admin`) → primary email verified in Clerk. Every issuance is written to `idjag_issuance_log` before signing; if the write fails nothing is signed.
- ID-JAG: header `{alg: ES256, typ: oauth-id-jag+jwt, kid}`; claims `iss, sub, email, email_verified: true, aud (vendor AS issuer), client_id, resource, scope, jti, iat, exp = iat + 300`.
- Vendor client secrets sealed with the credential cipher (`ALLTERNIT_CREDENTIALS_KEY`). Without the cipher a PUT that carries a secret is refused (503 `secrets_unavailable`): never stored in plaintext. Never logged, never returned to a browser (`has_secret: bool` only).

### WP3 (edge part): connector gateway
- `Target::Connectors`: `GET|POST /mcp/connectors`, resource `<MCP_PUBLIC_URL>/connectors`, scope `connectors:use`, approval target `connectors`, relay path `/webhooks/mcp-edge/connectors`, RFC 9728 doc at `/.well-known/oauth-protected-resource/mcp/connectors`.
- Same relay, signature, offline/warming handling and 25 s budget as bots. Like the agents server, it goes to the user's best runtime (connected first).
- `mcp_oauth_approvals` accepts target `connectors` (so a Clerk OAuth client can be approved for the gateway) and refuses to store a per-user approval for an `issuer:` client.
- Events: the gateway advertises none; `events/*` is answered at the edge with METHOD_NOT_FOUND and never relayed. `server/discover` answers from a local `allternit-connectors` spec (tools only).
- Human page on the MCP host lists the gateway URL.

### WP4: trusted issuers + edge authorization server
- Edge AS issuer `<ALLTERNIT_IDJAG_ISSUER_BASE>/oauth2/edge`: `POST /oauth2/edge/token` (RFC 7523 JWT-bearer), metadata at `/oauth2/edge/.well-known/oauth-authorization-server` (+ `openid-configuration`) and the RFC 8414 path `/.well-known/oauth-authorization-server/oauth2/edge`, JWKS at `/oauth2/edge/jwks.json`.
- Assertion checks, in order: `typ` = `oauth-id-jag+jwt`; `iss` trusted by ≥1 org; signature checks against **that org's** JWKS for the issuer (row `jwks_uri`, else discovered from the issuer's AS/OIDC metadata whose `issuer` must match; cached 1 h; an unknown `kid` refetches, at most once a minute); `aud` = edge AS issuer; `resource` is the agents server or the connector gateway (never a bot) and matches the request's `resource`; `iat`/`exp` sane (lifetime ≤ 1 h, 60 s leeway); `jti` present; the vouching org allowed that target; `email` present and not marked `email_verified: false`; `jti` new (`edge_jti_seen`, key `<iss>|<jti>`, kept until `exp`).
- User mapping (`edge_auth::pick_user`): Clerk users holding the email as a **verified** address (`GET /v1/users?email_address=`) who are members of a vouching org. Exactly one, else `invalid_grant`. Clerk failure ⇒ 503 `temporarily_unavailable`, never a guess.
- Edge access token: ES256, `typ: at+jwt`, `iss` = edge AS, `aud` = resource, `scope` = target scope, `sub` = Clerk user id, `client_id` = `issuer:<iss>`, `org_id`, `jti`, 1 h. `EdgeBackend::verify` checks it with our key when its (unverified) `iss` is the edge AS, and never sends it to Clerk; every other token goes to Clerk exactly as before (`check_claims` unchanged for Clerk).
- Org consent: an edge token's client is approved per call by `edge_auth::org_consent` (an active `mcp_trusted_issuers` row for that issuer and target, in an org the user still belongs to). A personal approval row never substitutes. Refusal: 403 `org_consent_required` in plain words.
- PRM `authorization_servers`: Clerk first, then the edge AS, only when the ES256 key is set, and never on a bot's document.
- Outbound fetches for issuer metadata/JWKS: https only, public hosts only (no loopback, private, link-local, CGNAT; no redirects), 10 s timeout. An org admin cannot point cloud-api at its own network.

## Endpoints

| Method | Path | Auth |
|---|---|---|
| GET | `/oauth2/org/:org/.well-known/oauth-authorization-server`, `…/openid-configuration` | public |
| GET | `/.well-known/oauth-authorization-server/oauth2/org/:org`, `/.well-known/openid-configuration/oauth2/org/:org` | public |
| GET | `/oauth2/org/:org/jwks.json` | public |
| POST | `/oauth2/org/:org/token` (RFC 8693; form or JSON) | Clerk session JWT as `subject_token` |
| GET | `/api/v1/managed-auth/configs` | runtime device credential |
| POST | `/api/v1/managed-auth/id-jag` | runtime device credential |
| GET | `/api/v1/orgs/:org/managed-auth` (+ `issuer_url`, `issuer_configured`, `is_admin`) | Clerk session, member |
| GET/PUT/DELETE | `/api/v1/orgs/:org/managed-auth/:connector_id` | Clerk session; PUT/DELETE need `org:admin` |
| GET | `/api/v1/orgs/:org/trusted-issuers` (+ `edge_issuer`, `is_admin`) | Clerk session, member |
| POST | `/api/v1/orgs/:org/trusted-issuers` | Clerk session, `org:admin` |
| DELETE | `/api/v1/orgs/:org/trusted-issuers/:id` | Clerk session, `org:admin` |
| POST | `/oauth2/edge/token` | ID-JAG assertion |
| GET | `/oauth2/edge/.well-known/oauth-authorization-server`, `/.well-known/oauth-authorization-server/oauth2/edge`, `/oauth2/edge/jwks.json` | public |
| GET/POST | `/mcp/connectors` | Clerk OAuth (+ owner approval `connectors`) or edge token (+ org consent) |
| GET | `/.well-known/oauth-protected-resource/mcp/connectors` | public |

**Runtime credential** (contract: "the same one allternit-api already uses"): both forms of the runtime device credential are accepted. (1) The relay signature allternit-api already signs its cloud calls with (`x-allternit-runtime-id`, `x-allternit-runtime-ts`, `x-allternit-runtime-sig` over `METHOD.path.sha256(body)`; `runtime_events::authenticate_runtime`, as used by `mcp_events_client.rs` / `floor_routes.rs`); sign `GET /api/v1/managed-auth/configs` with an empty body. (2) `Authorization: Bearer allternit_runtime_…` (`runtime_pairing::runtime_device_for_token`). Both resolve to the device's Clerk owner.

**Admin role**: the session's own `o.rol` when the session's active org is `:org`; otherwise Clerk's membership list (`GET /v1/users/:id/organization_memberships`, cached 5 min). Clerk unreachable ⇒ 503, non-member ⇒ 403 `not_member`, non-admin write ⇒ 403 `admin_required`. Error body everywhere outside the OAuth endpoints: `{ "error": { "code", "message" } }`. OAuth endpoints use RFC 6749 `{error, error_description}`.

## Files

- `cmd/allternit-cloud-api/migrations_pg/117_managed_connector_auth.sql` (new): `managed_auth_configs`, `idjag_issuance_log`, `mcp_trusted_issuers` (partial unique on active `(org_id, issuer)`), `edge_jti_seen`. Idempotent (`IF NOT EXISTS`) like the rest of `migrations_pg`, which `init_db` applies at startup (`ALLTERNIT_SKIP_MIGRATIONS=1` opts out). Header says "renumber at merge if main has moved past 116", per convention.
- `src/auth/es256.rs` (new): key, JWK/JWKS, sign, ES256/RS256 verify against a JWK set.
- `src/routes/managed_auth.rs` (new): WP1 issuer, admin API, runtime endpoints, `org_caller`.
- `src/routes/edge_auth.rs` (new): WP4 edge AS, trusted issuers, JWKS cache, org consent, edge-token verification.
- `src/routes/mcp_edge.rs`: `Target::Connectors`, edge-token acceptance + org consent, PRM, connectors spec, human page.
- `src/routes/mcp_oauth_approvals.rs`: target `connectors`; no per-user approvals for `issuer:` clients.
- `src/services/user_trust.rs`: `clerk_org_memberships_checked` (None on Clerk failure, so grants fail closed; the old best-effort fn is a wrapper, unchanged behaviour), `clerk_primary_email_status`, `clerk_users_with_verified_email`.
- `src/services/account_erase.rs`: account deletion erases the user's `idjag_issuance_log` rows; `created_by` on the org-owned `managed_auth_configs` / `mcp_trusted_issuers` rows is scrubbed to `'deleted'` (the org keeps its settings). Caught by the existing "every table with a person is classified" test.
- `src/lib.rs`: routes merged into the public, rate-limited router; `migrations_pg_applies_cleanly` now honours `TEST_DATABASE_URL` like `test_support` (it hard-coded `localhost:5432`).
- `src/routes/mod.rs`, `src/auth/mod.rs`: module wiring.

## Env

| Var | Default | Meaning |
|---|---|---|
| `ALLTERNIT_IDJAG_ES256_PEM` | unset ⇒ 503 | P-256 private key (PEM). One key signs org ID-JAGs and edge access tokens (different `iss`/`aud`/`typ`, all checked). Generate: `openssl ecparam -name prime256v1 -genkey -noout \| openssl pkcs8 -topk8 -nocrypt`. |
| `ALLTERNIT_IDJAG_ISSUER_BASE` | `https://api.allternit.com` | Base of both issuers. |
| `CLERK_SECRET_KEY`, `CLERK_API_BASE` | existing | Membership, primary-email and email→user lookups. |
| `ALLTERNIT_CREDENTIALS_KEY` | existing | Seals vendor client secrets. |
| `MCP_PUBLIC_URL` | existing | Edge resources; the edge AS refuses every resource while unset. |

## Tests run (all on this branch, `CARGO_TARGET_DIR` = the tree's own `target/`)

- `cargo check -p allternit-cloud-api --tests`: clean, no new warnings. `cargo clippy -p allternit-cloud-api --tests`: no warnings in the new/changed code (pre-existing warnings elsewhere untouched).
- `cargo test -p allternit-cloud-api --lib -- managed_auth edge_auth es256 mcp_edge mcp_oauth_approvals user_trust::tests::only_users` against a throwaway local Postgres 16 (`TEST_DATABASE_URL`): **56 passed, 0 failed** (59 with the account-erase checks). All pre-existing `mcp_edge` bot/agents/key/events tests stay green.
  - New: ID-JAG mint (header `typ`/`alg`/`kid`, claims, exp ≤ 300 s); refusals (non-member, disabled/missing config, role not allowed, unverified/missing primary email; non-member checked first); scope subset; admin input validation; ES256 sign/verify, wrong key, forged `kid`, tampered payload, `none`/HS256 refused; assertion checks (wrong `typ`, wrong `aud`, resource mismatch, expired, over-long lifetime, missing `jti`); user mapping 0 / 1 / 2 matches and holder outside the org; `email_verified: false`; edge token verify (forged key, expired, ID-JAG-typed); trusted-issuer input incl. SSRF hosts; `Target::Connectors` routing (best runtime, own scope/audience, cross-target refusals, own approval), events not relayed, metadata + well-known path; edge token needs org consent and a personal approval does not count; full `create_router` builds (axum duplicate-route panic guard); issuer/edge routes 503 without the key and serve metadata/JWKS with it through the real router.
- `migrations_pg_applies_cleanly` on a fresh database, run twice (idempotent): passed; `_sqlx_migrations` shows 117 applied.
- The new SQL (config upsert keeping the secret when omitted, partial-index `ON CONFLICT` for issuers, replay insert returning 0 rows the second time, `ANY($1)` lookups, issuance log) was run with `PREPARE`/`EXECUTE` against the migrated schema: behaves as intended.
- Full `cargo test -p allternit-cloud-api --lib` (scratch Postgres 16 on localhost:5432, since many older tests hard-code that URL): **1100 passed, 1 failed, 2 ignored**. The failure is `contabo_runtime_service::tests::provision_creates_container_and_instance_record`: "docker run failed: No such file or directory" (no `docker` binary on this machine; file not touched by this change). The first full run also caught the new `idjag_issuance_log.user_id` missing from `account_erase` classification; fixed (see Files).

## Deviations

1. **Several orgs manage the same connector** (runtime `POST /id-jag` without `org_id`): the spec doesn't say; never guessing, it answers `not_configured` with the message "More than one of your organizations manages this connector; pass org_id." No new error code was added to the contract.
2. **One ES256 key for both issuers.** The spec names one env var; tokens can't be confused (`iss`, `aud`, `typ` are all checked by each verifier). A separate edge key can be added later without a schema change.
3. **Outside issuers may sign RS256** (Okta's default) as well as ES256. Never `none`/HMAC.
4. **The edge AS is listed in PRM only when the key is set** (an unset key would advertise a server that answers 503) and never in a bot's document (bots stay per-user).
5. **The connector gateway has no MCP Events**: the event catalog has no connector audience, and relaying `events/*` would let the runtime answer for subscriptions the edge owns. Answered at the edge as METHOD_NOT_FOUND.
6. **Trusted issuer URLs must be public https** (stricter than the spec): no loopback/private/link-local hosts and no redirects on metadata/JWKS fetches, to stop an org admin from making cloud-api fetch internal addresses. Managed-auth config URLs (fetched by the runtime, not cloud-api) also allow `http://localhost` so a local test AS works.
7. **A bad assertion's `jti` is recorded only after signature and claim checks pass**, before the user mapping; a mapping refusal still burns the `jti` (safe side).
8. **Edge access tokens carry `org_id`** (the vouching org) but consent is re-checked on every call against the live issuer row and Clerk membership, so removing an issuer or a member takes effect at once (membership cache: up to 5 min).
9. The vouching org's JWKS binds the org: an org admin who adds an issuer with their own `jwks_uri` can only vouch for members of **their own** org, never another org that trusts the same `iss`.

## Not verified / left for others

- **Live Clerk calls** (`/v1/users?email_address=`, memberships, primary email) are not exercised against real Clerk; the response parsing is unit-tested on Clerk-shaped bodies.
- **Claude-issued ID-JAGs**: Claude's exact claims and JWKS location are not public. The code follows the MCP extension spec and keeps issuer-specific handling in `edge_auth::assertion_email`. Treat Claude as verified only after the Claude Connect-step guide is read and checked against it.
- **End-to-end with a real vendor AS** (spec acceptance "local end-to-end") needs WP2 (allternit-api `managed` auth kind) and is the orchestrator's integration step.
- **Runtime handler for `/webhooks/mcp-edge/connectors`** is WP3's allternit-api part (another executor). Until it lands, the gateway relays and the runtime answers its usual 404.
- **UI (WP5), docs (WP7), dependency-map `features.json`**: other work packages / the orchestrator; not touched here to avoid conflicting edits.
- No deploy, no prod migration, no PR (per task rules).

## Blocker: not committed or pushed

All changes are **staged but not committed**. The steering commit gate (`.steering/bin/steer-pre-commit-gate.sh`) refused every `git commit`. Its consult (`allternit-factory orchestration steer consult`) answers with an empty `text:` body, and the gate reads that as STEER with no reason (`.steering/state/consults.log`: `verdict=STEER`). I did not bypass the gate (`.steering/off` / `STEER_CONSULT_CMD`): that is the owner's call. Once the consult works or the owner allows a bypass, commit the staged tree as "feat(cloud): managed connector auth — ID-JAG issuer, connector gateway, trusted issuers" (with the Co-Authored-By line) and push `feat/managed-auth-cloud`. The sentinel is deliberately **not** created until that commit and push exist.
