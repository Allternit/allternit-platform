# Agent Gateway contract audit

Both sides extracted from code (not comments) at `gateway/integration` c3af0730. Rust paths are in `cmd/allternit-api/src/`.
Status: FIXED (TS/web/SDK, with a regression test), RUST (Rust change required, not edited), OK.

## A. web <-> allternit-api

| # | Item | Web (side A) | Rust (side B) | Match |
|---|---|---|---|---|
| A1 | Path prefix | `allternit-ai/src/lib/gateway/api.ts` sent `/gateway/*`, `/threads/*` | all routers merged into `v1_routes`, nested at `/api/v1` (`main.rs:843,926-929,1018`) | FIXED: every call now `/api/v1/...` (`V1` const) |
| A2 | Vendor event payload | `useThreadGateway.ts normalizeEvent` spread `payload` flat | `gateway_runner.rs:737` writes `{data, envelope}` | FIXED: `unwrapPayload` |
| A3 | Approval-requested title | look pack reads `title` | payload has `action`/`summary` (`gateway_runner.rs:746`) | FIXED: title := action/summary |
| A4 | Events cursor | typed `string`, compared as string | `thread_routes.rs:1413` returns numeric max `sequence` or null | FIXED: coerced to string |
| A5 | Channel event `reaction.updated` | `emoji`, `count` | `reaction`, `added` (`channel_gateway.rs:330`) | FIXED |
| A6 | Channel `sent` message id | `messageId` required | outbound has `remoteId`/`correlationId` only (`channel_gateway.rs:505`) | FIXED (id := correlationId) |
| A7 | `channel.message.pending` | ignored | emitted for unconfirmed posts (`channel_gateway.rs:523`) | FIXED (delivery=unconfirmed) |
| A8 | Channel author/time | `author{name}`, `ts` | `actor{type,id}`, `occurredAt` on the row | FIXED |
| A9 | Approval `detail` | typed `string` | `detail_json` surfaced as object `detail` (`gateway_runner.rs:827`, `agent_gateway_routes.rs camel()`) | FIXED (type `unknown`) |
| A10 | Vendor memory | `GET/POST /gateway/bots/:id/vendor-memory[...]` (`api.ts`) | no route exists | FIXED web (404 -> unavailable); FIXED Rust R6 (routes + `memoryRecordSchema` gains optional `id`,`text`) |
| A11 | Approval id on vendor cards | card `id` = vendor `approvalId` (event payload) | `POST /gateway/approvals/:id/respond` takes the gateway row id (`gap_...`, `gateway_runner.rs:791`), vendor id is `remoteRef` | FIXED R5 (raw `agent.approval.requested` event carries `data.gatewayApprovalId`; web already dedupes by `remoteRef`) |
| A12 | Channel display names | `channelName`, `workspaceName` | not in `CHAN_COLS` (`agent_gateway_routes.rs:~262`) | FIXED R7 (V201 `channel_name`,`workspace_name`; set on create/PATCH) |
| A13 | Sync/connection/exec/remote states, parity, severity, gap status | upper-case sets, `full/partial/blocked` | `agent_gateway_routes.rs:43-50,110-117` | OK |
| A14 | Envelopes `{account}`,`{accounts}`,`{binding}`,`{bindings}`,`{gap}`,`{gaps}`,`{approvals}`,`{agents}`, `{deleted,dependentBots}` | same | same | OK |
| A15 | 409 transition body `{error,from,to,allowed}`; 409 delete `{dependentBots}` | `conflictOf` | `agent_gateway_routes.rs:107,497` | OK |
| A16 | 428/409 vendor send body `{error,code,retryAfterMs,approvalId}` | `VendorSendError` reads `details.approvalId` | `gateway_runner.rs:203` (flat) | OK (client puts flat body in `details`) |
| A17 | Query `after`, `state`, `force`, `vendor` | same names | `EventsQuery`, `ListQ`, `DeleteQ`, `AccountFilter` | OK |
| A18 | Request bodies (camelCase) | `CreateAccountBody`, `PutExecutionBody`, `{apiKey}`, `{decision}` | serde `rename_all=camelCase` | OK |

## B. allternit-api <-> AAI host (`/aai/call`)

