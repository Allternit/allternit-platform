# Connections and credentials

**What this is:** how a vendor or channel account is connected, verified, stored and revoked.
**Who it's for:** engineers touching account bindings, keys, or the connection UI; reviewers of credential handling.
**Last verified against:** platform commit `c3af0730ca`, allternit-ai commit `88f4d6f6`.

Related: [ARCHITECTURE.md](ARCHITECTURE.md), [ADAPTERS.md](ADAPTERS.md), [AAI_REST.md](AAI_REST.md). Code: `cmd/allternit-api/src/agent_gateway_routes.rs`; UI in the web repo (`docs/gateway-ui.md`).

## Account bindings

A `provider_account_bindings` row (V198) is one connected vendor or channel account: `id, owner, vendor, auth_type, external_account_id, display_name, workspace, secret_ref, session_ref, scopes_json, restricted_bot_id, state, verified_at, expires_at`.

- **Owner-scoped.** Every row has `owner`. Another user gets 404, not 403.
- **References only.** Writes accept `secretRef` and `sessionRef`. Reads return `hasSecretRef` and `hasSessionRef`, never the value.
- **Shared or restricted.** One account can serve several bots. `restricted_bot_id` limits it to one bot (channel accounts use this: a new conversation becomes a thread only if the account is restricted to a bot).
- **Health is separate.** Account state (this page) and execution binding state (`bot_execution_bindings.state`) are tracked separately.
- **Audit.** Every state change, secret set or clear, and delete writes `connection_audit` (`event`, `from_state`, `to_state`, `actor`, `detail_json`).

## Auth types

`auth_type` is one of `oauth, browser_session, api_key, desktop_session, local_endpoint, channel_oauth, mcp_plugin`. What each means, and what the code does today:

| Auth type | Flow | Rule | Status in code |
|---|---|---|---|
| `oauth` | Vendor authorize, callback, verify | Store a revocable binding, never a password | State machine and binding exist. No vendor OAuth flow is implemented in the platform. |
| `browser_session` | User signs in inside an isolated login surface, adapter watches a logged-in probe | Never ask for passwords or session tokens | Used by the Grok and dots seeds. `session_ref` is a pointer to the local session. |
| `api_key` | User's own key, sealed, validation call | User-owned, per-bot binding allowed | Implemented end to end for Claude Managed Agents. |
| `desktop_session` | Connect the local vendor app session | Terms warning, show fragility and guarantee | Grok Bot (CDP), Claude desktop, ChatGPT app. |
| `local_endpoint` | Discover endpoint, handshake, capability probe | No cloud credential | OpenClaw. |
| `channel_oauth` | Authorize workspace, tenant or number | Show workspace or tenant identity | Channel accounts store sealed JSON secrets. See [CHANNELS.md](CHANNELS.md). |
| `mcp_plugin` | Install the Allternit extension vendor-side | Extension identity separate from account identity | Type exists. No plugin ships. |

## Connection state machine

States (`CONNECTION_STATES`): `DISCONNECTED, CONSENT_REQUIRED, AUTHENTICATING, VERIFYING, CONNECTED, DEGRADED, AUTH_FAILED, EXPIRED, REVOKED, BLOCKED`.

```
DISCONNECTED -> CONSENT_REQUIRED -> AUTHENTICATING -> VERIFYING -> CONNECTED
                     |                  |                 |          |  ^
                     v                  v                 v          v  |
                DISCONNECTED       AUTH_FAILED        DEGRADED <---> (DEGRADED)
CONNECTED  -> EXPIRED | REVOKED | BLOCKED | DEGRADED | DISCONNECTED
```

Full transition table (`connection_next`):

| From | Allowed next |
|---|---|
| `DISCONNECTED` | `CONSENT_REQUIRED` |
| `CONSENT_REQUIRED` | `AUTHENTICATING`, `DISCONNECTED` |
| `AUTHENTICATING` | `VERIFYING`, `AUTH_FAILED`, `DISCONNECTED` |
| `VERIFYING` | `CONNECTED`, `DEGRADED`, `AUTH_FAILED`, `DISCONNECTED` |
| `CONNECTED` | `EXPIRED`, `REVOKED`, `BLOCKED`, `DEGRADED`, `DISCONNECTED` |
| `DEGRADED` | `CONNECTED`, `EXPIRED`, `REVOKED`, `BLOCKED`, `DISCONNECTED` |
| `AUTH_FAILED` | `CONSENT_REQUIRED`, `AUTHENTICATING`, `DISCONNECTED` |
| `EXPIRED` | `AUTHENTICATING`, `DISCONNECTED` |
| `REVOKED` | `CONSENT_REQUIRED`, `DISCONNECTED` |
| `BLOCKED` | `AUTHENTICATING`, `DISCONNECTED` |

