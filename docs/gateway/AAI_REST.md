# AAI REST reference

Generated from the route tables in `cmd/allternit-api/src/{agent_gateway_routes,gateway_runner,thread_routes,agent_session_routes}.rs`. All paths are under `/api/v1`; all require `Authorization: Bearer <token>` and are owner-scoped (a resource you do not own returns 404). Bodies use the field names the handlers deserialize (mostly `snake_case`; responses are camelCase).

SDKs: TypeScript [`@allternit/aai-sdk`](../../platform/packages/aai-sdk/README.md), Python [`allternit-aai`](../../platform/python/allternit-aai/README.md).

## Error shape

`{ "error": string, "code"?: string, "retryAfterMs"?: number|null, "approvalId"?: string|null }`

| Status | Meaning | SDK error |
|---|---|---|
| 400 | Bad body or decision | `AaiHttpError` |
| 401 / 403 | Auth required/revoked; lane blocked; `HUMAN_REQUIRED` (bot tried to answer an approval) | `AaiHttpError` |
| 404 | Not found or not owned | `AaiHttpError` |
| 409 | `BINDING_NOT_READY`, `REMOTE_CLOSED`, `CONTEXT_LOST`, `CONTEXT_LOST_RESUMABLE`, `ALREADY_RESOLVED`, frozen-field or duplicate-generation conflicts | `ConflictError` |
| 428 | `APPROVAL_REQUIRED` (has `approvalId`) | `ApprovalRequiredError` |
| 429 | `RATE_LIMITED` (has `retryAfterMs`) | `RateLimitedError` |
| 502 / 503 | Vendor failure; `GATEWAY_OFFLINE` | `AaiHttpError` |

## Provider accounts (`/gateway`)

| Method | Path | Body | Response | Errors |
|---|---|---|---|---|
| POST | `/gateway/provider-accounts` | `vendor, auth_type` (oauth, browser_session, api_key, desktop_session, local_endpoint, channel_oauth, mcp_plugin), optional `external_account_id, display_name, workspace, secret_ref, session_ref, scopes[], restricted_bot_id, expires_at` | 201 `{account}` | 400, 403 |
| GET | `/gateway/provider-accounts?vendor=&state=` | | `{accounts}` | |
| GET | `/gateway/provider-accounts/:id` | | `{account}` | 404 |
| PATCH | `/gateway/provider-accounts/:id` | `state, display_name, workspace, external_account_id, expires_at, verified_at, reason` (state follows the connection state machine) | `{account}` | 400, 404, 409 |
| DELETE | `/gateway/provider-accounts/:id?force=` | | ok | 409 if bound |
| POST | `/gateway/provider-accounts/:id/secret` | `api_key` (sealed AES-256-GCM, never echoed) | ok | 503 no encryption key |
| DELETE | `/gateway/provider-accounts/:id/secret` | | ok | |
| GET | `/gateway/provider-accounts/:id/agents` | | `{agents:[{externalAgentId,name,description?,avatarUrl?}]}` | 401, 403, 429, 502 |

## Execution bindings

| Method | Path | Body | Response | Errors |
|---|---|---|---|---|
| GET | `/gateway/execution-bindings` | | `{bindings}` | |
| PUT | `/gateway/bots/:bot_id/execution-binding` | `type, mode, vendor, adapter_id, account_binding_id, preferred_lane, external_agent_id, capabilities, health` | 200/201 `{binding}` | 403, 400 |
| GET | `/gateway/bots/:bot_id/execution-binding` | | `{binding}` | 404 |
| PATCH | `/gateway/bots/:bot_id/execution-binding` | `state, health, reason` | `{binding}` | 400, 404 |

## Remote thread and channel bindings

| Method | Path | Body | Response | Errors |
|---|---|---|---|---|
| POST | `/gateway/threads/:thread_id/remote-bindings` | `generation, execution_binding_id, external_context_id, external_task_id, lane, capability_snapshot` | 201 `{binding}` | 409 generation exists |
| GET | `/gateway/threads/:thread_id/remote-bindings` | | `{bindings}` | |
| PATCH | `/gateway/remote-bindings/:id` | state/ids (lane and capabilitySnapshot are frozen) | `{binding}` | 409 |
| POST | `/gateway/threads/:thread_id/channel-bindings` | `provider, external_conversation_id`, optional `account_binding_id, external_workspace_id, external_channel_id, external_thread_id, canonical_url, bidirectional, read_only, posting_identity_id` | 201 `{binding}` | 400 |
| GET | `/gateway/threads/:thread_id/channel-bindings` | | `{bindings}` | |
| PATCH | `/gateway/channel-bindings/:id` | sync state/fields | `{binding}` | |

## Vendor packs

| Method | Path | Body | Response |
|---|---|---|---|
| POST | `/gateway/vendor-packs/:vendor/gaps` | `capability, surface` (transcript, composer, activity, card, computer, approval), optional `severity, fallback_used, sample_ref` | 200/201 `{gap}` |
| GET | `/gateway/vendor-packs/:vendor/gaps?status=` | | `{gaps}` |
| PATCH | `/gateway/vendor-pack-gaps/:id` | `status, severity` | `{gap}` |
| GET | `/gateway/vendor-packs/:vendor/parity` | | `{vendor, parity, openGaps, blockingGaps}` |

## Threads, turns, events, approvals

| Method | Path | Body / query | Response | Errors |
|---|---|---|---|---|
| POST | `/agent-sessions/:id/messages` | `text, metadata?` (vendor-bound sessions are intercepted by the gateway runner) | assistant message | 409, 428, 429, 502 |
| GET | `/threads/:id/events?after=&limit=` | `after` = `sequence` cursor (ascending); absent = newest first; limit 1-500 (default 100) | events `{id, sequence, type, actor, payload, sessionId, occurredAt}` | 404 |
| POST | `/threads/:id/gateway/sync` | | `{events: n}` | 404, 503 |
| GET | `/threads/:id/approvals?state=` | | `{approvals}` | 404 |
| POST | `/gateway/approvals/:id/respond` | `decision` (approve or deny), `actor?: {type:"user"}` | `{approvalId, state}` | 400, 403 `HUMAN_REQUIRED`, 404, 409 `ALREADY_RESOLVED`, 503 |

Also on `thread_routes.rs` (not wrapped by the SDKs yet): `/threads` (GET, POST), `/threads/:id` (GET, PATCH), `/threads/by-session/:session_id`, `/threads/:id/resolve`, `/threads/:id/usage`, `/threads/:id/handoff`, `/threads/:id/deps`.
