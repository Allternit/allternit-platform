# claude-managed-agents adapter

Drives the user's own Claude Managed Agents (agents, environments and sessions in their Anthropic workspace) through the official API with the user's own API key. Lane `official`, guarantee `exact`, mode `hosted`. One Managed Agents session per AAI context, and the context id is the session id.

**Status: offline conformance passes. Not verified live.** No API key was available. Every event shape comes from the installed `@anthropic-ai/sdk` 0.95.2 types and the Managed Agents docs, and is exercised only against `fixtures/fake-client.ts`.

## Configuration

Only non-secret settings come from env (`aai.ts`). The API key never comes from env. allternit-api attaches it to each `POST /aai/call` as the short-lived per-call credential (`credential.ts`).

| Variable | Default | Meaning |
|---|---|---|
| `SUBS_GATEWAY_CLAUDE_MA_ENVIRONMENT_ID` | none (required to open new sessions) | Environment new sessions run in. Allternit never creates environments. A wrong or archived id gives an error that names the available environments. |
| `SUBS_GATEWAY_CLAUDE_MA_BASE_URL` | SDK default | API base URL override. |
| `SUBS_GATEWAY_CLAUDE_MA_MAX_PARALLEL` | 4 | Open sessions per binding. One more open returns `CONTEXT_BUSY`. |
| `SUBS_GATEWAY_CLAUDE_MA_MEMORY_STORE_IDS` | none | Comma-separated memory stores mounted into new sessions. |
| `SUBS_GATEWAY_CLAUDE_MA_VAULT_IDS` | none | Comma-separated vaults attached to new sessions. |
| `SUBS_GATEWAY_CLAUDE_MA_ARCHIVE_ON_CLOSE` | `true` | `context.close` archives the session. Nothing is ever deleted. |

Provider options that are not set from env, with their defaults: `replyTimeoutMs` 120 s, `reconnectBaseMs` 250 ms (doubles, capped at 10 s), `maxReconnects` 5, `backfillOverlapMs` 60 s and `pollMs` 500 ms. `pollMs` is used only to verify interrupts.

## How events flow

- **Stream first.** Before each send (message, tool confirmation or custom tool result), the adapter opens `sessions.events.stream` and keeps it open until the turn ends or pauses.
- **Backfill on every connect and reconnect.** The adapter reads `sessions.events.list` from a cursor: `created_at[gte]` set to the newest `processed_at` seen, minus a 60 s overlap. Events are deduped by vendor event id. A dropped stream reconnects with backoff. After `maxReconnects` consecutive failures, the adapter logs `agent.health.changed` (`stream_lost`) and the waiting `context.message` fails with `VENDOR_UNAVAILABLE`.
- **Full history is read once per context.** This happens on first open and on adoption. `events()` reads with the cursor while no stream is open, and reads nothing while a stream is open.
- **Queued messages are handled.** The documented queued-then-processed second sighting of a client event is honoured: the stale idle of an earlier turn does not end a queued message's turn.
- **Deltas arrive one event at a time.** The API streams whole `agent.message` events and has no token deltas. Each event becomes `agent.message.delta` + `agent.message.completed` as soon as it arrives. `buffered: false` means the event came live from the stream. `buffered: true` means it was recovered from history.

## Support matrix

| Area | Support |
|---|---|
| list / get / identity | Yes, non-archived agents only. Agent and environment create, edit and delete are out of scope by design. |
| context open / adopt / close | Yes. Adoption by session id rebuilds the cursor log and resumes the stream if the session is running. |
| session listing | `agent.snapshot` → `fields.sessions` (id, status, title, Allternit thread id, created, archived, adopted). Also `provider.listSessions()`. |
| message / steer / cancel | Yes. Steer interrupts the turn, then sends a message. Cancel interrupts, then verifies with `sessions.retrieve`. |
| events | Native SSE stream plus cursor backfill. Exact, replayable, deduped. |
| approvals | Yes, vendor authority. The adapter only relays a person's decision; a non-human actor gets `APPROVAL_REQUIRED`. |
| custom tools | `agent.custom_tool_use` becomes `agent.tool.called` (`custom: true`, `toolUseId`, `name`, `input`), and the pause emits `agent.task.updated` `{ kind: "needs_attention", reason: "custom_tool_result_required" }`. Allternit answers with `context.message({ text, toolResult: { toolUseId, isError? } })`, which sends `user.custom_tool_result`. |
| artifacts | Read: files scoped to the session. No file upload, because the AAI `artifacts` op has no upload shape. |
| memory / computer / tasks / sync | Not supported. They answer `UNSUPPORTED`. Memory stores can still be mounted at create. |

## Not verified

- No live session has run. Unverified against the live service: the SSE behaviour (keep-alives, how long an idle stream stays open), whether `events.list` `created_at[gte]` matches `processed_at` closely enough for the cursor, and the exact 4xx returned for a bad environment id.
- Sub-agent threads (`session_thread_id`) are passed through on custom tool results but have not been tested.

## Manual live gate (needs the account owner's consent)

1. The owner creates an agent and an environment in the Claude Console. If the custom-tool step is to be tested, the agent needs one custom tool. Set `SUBS_GATEWAY_CLAUDE_MA_ENVIRONMENT_ID`.
2. Link the owner's API key in Allternit. Check that `agent.list` shows the agent and that `agent.health` is `healthy`.
3. Open a context and send one message. `events()` must show a live delta (`buffered: false`) before the turn ends, and the reply must match the Console transcript.
4. During a long turn, cut the network for about 10 s. After it returns, check for no gaps and no duplicate `remoteEventId`s, and that the reconnect backfill used `created_at[gte]`.
5. Trigger an `always_ask` tool. The approval must appear and be answered by the owner, then the turn must finish.
6. Trigger the custom tool. `needs_attention` must appear, and answering it with `toolResult` must let the turn finish.
7. Restart the gateway. `agent.snapshot` must list the session, and `context.open({ adoptContextId })` must rebuild the same events.
8. Set a wrong environment id. The open must fail and name the available environments.
9. Close the context and check the session is archived, not deleted. Record the date, SDK version and results here, and update the row in `docs/gateway/ADAPTERS.md`.