| # | Item | Rust sends/reads | TS host | Match |
|---|---|---|---|---|
| B1 | Body | `{op, binding, input, credential?}` (`gateway_runner.rs:133`) | `routes_aai.ts` same | OK |
| B2 | Binding nulls | rows() emits JSON `null` for NULL columns | zod `.optional()` rejected null (400 -> GATEWAY_UNAVAILABLE) | FIXED `normalizeWireBinding` |
| B3 | Transient discovery binding | `{type,vendor,accountBindingId}` only (`agent_gateway_routes.rs:565`) | schema needs id/botId/mode/state; router needs `adapterId` | FIXED (defaults, adapterId := vendor) |
| B4 | `agent.context.open` agent field | `externalAgentId` (`gateway_runner.rs:487`) | `p.contextOpen(i)` expects `agentId` | FIXED (`registry.ts` alias) |
| B5 | `agent.events` element shape | flat `type`, `payload`, `remote_event_id`, `correlation_id`, `remote_context_id`, `causation_id` (`gateway_runner.rs:706-737`) | `{cursor, event:{camelCase}}` | FIXED `wireEvent` (flat, both spellings) |
| B6 | events result | `events`, `cursor` | `{events, cursor: nextCursor}` | OK |
| B7 | Result envelope | `{ok,value}` / `{ok:false,error}` | same, HTTP 200 | OK |
| B8 | Error fields | `code,retryable,retryAfterMs,humanMessage` | `aaiErrorSchema` | OK |
| B9 | `agent.list` element | `externalAgentId`\|`id`, `name` (`agent_gateway_routes.rs:582`) | `{agentId, displayName, vendor, state}` | FIXED R1 (reads `agentId`/`displayName`, old keys as fallback) |
| B10 | approvals respond actor | `actor.type: "user"` (`gateway_runner.rs:880`) | `ApprovalsInput` actor `"human"\|"system"` (types.ts) | FIXED R2 (`actor.type: "human"`) |
| B11 | `generationId` | integer (`gateway_runner.rs:713`) | contracts `z.string()` (`agent.ts:159`) | FIXED R4 (string on the wire) |
| B12 | Error code -> status map | uses `BOT_DETECTED`,`BOT_DETECTION`,`ACCOUNT_RISK`,`REMOTE_CLOSED` | not in `aaiErrorCodeSchema` | FIXED R3 (every `aaiErrorCodeSchema` code mapped; dead codes removed) |

## C. SDKs <-> allternit-api

| # | Item | SDK | Rust | Match |
|---|---|---|---|---|
| C1 | Request body key case | TS `snake()` and Python `_snake` sent `auth_type`, `display_name`, `account_binding_id`, ... | camelCase only (`agent_gateway_routes.rs:349..929`) | FIXED both, tests updated + new `aai-sdk/test/contract.test.ts` |
| C2 | Secret body | `{api_key}` | `SecretBody{apiKey}` | FIXED |
| C3 | Paths, methods, envelopes, 428/409/429 error body | same as A | same | OK |
| C4 | `accounts.list({state})` | sends `state` | `AccountFilter` only `vendor`; state silently ignored | FIXED R8 |
| C5 | `vendorPacks.updateGap({severity})` | body may lack `status` | `PatchGap{status}` required, severity ignored | FIXED R9 (`status` optional, `severity` accepted) |
| C6 | docs `AAI_REST.md` said bodies are snake_case | | | FIXED (doc) |

## D. contracts package <-> Rust constants

| # | Item | contracts | Rust | Match |
|---|---|---|---|---|
| D1 | ChannelSyncState | `live,delayed,reconnecting,paused,error` (`bindings.ts:122`) | `LIVE,DELAYED,RECONNECTING,DEGRADED,DISCONNECTED` (`agent_gateway_routes.rs:50`) | FIXED (contracts) |
| D2 | Connection/exec/remote states + transition tables | identical | identical | OK (pinned by `rust-parity.test.ts`) |
| D3 | Gap severity/status, `parity_of` vs `packParity` | identical | identical | OK |

## E. Event consumption (bot_events written vs read)

| # | Event | Written | Read by web | Match |
|---|---|---|---|---|
| E1 | `agent.*` | `{data, envelope}` | flat | FIXED (A2) |
| E2 | `approval.requested/resolved`, `gateway.*` | ledger only | not consumed by packs | OK |
| E3 | `channel.*` | see A5-A8 | fold expects message fields | FIXED |
| E4 | `channel.message.delivery` | emitted (`channel_gateway.rs:55`) | not in `CHANNEL_TYPES` | FIXED R10 (payload `{messageId, state: confirmed\|failed, delivery}`; web folds it, ai #291) |

## Rust fixes (all landed on `gateway/accept-rust`, tests in `agent_gateway_routes`, `gateway_runner`, `channel_transports`)

- R1 `agent_gateway_routes.rs:582-585` discover_agents: read `agentId` (fallback `externalAgentId`/`id`) and `displayName` (fallback `name`). Today all agents returned by the real host are filtered out.
- R2 `gateway_runner.rs:880`: send `actor: {"type":"human", ...}` for the AAI `agent.approvals` respond (router refuses non-human).
- R3 `agent_gateway_routes.rs` status map / `gateway_runner.rs`: align error codes with `aaiErrorCodeSchema` (or add the extras to the contracts).
- R4 `gateway_runner.rs:713`: `generationId` as string, or change contracts `generationId` to number.
- R5 `gateway_runner.rs:745-750`: include the gateway approval row id in the vendor approval event/card (e.g. `gatewayApprovalId`) so the web card can call `/gateway/approvals/:id/respond`; today it holds the vendor id.
- R6 Add `GET /gateway/bots/:id/vendor-memory` and `POST .../:record/promote` (web is wired, currently shows "unavailable").
- R7 Add `channelName`/`workspaceName` to the channel binding response.
- R8 Honour `state` in `AccountFilter`.
- R9 `PatchGap`: accept `severity`, make `status` optional.
- R10 Document/emit a `messageId` + `state` on `channel.message.delivery`.
