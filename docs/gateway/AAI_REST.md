# AAI REST reference

Generated from the route tables in `cmd/allternit-api/src/{agent_gateway_routes,gateway_runner,thread_routes,agent_session_routes}.rs`. All paths are under `/api/v1`; all require `Authorization: Bearer <token>` and are owner-scoped (a resource you do not own returns 404). Bodies use the field names the handlers deserialize (the handlers in `agent_gateway_routes.rs` use `#[serde(rename_all = "camelCase")]`, so send camelCase such as `authType` and `accountBindingId`; the `snake_case` names in the tables below are what the SDKs send, and the SDKs and handlers disagree, see [QUICKSTART.md](QUICKSTART.md#9-the-same-flow-with-the-sdks); responses are camelCase).

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

## One-step connect (`/gateway/connect`)

| Method | Path | Body | Response | Errors |
|---|---|---|---|---|
| GET | `/gateway/connect` | | `{products:[{product,name,offers,signIn,verifiedLive,terms?,account?,bots[]}]}` | |
| POST | `/gateway/connect` | `product`; optional `acceptTerms` (website/app products, after showing `action.terms`; recorded once per account), `apiKey` (claude-managed-agents), `agents[]` (externalAgentIds to make bots for; default all), `channelAccountId`, `accessToken`, `phoneNumberId`, `appSecret`, `agentAddress` (muse) | `{product,status,message,action?,verifiedLive,account:{id,state},agents[],bots:[{botId,name,externalAgentId,state}]}`; `status` ∈ `ready, needs_consent, needs_sign_in, needs_key, needs_setup, blocked` | 400 unknown product |

Idempotent: a second call reuses the account, login and bots. Source: `cmd/allternit-api/src/gateway_connect.rs`.

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
| POST | `/threads/:id/gateway/tool-result` | `toolUseId, result, isError?` — answers a vendor agent's custom tool call (`agent.tool.called` with `custom: true`; the thread shows `needs_you` while it waits). Sent as `agent.context.message` with `toolResult` | `{sent: true, events: n}` | 400, 404, 409 `NOT_VENDOR_BOUND` / `CONTEXT_LOST`, 503 |

Also on `thread_routes.rs` (not wrapped by the SDKs yet): `/threads` (GET, POST), `/threads/:id` (GET, PATCH), `/threads/by-session/:session_id`, `/threads/:id/resolve`, `/threads/:id/usage`, `/threads/:id/handoff`, `/threads/:id/deps`.

## Subscription gateway AAI host

The subscription gateway (`services/subscription-gateway`, `src/http/routes_aai.ts`) is what allternit-api calls for vendor bots. Its routes are not under `/api/v1`; they take the gateway's own bearer token (`Authorization: Bearer <token>`, scopes as listed). AAI results answer HTTP 200 as `{ok:true,value}` or `{ok:false,error:AAIError}`; only transport problems use other statuses.

| Method | Path | Scope | Body / query | Response |
|---|---|---|---|---|
| POST | `/aai/call` | `tasks:submit` | `op, binding, input, credential?` | `{ok,value}` or `{ok:false,error}` |
| GET | `/aai/events/stream` | `tasks:read` | `contextId` and `adapterId` (or `binding=<BotExecutionBinding JSON>`), `cursor?`; header `Last-Event-ID?` | `text/event-stream` (below); 400 bad query, 503 no host |
| GET | `/aai/providers` | `tasks:read` | | `{providers:[{adapterId, disabled, pacing?, manifest, error?}]}` |
| POST | `/aai/conformance/:adapterId` | `tasks:submit` | `?offline=1` for shipped fixtures | conformance report; 404 unknown adapter or no fixtures |

`GET /aai/events/stream` is SSE over `agent.events` for one context. Each event is `event: aai`, `id: <cursor>`, `data: <wire event>` (the same flat event `/aai/call agent.events` returns, including `cursor`). It starts after `Last-Event-ID` when the client sends one (a reconnecting `EventSource` does), else after `?cursor`, else from the start of the context's log. Without `binding`, the binding the context was opened with is used. The gateway polls the provider (every 500 ms), sends a `: hb` comment every 15 s, and stops polling when the client disconnects. A failed read is sent as `event: error` with the `AAIError` as data: a retryable one keeps the stream polling (after `retryAfterMs`), any other ends it.

Contexts survive a gateway restart: the host keeps each context's binding, agent, vendor conversation keys, last cursor and closed flag in `state.db` (`aai_contexts`), plus the provider's own snapshot for providers that keep the conversation client-side (OpenClaw, Hermes), which continue with their history and cursor. A context whose provider cannot resume it (the desktop-app adapters: Grok Bot, ChatGPT dots, Claude desktop) answers `CONTEXT_NOT_FOUND` with `details.reason: "CONTEXT_LOST"`, which allternit-api turns into its lost-context handoff; a closed one answers `CONTEXT_NOT_FOUND` with `details.closed: true`.

Turns on a website or desktop subscription (manifest lane `ui_bridge`: Grok Bot, ChatGPT dots, Claude desktop and the `*-subscription` lanes) take turns per account: up to the lane's `context.maxParallel` run at once and the rest wait in arrival order, without opening another window or tab. While a message waits, `agent.events` on its context carries an `agent.activity.started` event with `payload.kind: "queued"`, `position` and `detail` such as `queued: waiting for your Grok app (position 1)`, then one with `state: "started"` when it begins. `agent.context.cancel` on a waiting message takes it out of line (it answers `details.cancelled`). A full line answers `RATE_LIMITED`; a message that waited too long answers `CONTEXT_BUSY` with `details.queueTimeout`. Limits: `SUBS_GATEWAY_AAI_QUEUE_MAX` (default 8 waiting per account) and `SUBS_GATEWAY_AAI_QUEUE_WAIT_MS` (default 600000).
