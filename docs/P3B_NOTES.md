# P3b — MCP App directory backend (allternit-api)

Branch `p3b-directory-backend`. Client contract: `wt-p3-directory/src/lib/ai/mcp/directory/*` (read-only, unchanged).
Nothing pushed, merged, deployed, or migrated against any real DB.

## Files
- `cmd/allternit-api/migrations/V198__mcp_app_directory.sql` — written, NOT applied (tables: `developer_domain_tokens`,
  `directory_submissions`, `directory_reviewer_credentials`, `mcp_app_installs`). Only exercised on throwaway temp DBs by tests.
- `src/mcp_directory_guard.rs` — SSRF guard, pinned-address fetch, domain challenge check.
- `src/mcp_directory_held.rs` — review lifecycle + held-update state machine (Rust port of `submission.ts` / `held-update.ts`).
- `src/mcp_directory_routes.rs` — handlers, storage, installs, connector OAuth start, CIMD doc.
- Wiring: `lib.rs` (3 `pub mod`), `main.rs` (directory router merged into the authed router at `/v1/...` and `/api/v1/...`;
  `/oauth/client.json` on the public router), `mcp_routes.rs` (**one route** added: `POST /connectors/:id/oauth/start`).

## Endpoints (JSON camelCase, matching `directory-api.ts`)
| Route | Auth | Notes |
|---|---|---|
| `POST /v1/developers/domain-tokens` `{host}` → `{token}` | user | token shown once; SHA-256 stored; re-issue replaces + un-verifies |
| `POST /v1/developers/domain-tokens/check` `{host}` → `{ok,url,reason?}` | user | server-side fetch of `https://<host>/.well-known/allternit-apps-challenge` |
| `POST /v1/miniapps/submissions` → `SubmissionRecord` (201) | user | body = `AppSubmission` + `packageZip` (+ optional `appId`, `snapshot`) |
| `GET /v1/miniapps/submissions[?scope=all]` → `{items}` | user (`all`: admin) | owner-scoped; never includes credentials or package bytes |
| `GET /v1/miniapps/submissions/:id` | owner/admin | others get 404 |
| `POST /v1/miniapps/submissions/:id/review` `{status: approve\|reject, notes}` | admin | directory-side verdict; reject needs a reason |
| `GET /v1/miniapps/submissions/:id/reviewer-credentials` | admin | the only read path for credentials |
| `POST /v1/miniapps/:id/publish` | owner/admin | only from `approved` (409 otherwise) |
| `POST /v1/miniapps/:id/held/approve` | admin | promotes pending form/package + candidate snapshot |
| `GET/PUT /v1/mcp-app-installs`, `PATCH/DELETE /v1/mcp-app-installs/:appId` | user | scoped by `user_id`; connector must be the caller's |
| `POST /mcp/connectors/:id/oauth/start` → `{authorize_url}` | user | PKCE S256 + `resource`; session row in `mcp_oauth_sessions` |
| `GET /oauth/client.json` | public | CIMD: `client_id` = own URL, `redirect_uris` = `<ALLTERNIT_PUBLIC_BASE_URL>/mcp/oauth/callback`, auth method `none` |

## Design decisions
- **SSRF**: https only, default port only, no IP literals (any notation), name screen (`localhost/.local/.internal/...`), DNS resolved
  server-side and *every* answer must be public (v4 private/CGNAT/reserved/test-nets, v6 loopback/ULA/link-local/mapped/NAT64/6to4),
  connection pinned to the validated address (no rebinding), redirects never followed (stricter than "no cross-host"), 5 s timeout,
  4 KiB body cap, `Content-Type: text/plain`, exact body match (one trailing newline tolerated). The same guard covers OAuth discovery fetches.
- **Tokens**: per developer *and* per host, hash-only at rest; the check compares SHA-256(fetched body) to the stored hash.
- **Reviewer credentials**: separate table, sealed with the existing `token_crypto` (AES-256-GCM when a key is configured), no list/get query touches it.
- **Held updates**: approved snapshot + candidate snapshot persisted; an update to an approved/published listing leaves the live form,
  package and snapshot untouched until `/held/approve`. Removals narrow scope and apply immediately. Publish is allowed for the owning
  developer (matches the client's "publish when ready") *or* an admin; review/held-approve are admin-only.
- **Permission modes**: stored as the client's values (`always_ask | ask_before_changes | ask_before_important_changes`); the short forms in
  the task text (`always | before_changes | before_important`) are accepted and normalised.
- When the auth server supports CIMD and the connector has no client configured, `oauth_client_id` on the connector row is set to our CIMD URL
  so the existing callback's token exchange presents the same public client (no callback edit needed).

## Commands and results
- `cargo test -p allternit-api --lib mcp_directory` → **50 passed, 0 failed** (guard 13, held state machine 12, routes/storage/installs/OAuth 25).
  Covers: SSRF guard, domain check, held-update state machine, install scoping, credential isolation, token scoping, PKCE (RFC 7636 vector), CIMD doc.
- `cargo check -p allternit-api --all-targets` → Finished, no errors (pre-existing warnings only).
- Not run: full `cargo test -p allternit-api` (unrelated suites; ~1.5k tests), any server start, any live HTTP/DNS.

## Cuts / needs follow-up
1. **Client change needed**: `submitForReview` must also send `snapshot` (`ListingSnapshot` from the scanner). Without it the server stores the
   submission with a `snapshot.missing` warning and cannot diff tools for that version. The server does not unzip `packageZip` or call the MCP server.
2. **Registry linkage**: `GET /v1/miniapps` and `POST /v1/miniapps/:id/review` live in `services/registry/apps-registry`. This work does not sync
   with that store; directory state lives in `directory_submissions` and has its own admin verdict route (`submissions/:id/review`).
3. **No "reject held update"** and no unpublish endpoint (not requested; state machine supports `unpublish`, no route).
4. **Callback gaps left alone** (mcp_routes.rs kept to one route as instructed, other agent owns it): `mcp_oauth_callback`'s token exchange does not send
   `redirect_uri`, and stores tokens unencrypted in `mcp_oauth_sessions.tokens`; strict servers may reject the exchange. Worth a follow-up on p1.
5. OAuth discovery refuses non-443 ports and non-public hosts, so local/dev connectors cannot use `oauth/start` (no dev bypass added; fail closed).
6. Router nesting under `/api` and `/v1` was validated only by constructing the router (no conflicts) — not by booting the server.
7. Existing localStorage installs are not migrated; the client must switch to the new endpoints.
8. Migration is written, not applied; tests ran it on temp SQLite files only.

status: done
