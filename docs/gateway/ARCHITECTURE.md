# Agent Gateway architecture

**What this is:** the object model, request flow, event and approval model, failure behavior and file map of the Agent Gateway.
**Who it's for:** engineers changing the gateway or debugging a vendor turn.
**Last verified against:** platform commit `c3af0730ca`.

Spec (source of intent): `Allternit Brain/Research/specs/agent-gateway.md`. Index: [README.md](README.md). Tone and structure follow [../architecture/BOT_THREAD_PARITY_SPEC.md](../architecture/BOT_THREAD_PARITY_SPEC.md).

## 1. Premise

Any vendor's bot can work in Allternit and stay itself. Vendors enter through one interface, AAI. AAI is canonical: REST + SSE, the SDKs, the MCP server and the A2A card are facades over it ([FACADES.md](FACADES.md)). Allternit itself implements AAI: an Allternit Bot is one more AAI provider (`LoopbackProvider`), which is how the conformance harness proves the contract without a vendor.

Rule for this code: nothing new is built where Allternit already had the part. Vendor work reuses `bot_threads`, the bot event ledger (`bot_events`) and `send_bot_turn`.

## 2. Object model

```
Project
 └── Coordinator
      └── Bot                       durable worker identity            (existing)
           ├── ExecutionBinding     native or vendor                   bot_execution_bindings   (V198)
           └── Thread               durable isolated work              bot_threads (V186)
                └── Generation      one runtime context                (existing)
                     └── RemoteThreadBinding  -> vendor context        remote_thread_bindings   (V198)
```

| Object | Table | Key facts |
|---|---|---|
| ProviderAccountBinding | `provider_account_bindings` | A connected vendor or channel account. Holds `secret_ref` / `session_ref`, never raw credentials. Owner-scoped. States in [CONNECTIONS_AND_CREDENTIALS.md](CONNECTIONS_AND_CREDENTIALS.md). |
| BotExecutionBinding | `bot_execution_bindings` | `type` (allternit or vendor), `mode`, `vendor`, `adapter_id`, `account_binding_id`, `preferred_lane`, `external_agent_id`, `capabilities_json`, `health_json`, `state`. Rebinding never changes the Bot id. |
| RemoteThreadBinding | `remote_thread_bindings` | One row per thread generation. `external_context_id`, `sync_cursor`, `last_remote_event_id`. `lane` and `capability_snapshot` are frozen at create (409 on change). |
| ChannelConversationBinding | `channel_conversation_bindings` | One Thread to one external channel conversation. See [CHANNELS.md](CHANNELS.md). |
| Vendor pack registry and gaps | `vendor_pack_registry`, `vendor_pack_gaps` | Installed pack manifests, and open/resolved PackGaps. See [LOOK_PACKS.md](LOOK_PACKS.md). |
| Approval | `gateway_approvals` (V199) | `authority` is `allternit` or `vendor` (CHECK constraint). |
| Send dedupe | `gateway_sends` (V199) | `(remote_binding_id, correlation_id)`. A replayed correlation id never re-sends. |
| Connection audit | `connection_audit` | Every account state change, with from/to state and actor. |
| Channel message log | `channel_message_log` (V200) | Append-only log of inbound and outbound channel messages. |

Binding states:

- Execution: `UNBOUND, BOUND, READY, DEGRADED, NEEDS_AUTH, PAUSED, DISABLED, FAILED`
- Remote thread: `UNBOUND, OPENING, ACTIVE, HANDOFF_PENDING, CLOSED`
- Channel sync (Rust): `LIVE, DELAYED, RECONNECTING, DEGRADED, DISCONNECTED`

Transition tables are in `agent_gateway_routes.rs` (`connection_next`, `exec_next`, `remote_next`) and mirror `CONNECTION_TRANSITIONS`, `EXECUTION_TRANSITIONS` and `REMOTE_THREAD_TRANSITIONS` in `platform/packages/subscription-fabric-contracts`. An illegal move returns 409 with the allowed list.

Known inconsistency: the TypeScript contract `channelSyncStateSchema` in `subscription-fabric-contracts/src/bindings.ts` is lowercase (`live, delayed, reconnecting, paused, error`) while Rust stores and validates the uppercase set above. Nothing crosses the two today, but it should be reconciled before a TS consumer reads `syncState`.

Vendor switching: because each generation has its own RemoteThreadBinding, a Thread can move between vendors across generations. `handoff_closes_old_binding_and_opens_a_new_one` (in `gateway_runner.rs`) covers it.