Moving to the same state is a no-op. An unknown state is 400. An illegal move is 409 with the allowed list. The client drives the machine one legal hop at a time (`driveAccount` and `connectionPath` in `src/lib/gateway/wizard.ts`) and reads the `allowed` list from a 409 instead of guessing.

`PATCH /gateway/provider-accounts/:id` with `state` is how the connection moves. Verified is a UI badge that needs `state` past `VERIFYING` and a `verifiedAt` value.

## Connection wizard and consent

The web wizard has 12 steps: `select_pack, method, consent, terms, authenticate, verify, discover, pick_agents, binding, create, health, ready`. `terms` is a separate step only when the connection profile carries a `termsWarning`, which every `ui_bridge` profile does. The UI gates first use of a `ui_bridge` lane on that acceptance. The server-side counterpart is the adapter's consent flag (`SUBS_GATEWAY_*_CONSENT`, see [OPERATIONS.md](OPERATIONS.md#environment-variables)): without it the adapter answers `LANE_BLOCKED` and never launches or attaches to the vendor app.

Discovery calls `GET /gateway/provider-accounts/:id/agents`, which runs `agent.list` through a transient binding and attaches the sealed key only if the account has one.

## Sealed user keys

`POST /gateway/provider-accounts/:id/secret` with `{ apiKey }`:

- Trims and rejects an empty key (400).
- Seals with `token_crypto` (AES-256-GCM, the mechanism `aci_credentials` uses). The stored value starts `enc:v1:`.
- **Strict:** if no encryption key is available the key is not stored and the route returns 503. There is no `plain:` fallback.
- Stores the sealed value in `secret_ref`, audits `secret_set`, and returns the account without the key.
- `DELETE .../secret` clears it.

The key source is `ALLTERNIT_ENCRYPTION_KEY`, falling back to `ENCRYPTION_KEY`, then a platform key file at the path in `ALLTERNIT_PLATFORM_KEY_FILE` (`token_crypto.rs`).

Use at call time: `gateway_runner` unseals the key just now and sends it to the gateway as a top-level `credential` on `POST /aai/call`. The gateway holds it in `AsyncLocalStorage` for that call only. An `api_key` account with no usable key fails the call fast with `AUTH_REQUIRED`, and the binding moves to `NEEDS_AUTH`. The web API key panel never keeps the key after save (`ConnectionWizard.tsx`), and the SDKs never echo it.

The gateway never stores a credential. UI-bridge adapters never read a vendor login, cookie or token.

## Kill switch

Two levels:

- **Per account, from the UI:** `ConnectionCard` "Kill switch" patches every dependent bot's execution binding to `DISABLED` with reason `kill switch` (`PATCH /gateway/bots/:bot_id/execution-binding`). Threads stay. A `DISABLED` binding never falls back to a native brain.
- **Per adapter, in the gateway:** `SUBS_GATEWAY_AAI_DISABLED` (comma-separated adapter ids) or `disabled: true` in the registration. A disabled adapter answers `LANE_BLOCKED` for every operation except `list, get, capabilities, identity, health, events, contextCancel, contextClose`. There is no route to toggle it at runtime; `AaiHost.setDisabled` exists in code but no HTTP route calls it, so changing it means a gateway restart.

The runner also disables a binding itself on `LANE_BLOCKED` (bot detection or account risk). See the failure table in [ARCHITECTURE.md](ARCHITECTURE.md#8-failure-table).

## Revocation cascade

`cascade_needs_auth` runs when an account moves to `REVOKED` or `EXPIRED`, and on account delete. Every execution binding pointing at the account moves to `NEEDS_AUTH` if its current state allows that move. Bindings in `NEEDS_AUTH`, `UNBOUND`, `DISABLED` or `FAILED` stay where they are. Threads are never deleted, and the affected bots show Needs attention.

`DELETE /gateway/provider-accounts/:id`:

- With dependent bots and no `?force=true`: 409 with `dependentBots` (the UI lists them before disconnect).
- With `?force=true`: cascades dependents to `NEEDS_AUTH`, nulls `account_binding_id` on them, deletes the account, audits `account.deleted` with the bots moved.

Execution binding state changes land on the bot ledger with `bot_id`, so the cascade is visible in the activity trail.

## Not built

- No vendor OAuth flow, so `oauth` accounts can only be moved through states by a caller.
- Vendor memory is a separate partition under the Bot, and promotion into native memory is explicit. The web UI exists (allternit-ai PR #282: `VendorMemoryPartition.tsx`, calling `GET /gateway/bots/:id/vendor-memory` and `POST .../vendor-memory/:recordId/promote`). Those backend endpoints are **not on `gateway/integration` at this commit**. They are in review on the platform `gateway/accept-rust` branch. Until it merges the panel has no server to talk to, and [ACCEPTANCE.md](ACCEPTANCE.md) still records the line as blocked.
- Terms acceptance is a UI step. The server does not store who accepted which terms version.
