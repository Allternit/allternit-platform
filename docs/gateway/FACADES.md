# Facades: REST, SDKs, MCP, A2A

**What this is:** the four ways other software reaches Allternit bots through AAI, with examples.
**Who it's for:** developers integrating an outside agent or app with Allternit.
**Last verified against:** platform commit `c3af0730ca`.

AAI is canonical. Everything here ends in the same runner (`gateway_runner::run_turn`) through `cmd/allternit-api/src/aai_facade.rs`, which is owner-scoped and cannot answer an approval. Index: [README.md](README.md). Setup and a full walkthrough: [QUICKSTART.md](QUICKSTART.md).

## Common rules

- Auth: `Authorization: Bearer <token>`. Every resource is owner-scoped, and someone else's id is 404.
- Vendor bots go through the gateway. Native bots run as before.
- A consequential send needs an Allternit approval first (428 `APPROVAL_REQUIRED` with an approval id). Only a person answers it, in Allternit.
- Every façade takes a correlation id. Reusing one does not send twice.
- Task and thread state is derived on the server from thread status. A caller cannot set it.

## REST

Full route reference: [AAI_REST.md](AAI_REST.md). Base path `/api/v1`. The turn route is the normal Allternit message route.

```bash
curl -s -X POST -H "Authorization: Bearer $UT" -H 'content-type: application/json' \
  $API/agent-sessions/SESSION_ID/messages -d '{"text":"summarize the report","metadata":{"correlationId":"c-1"}}'
curl -s -H "Authorization: Bearer $UT" "$API/threads/THREAD_ID/events?after=0&limit=100"
```

Field naming caveat, from [QUICKSTART.md](QUICKSTART.md#9-the-same-flow-with-the-sdks): the handlers take camelCase request bodies.

## SDKs

TypeScript: `platform/packages/aai-sdk` (`@allternit/aai-sdk`). Python: `platform/python/allternit-aai` (`allternit_aai`). Both are thin, have the same shape, and refuse to answer an approval without explicit human intent.

Namespaces (TS): `accounts` (create, list, get, update, setConnectionState, setSecret, clearSecret, discoverAgents, delete), `bots` (bindExecution, getBinding, setBindingState, listBindings), `threads` (sendTurn, remoteBindings, sync, events, streamEvents), `approvals` (list, respond), `vendorPacks` (recordGap, gaps, updateGap, parity), `channels` (bind, list, update). Python uses flat snake_case methods (`create_account`, `send_turn`, `stream_events`, `respond_approval`, ...) in `client.py`.

```ts
import { AllternitAgents, ApprovalRequiredError, RateLimitedError } from "@allternit/aai-sdk";
const aai = new AllternitAgents({ baseUrl: "https://api.allternit.com", token });
try {
  await aai.threads.sendTurn(sessionId, "delete the report", { consequential: true });
} catch (e) {
  if (e instanceof ApprovalRequiredError) console.log("needs a person:", e.approvalId);   // 428
  else if (e instanceof RateLimitedError) console.log("retry in", e.retryAfterMs);         // 429
  else throw e;
}
for await (const ev of aai.threads.streamEvents(threadId, { after: 0 })) console.log(ev.sequence, ev.type);
```

`streamEvents` pages the ascending event log by `sequence` with idle backoff. Errors: `AaiHttpError` (base), `ConflictError` (409), `ApprovalRequiredError` (428), `RateLimitedError` (429), `HumanIntentRequiredError` (client-side refusal). The Python module has the same set.

**Known defect:** SDK methods that send a body convert field names to snake_case, but the Rust handlers accept camelCase only, so `accounts.create`, `accounts.update`, `bots.bindExecution`, `vendorPacks.recordGap`, `channels.bind` and `setSecret` do not match the server at this commit. The SDK tests check the snake_case body, not a live server. See [QUICKSTART.md](QUICKSTART.md#9-the-same-flow-with-the-sdks) for the workaround (`request()` with a camelCase body).

## MCP tools

Server: `POST /mcp/server` (`mcp_server_routes.rs`), MCP Streamable HTTP as single JSON-RPC requests with plain JSON responses. Same auth gate as the rest of `/mcp`. Limits stated in the code: no batch arrays, no `Mcp-Session-Id`. The local gizzi runtime uses the internal sibling at `/internal/tools/mcp` with a shared secret and `x-allternit-user-id`.

Five AAI tools (listed by `tools/list`, scoped to the caller):

| Tool | Input | Result |
|---|---|---|
| `agents_list` | none | The user's bots with provenance (`kind: native` or vendor, lane, guarantee, binding state) |
| `agent_send` | `bot_id`, `text`, optional `thread_id` (new task thread when omitted), `correlation_id`, `consequential`, `allternit_approval_id` | `{threadId, reply, pending, events}`. Fails with the approval id when approval is needed. |
| `thread_events` | `thread_id`, optional `after` (sequence), `limit` | `{events, cursor}` ascending |
| `approvals_list` | `thread_id`, optional `state` | Approvals on the thread |
| `approval_respond` | (any) | Always refuses. MCP callers are not human, so approvals are answered in Allternit. |

```bash
curl -s -X POST -H "Authorization: Bearer $UT" -H 'content-type: application/json' $HOST/mcp/server -d '{
  "jsonrpc":"2.0","id":1,"method":"tools/call",
  "params":{"name":"agent_send","arguments":{"bot_id":"BOT_ID","text":"say ok"}}}'
```

## A2A

`cmd/allternit-api/src/a2a_routes.rs`, mounted with the authenticated API router (bearer, owner-scoped), protocol version `0.3.0`, JSON-RPC transport.

| Route | Purpose |
|---|---|
| `GET /.well-known/agent-card/:bot_id` and `GET /a2a/bots/:bot_id/agent-card.json` | Agent card for a bot |
| `POST /a2a/bots/:bot_id` | JSON-RPC: `message/send`, `tasks/get`. `tasks/cancel` returns `UNSUPPORTED` ("cancel the thread in Allternit"). |

The card's `url` is `/api/v1/a2a/bots/{bot_id}`. It advertises no streaming or push, a `converse` skill plus one skill per `true` capability, bearer security, and `x-allternit.provenance` (native or vendor/lane/guarantee).

```bash
curl -s -X POST -H "Authorization: Bearer $UT" -H 'content-type: application/json' $API/a2a/bots/BOT_ID -d '{
  "jsonrpc":"2.0","id":1,"method":"message/send",
  "params":{"message":{"messageId":"m-1","parts":[{"kind":"text","text":"say ok"}],
            "metadata":{"consequential":false}}}}'
```

Response is a task: `{kind:"task", id: <threadId>, contextId: <threadId>, status:{state}, artifacts:[{parts:[{kind:"text",text:<reply>}]}]}`. Pass `message.contextId` or `taskId` to continue a thread, otherwise a new task thread is created. `messageId` becomes the correlation id.

Task state mapping from thread status: `queued` to `submitted`; `planning`, `working` to `working`; `needs_you`, `blocked`, `paused` to `input-required`; `done`, `review`, `idle` to `completed`; `failed` to `failed`. A 428 becomes an `input-required` task with `approvalId` in metadata. It is never auto-approved.

## Not built

Vendor-directory plugins (the spec's "Allternit outward" item) are not in the code. There is no streaming or push on A2A, and no MCP session handling.
