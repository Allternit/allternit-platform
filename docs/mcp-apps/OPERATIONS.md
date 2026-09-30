# MCP Apps — operations

Config, deploy and migration steps for the MCP Apps backend. Design is in [ARCHITECTURE.md](ARCHITECTURE.md).

> **Nothing here has been applied to production.** Migrations V198 and V199 are written and tested on temporary SQLite
> files only. Production migrations are run manually. `seal_legacy_mcp_secrets()` has not been run against any database.

## Environment variables

Read by `allternit-api` (found by grepping `env::var` in the files these features touch).

| Variable | Read in | Meaning / default |
|---|---|---|
| `ALLTERNIT_MCP_PROXY_SECRET` | `mcp_user_proxy.rs` | HMAC key for per-user proxy tokens. **Unset = a random secret per process.** Set it (same value everywhere) when running more than one API replica, or a token minted by one replica fails on another (401). Also resets on restart. |
| `ALLTERNIT_MCP_PROXY_URL` | `mcp_user_proxy.rs` | URL gizzi uses to reach the proxy. Default `http://127.0.0.1:<api_port>/mcp/user-proxy` (port 8013 if config is unset). Set it when gizzi is not on the API host. |
| `ALLTERNIT_DIRECTORY_REVIEW_ORG_ID` | `mcp_directory_routes.rs` | Clerk org id whose admins may review submissions. **Unset = nobody can review** (review, credentials, held-approve and `scope=all` return 403). |
| `ALLTERNIT_MCP_ALLOW_PRIVATE_CONNECTORS` | `mcp_apps.rs` | Dev flag. `1`, `true` or `yes` lets connector URLs resolve to loopback/private addresses. Do not set in production: it turns off the SSRF check for the bridge, the proxy and OAuth calls. |
| `ALLTERNIT_PUBLIC_BASE_URL` | `mcp_directory_routes.rs` | Public API origin for the CIMD document and the OAuth `redirect_uri` (`<base>/mcp/oauth/callback`, a public route: no Clerk token on the browser redirect). Default `http://127.0.0.1:8013`. |
| `MCP_PUBLIC_URL` | `mcp_agents.rs` | Public URL of the Agents MCP server; the expected `aud`. Default `https://mcp.allternit.com/mcp`. |
| `MCP_OAUTH_ISSUER` | `mcp_agents.rs` | Authorization server advertised in protected-resource metadata. Default: the Clerk proxy issuer `https://allternit.com/__clerk`. |
| `CLERK_JWKS_URL`, `CLERK_ISSUER` | `auth.rs` | JWT verification. Defaults `https://clerk.allternit.com/.well-known/jwks.json` and `https://clerk.allternit.com`. |
| `ALLTERNIT_ENCRYPTION_KEY` (or `ENCRYPTION_KEY`) | `token_crypto.rs` | Key for sealing connector tokens. With neither, a key file is read (`ALLTERNIT_PLATFORM_KEY_FILE`, else `connector-encryption.key` in `ALLTERNIT_DATA_DIR` or the platform data dir). With no key at all, values are stored as `plain:`. |
| `ALLTERNIT_COMMERCE_STRIPE_SECRET_KEY` | `commerce.rs` | Must be `sk_test_`/`rk_test_`. Unset = commerce off. |
| `ALLTERNIT_COMMERCE_STRIPE_PUBLISHABLE_KEY` | `commerce.rs` | Must be `pk_test_`. |
| `ALLTERNIT_COMMERCE_STRIPE_WEBHOOK_SECRET` | `commerce.rs` | Verifies `POST /webhooks/stripe-commerce`; there is no unsigned fallback. |
| `ALLTERNIT_COMMERCE_OPERATOR_USER_IDS` | `commerce.rs` | Comma-separated user ids allowed to refund. |
| `ALLTERNIT_COMMERCE_PLATFORM_FEE_BPS`, `..._FIXED_MINOR` | `commerce.rs` | Platform fee; both default 0. |

Desktop-only SIWC settings are in [SIWC.md](SIWC.md).

## Sandbox deploy

The sandbox origin is `https://mcp-sandbox.gizziio.com`, served by the Cloudflare Pages project `allternit-mcp-sandbox`.
Deploy only three files: `sandbox.html`, `sandbox.js`, `_headers`. Nothing else goes in that project, so the sandbox origin
holds no other content. These files and the Pages project are not in this repo; this section comes from the program
handoff, not from code checked here. Per-app wildcard sandboxes (`*.mcp-sandbox.gizziio.com`) are not set up.

## Clerk settings

Set in the Clerk dashboard (not checkable from this repo):

- Client ID Metadata Documents (CIMD): on.
- Include Audience in access tokens: on. The Agents server rejects a token whose `aud` is not `MCP_PUBLIC_URL`.
- JWT access tokens (not opaque).
- PKCE required.
- OAuth scope `agents:read` created (the Agents server requires it).

## Migrations

Both are embedded refinery migrations in `cmd/allternit-api/migrations/`, and run when an API process next opens a DB.
Production migrations are run manually, so **run these on purpose, not by starting a new build against production.**

- **V198 `mcp_app_directory`** (directory): `developer_domain_tokens`, `directory_submissions`,
  `directory_reviewer_credentials`, `mcp_app_installs`. All `CREATE ... IF NOT EXISTS`.
- **V199 `commerce_orders`** (commerce): `commerce_connected_accounts`, `commerce_checkout_sessions`, `commerce_orders`,
  `commerce_disputes`, `commerce_webhook_events`. Renumbered from V198 after the directory took V198.

Check the migration history of the target DB for a clash before applying.

### `seal_legacy_mcp_secrets()`

`mcp_routes.rs::seal_legacy_mcp_secrets(&Connection)` seals pre-existing plaintext `mcp_oauth_sessions.tokens` and
`mcp_connectors.oauth_client_secret`. It is idempotent and returns how many values it sealed. **It is not called at startup
or from any CLI**, so running it is a manual operator step. New writes are already sealed; the read path accepts both forms.
Configure an encryption key first: with no key it would write `plain:` and nothing is protected.

## Other deploy items

- Stripe: register `https://<api>/webhooks/stripe-commerce` for `account.updated` and `charge.dispute.*`, and enable Connect.
- Edge: `mcp.allternit.com/mcp` → `/mcp/server` routing is not in this repo.
- Gateway: `/api/mcp/apps` and `/api/mcp/sandbox` resolve through the existing `/api` prefix; no registry change.
- With commerce misconfigured (a live key), the API still starts: commerce routes return 503.