## 3. Modes, lanes, guarantees

| Term | Values | Meaning |
|---|---|---|
| Mode | `hosted`, `linked`, `mirror` (`native` for Allternit bots) | Who holds authority. Hosted: vendor semantics, execution through Allternit infrastructure. Linked: the vendor stays authority, Allternit orchestrates and observes. Mirror: an Allternit copy of observable vendor state. |
| Lane | `official`, `channel`, `ui_bridge`, `local` | How we reach the vendor: vendor API, a messaging channel, driving the vendor's app, or a self-hosted runtime. |
| Guarantee | `exact`, `best_effort`, `read_only` | What the lane can promise. Event guarantee is `exact`, `best_effort` or `inferred`. |

Moving an adapter from `ui_bridge` to `official` changes nothing above AAI. Context isolation (`isolated`, `shared`, `stateless`) and `maxParallel` are routing inputs, see [Placement](#10-coordinator-placement).

Mirror mode has an engine (`platform/packages/agent-gateway/src/mirror.ts`) but no adapter uses it yet. See [Mirror](#9-mirror-field-states).

## 4. Where each component lives

| Concern | Location |
|---|---|
| Wire types, schemas, transition tables | `platform/packages/subscription-fabric-contracts/src/{agent,bindings,capability,events,vendor-pack,manifest,thread}.ts` |
| Provider interface, router, loopback, conformance, Mirror, replay | `platform/packages/agent-gateway/src/{provider,router,loopback,conformance,mirror,replay}.ts` |
| AAI host (`/aai/*`), kill switch, pacing, adapter loading | `services/subscription-gateway/src/aai/registry.ts`, `src/http/routes_aai.ts`, `src/aai/call-scope.ts` |
| Vendor adapters | `services/subscription-gateway/adapters/{grok-bot,claude-desktop,claude-managed-agents,chatgpt-dots,openclaw}`; macOS AX helper in `services/subscription-gateway/native/ax-bridge` |
| Bindings, accounts, packs, gaps routes | `cmd/allternit-api/src/agent_gateway_routes.rs` |
| Thread runner, event bridge, approvals | `cmd/allternit-api/src/gateway_runner.rs` |
| Coordinator placement | `cmd/allternit-api/src/gateway_placement.rs` (called from `coordinator_routes.rs`) |
| Channels | `cmd/allternit-api/src/{channel_gateway,channel_transports,teams_auth,discord_gateway}.rs` |
| Facades | `cmd/allternit-api/src/{aai_facade,a2a_routes,mcp_server_routes}.rs`; SDKs `platform/packages/aai-sdk`, `platform/python/allternit-aai` |
| Vendor CLI groundwork | `cmd/gizzi-code/src/runtime/session/{vendor-message,vendor-session}.ts`; routes `POST /:sessionID/vendor-message` in `server/routes/session.ts`, `POST /v1/agent-sessions/:sessionID/vendor-message` in `agent-compat.ts` |
| Migrations | `cmd/allternit-api/migrations/V198__agent_gateway_bindings.sql`, `V199__gateway_runner.sql`, `V200__channel_message_log.sql`, `V201__channel_binding_names.sql` |
| UI | allternit-ai repo, `docs/gateway-ui.md` |

## 5. Request flow: one vendor turn

```
web (BotChatSessionView)
  POST /api/v1/agent-sessions/:id/messages  {text, metadata}
        |
        v
allternit-api  gateway_runner::run_turn
  1. resolve thread + bot -> bot_execution_bindings row
       no binding, or type=allternit  -> native path, unchanged
       type=vendor, state != READY    -> 409 BINDING_NOT_READY (never a native fallback)
  2. replay check: gateway_sends(remote_binding_id, correlation_id)
  3. consequential? -> needs approved, unconsumed Allternit approval, else 428 APPROVAL_REQUIRED
  4. one remote_thread_bindings row per generation (open_remote -> agent.context.open)
  5. unseal provider credential just now (token_crypto), attach as top-level `credential`
        |
        |  POST {gateway}/aai/call   {op, binding, input, credential?}
        v            (Sessions computer guest port + sealed token, GATEWAY_OFFLINE 503 if absent)
subscription-gateway  AaiHost.call -> AaiRouter.bind(binding) -> guardProvider
        |                (kill switch: LANE_BLOCKED; pacing: minGapMs / maxPerHour)
        v
adapter AaiProvider   contextMessage ... vendor (API / CDP / AX / browser / local HTTP)
        |
        v  events (agent.events, cursor-based)
gateway_runner::pull_events
  - dedupe by remote_event_id, append to bot_events tagged with thread_id (envelope in payload)
  - agent.message.completed -> assistant message in the session transcript (attributed to vendor bot)
  - agent.approval.requested -> gateway_approvals (authority=vendor), thread -> needs_you
  - cursor stored on remote_thread_bindings.sync_cursor
        |
        v
web polls  POST /threads/:id/gateway/sync, GET /threads/:id/events?after=<sequence>
```

The `credential` is held in `AsyncLocalStorage` for one call inside the gateway (`call-scope.ts`), never persisted, logged or put on an event or error. Today only `claude-managed-agents` reads it (`adapters/claude-managed-agents/credential.ts`). Session-based and local adapters ignore it.

Channel-lane bots (Muse over WhatsApp) take a different transport at step 5: `ChannelLaneTransport` in `channel_transports.rs` sends over the account's channel and returns the vendor's replies as `agent.message.completed`. Everything on another lane passes through to the normal transport.

Every inbound path (REST, MCP, A2A, channel) ends in the same runner. `aai_facade.rs` is the shared owner-scoped core, and nothing in it can answer an approval.

## 6. Event envelope

Vendor events go into the existing bot ledger and gizzi's event stream. There is no second user-visible event model. The envelope rides in the event payload and adds: `bot_id, thread_id, generation_id, source (allternit|vendor), vendor, adapter, lane, remote_event_id, remote_context_id, causation_id, correlation_id, guarantee (exact|best_effort|inferred)`.

Normalized types (`gatewayEventTypeSchema`): `agent.context.opened, agent.activity.started, agent.message.delta, agent.message.completed, agent.tool.called, agent.approval.requested, agent.approval.resolved, agent.artifact.created, agent.computer.frame, agent.task.updated, agent.health.changed`, plus `channel.message.received, channel.message.sent, channel.reaction.updated, channel.message.edited, channel.message.deleted`.

A UI bridge observing a page is `best_effort` or `inferred`, and the UI never shows it with the certainty of an official API event. Reads: `GET /threads/:id/events?after=<sequence>&limit=` (ascending by `sequence`; without `after`, newest first).

## 7. Approvals: two authorities

`gateway_approvals.authority` is `allternit` or `vendor`. States: `pending, approved, denied, expired, cancelled`.

- **Allternit approval** gates consequential sends before anything reaches the vendor. A consequential turn without an approved, unconsumed approval fails 428 `APPROVAL_REQUIRED` with `approvalId`. Sending with `allternit_approval_id` consumes it.
- **Vendor approval** is recorded when the vendor asks (`agent.approval.requested`), unique per `(thread_id, remote_ref)`. The thread goes `needs_you`. It is never bypassed.
- **Never conflated.** An Allternit approval never resolves a vendor one (`allternit_approval_gates_consequential_sends_and_never_resolves_a_vendor_one`).
- **Humans only.** `POST /gateway/approvals/:id/respond` accepts `actor: {type:"user"}`. A bot or other actor gets 403 `HUMAN_REQUIRED`. MCP `approval_respond` always refuses. The TS SDK throws `HumanIntentRequiredError` without human intent. Replays get 409 `ALREADY_RESOLVED`.

## 8. Failure table

Implemented in `gateway_runner::fail` and `handle_context_lost`. Every failure also writes a ledger event (`gateway.turn.failed`, or `gateway.adapter.drift`).

| AAI code | Binding effect | Thread effect | HTTP |
|---|---|---|---|
| `AUTH_REQUIRED`, `AUTH_REVOKED` | to `NEEDS_AUTH` | `needs_you` | 409 |
| `LANE_BLOCKED`, `BOT_DETECTED`, `BOT_DETECTION`, `ACCOUNT_RISK` | to `DISABLED` (lane switched off, no retries) | `needs_you` | 403 |
| `ADAPTER_DRIFT` | to `DEGRADED`, consequential automation stops | `needs_you` | 503 |
| `RATE_LIMITED` | `health.rateLimitedUntil` set from `retryAfterMs` (default 30000), state unchanged, contexts not mixed | unchanged | 429 |
| context lost, resume declared | none | none | 409 `CONTEXT_LOST_RESUMABLE`, retry resumes |
| context lost, no resume | old binding closed, new generation from checkpoint | none | 409 `CONTEXT_LOST`, resend continues |
| other vendor failure | none | none | 502 |
| gateway not connected | none | none | 503 `GATEWAY_OFFLINE` |
| binding not READY | none | none | 409 `BINDING_NOT_READY` |

Account revocation cascade: an account going `REVOKED` or `EXPIRED` moves every dependent execution binding to `NEEDS_AUTH`. Threads are never deleted. See [CONNECTIONS_AND_CREDENTIALS.md](CONNECTIONS_AND_CREDENTIALS.md#revocation-cascade).

## 9. Mirror field states

`platform/packages/agent-gateway/src/mirror.ts` computes field-level sync from normalized snapshots (`agent.snapshot`), never raw DOM.

- An adapter declares each field: `observability` (`exact`, `partial`, `none`) and `writable`.
- States: `synced`, `partial`, `stale` (direction `remote_ahead` or `local_ahead`), `conflict`, `unobservable`.
- A field with `observability: none` is `unobservable` and never claimed synced (`AG/mirror.test.ts`).
- `planMirrorWrites` refuses to write a field the adapter cannot write back, so local editing is disabled for it.
- States carry a short opaque reference, not raw values.

Status: engine and tests exist. No shipped adapter declares a Mirror snapshot, and no UI or REST route drives it beyond the `agent.snapshot` and `agent.sync` AAI ops.

## 10. Coordinator placement

`gateway_placement::place_plan` runs when the coordinator builds a plan (`coordinator_routes.rs`). It contains no vendor-name branches.

- **Candidate filter:** agent status permits work, binding is READY (or the bot is native), required capabilities are a subset of the capability snapshot, parallel capacity exceeds ACTIVE remote contexts, policy allows.
- **Capacity:** from `context.maxParallel` and `context.isolation`. A `shared` context counts as capacity 1 (every thread would land in one remote conversation). Default capacity applies when none is declared.
- **Score:** `roleFit + capabilityFit + availableCapacity - degradedLanePenalty`. `historicalPerformance, locality, costFit, vendorPreference, riskPenalty` are stubbed at 0.
- The planner's own pick is kept when it qualifies. Otherwise the best-scoring alternate is used. If only capacity or isolation blocks, the step is serialized behind the step holding the context.
- A step nothing can host stays on the planner's bot with `placed: false` and a plain-language reason for each rejected bot ("why not chosen").

## 11. Terms and safety rules that the code enforces

- A vendor bot that is not READY never falls back to a native brain.
- Credentials are unsealed per call and never stored in the gateway.
- Consequential outbound actions need Allternit approval even if the vendor also asks.
- UI-bridge lanes latch on bot detection and drift; nothing retries or solves challenges.
- A new scope is a spec change, not silent code.

## 12. Decisions log

All from the spec, `Research/specs/agent-gateway.md`.

| Date | Decision |
|---|---|
| 2026-09-29 | Gateway and AAI are the long-term product, separate from subsfab; subsfab lanes underneath for now. |
| 2026-09-29 | Bots stay the one worker model; no VendorAgent/VendorTask/VendorSession objects. |
| 2026-09-29 | Threads stay the one unit of work (`bot_threads`, V186). Vendor ids live in `remote_thread_bindings`. |
| 2026-09-29 | The Coordinator never talks to vendors; generic questions only. |
| 2026-09-29 | Modes are Hosted, Linked, Mirror. Never "Twin". |
| 2026-09-29 (v2) | Mirror sync is per field; unobservable fields are shown honestly. |
| 2026-09-29 (v2) | Look packs are authentic; Allternit keeps navigation, inspector, artifacts, memory and a mandatory provenance layer. |
| 2026-09-29 (v2) | Build order: contracts, loopback and conformance, then adapters Grok Bot, Claude, dots, Muse, OpenClaw. |
| 2026-09-29 (v2) | An adapter is not done until its look pack renders. |

## 13. Read the code in this order

1. `platform/packages/subscription-fabric-contracts/src/agent.ts`: AAI operations, lanes, guarantees, error codes, event types, approvals.
2. `platform/packages/subscription-fabric-contracts/src/bindings.ts`: binding schemas and the state transition tables.
3. `platform/packages/agent-gateway/src/provider.ts` and `router.ts`: the provider interface and the router that adds idempotency, capacity and human-only approvals.
4. `services/subscription-gateway/src/aai/registry.ts` and `src/http/routes_aai.ts`: how providers are loaded, guarded and called over HTTP.
5. `services/subscription-gateway/adapters/openclaw/` (small) then `adapters/grok-bot/` (a UI bridge): manifest, provider, `aai.ts`, fixtures.
6. `cmd/allternit-api/migrations/V198__agent_gateway_bindings.sql` and `V199__gateway_runner.sql`: the tables.
7. `cmd/allternit-api/src/agent_gateway_routes.rs`: accounts, bindings, packs, gaps, state machines, revocation cascade.
8. `cmd/allternit-api/src/gateway_runner.rs`: `run_turn`, `open_remote`, `pull_events`, `gate_allternit`, `fail`, approvals.
9. `cmd/allternit-api/src/gateway_placement.rs` and `aai_facade.rs`: placement, and the shared core behind MCP and A2A.
10. `cmd/allternit-api/src/channel_gateway.rs` then `channel_transports.rs`: the channel trait, send policy, and per-platform transports.

The web side starts at `allternit-ai/src/lib/gateway/api.ts` and `src/components/gateway/VendorPackSurface.tsx`.

## 14. How to debug a vendor turn

Follow the hops in section 5. At each one, check this before moving on.

| Hop | Symptom | Check |
|---|---|---|
| Web send | Turn never leaves the browser, or an error banner | Browser network tab for `POST /api/v1/agent-sessions/:id/messages`. 409 or 428 bodies carry `code` and `approvalId` (`VendorSendError` in `allternit-ai/src/lib/gateway/api.ts`). |
| Binding | 409 `BINDING_NOT_READY` | `GET /gateway/bots/:bot_id/execution-binding`: `state` must be `READY`, and `health` shows `rateLimitedUntil`. `NEEDS_AUTH` means the account moved (`GET /gateway/provider-accounts/:id`, then `connection_audit` for the event trail). `DISABLED` usually means a latched `LANE_BLOCKED` or the kill switch. |
| Approval gate | 428 `APPROVAL_REQUIRED` | `GET /threads/:id/approvals?state=pending`. The turn is resent with the approved `allternitApprovalId`. An approval is consumed once (`gateway_approvals.consumed`). |
| Replay | A retry returns quickly without reaching the vendor | `gateway_sends` has the `(remote_binding_id, correlation_id)` row. Use a new correlation id for a genuine resend. |
| Remote context | Wrong or missing vendor context | `GET /gateway/threads/:id/remote-bindings`: one row per generation, `state`, `external_context_id`, frozen `lane`. Ledger events `agent.context.opened`. |
| allternit-api to gateway | 503 `GATEWAY_OFFLINE`, 409 `sessions_computer_not_bound`, `sessions_computer_not_running` | `GET /api/v1/subscriptions/binding`, and whether the Sessions computer is running. `forward` in `subscription_routes.rs` returns these before the gateway sees anything. |
| Gateway host | `LANE_BLOCKED` with "disabled (kill switch)" or "pacing" | `GET /aai/providers` shows `disabled` and `pacing` per adapter. The message says which limit fired and `retryAfterMs`. |
| Adapter | `VENDOR_UNAVAILABLE`, `AUTH_REQUIRED`, `ADAPTER_DRIFT` | The adapter's README for the failure list, and its log lines. Run `POST /aai/conformance/<id>` to separate an adapter bug from a vendor change. UI bridges latch after a bot check or drift and need `clearHalt()` after the user resolves it. |
| Events back | Reply missing from the transcript | `POST /threads/:id/gateway/sync`, then `GET /threads/:id/events?after=0`. `agent.message.completed` becomes a transcript message, deduped by `remote_event_id` (`gizzi-code` `VendorMessage.append` skips duplicates). `remote_thread_bindings.sync_cursor` shows how far the pull got. |
| Failure effects | Thread shows needs_you | Ledger events `gateway.turn.failed` and `gateway.adapter.drift` carry the AAI code. The mapping is in section 8. |
| Channel post | Post shows Pending | `channel_message_log` rows for the binding, state `unconfirmed`. It is confirmed by the inbound echo or a resume. It is never re-posted blindly. |

Tables to query directly (SQLite, owner-scoped rows): `bot_execution_bindings`, `provider_account_bindings`, `remote_thread_bindings`, `gateway_approvals`, `gateway_sends`, `connection_audit`, `channel_message_log`, and `bot_events` filtered by `thread_id`.
