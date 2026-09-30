# Quickstart

**What this is:** the shortest path to see the gateway work: start the services, connect Grok Bot, bind a bot, send a turn, watch events, answer an approval.
**Who it's for:** a developer who has not seen the gateway before.
**Last verified against:** platform commit `c3af0730ca`, allternit-ai commit `d879af36`. Commands were read from the code, not replayed end to end. Where output is shown it is the shape from the handlers and tests, not captured output.

Read [README.md](README.md) first for what the gateway is, and [GLOSSARY.md](GLOSSARY.md) for terms.

## Two paths, and a constraint

```
Path A (gateway only)              Path B (full stack)
curl -> subscription-gateway       web / curl -> allternit-api -> Sessions computer -> subscription-gateway -> adapter
        /aai/call -> adapter
```

- **Path A** runs on your machine with one process. It proves an adapter and the AAI contract. Grok Bot was verified live this way.
- **Path B** is the real product flow: bindings, threads, ledger, approvals, the web UI. **allternit-api only reaches the gateway through a bound Sessions computer** (`PUT /api/v1/subscriptions/binding`). The bind is refused for "the desktop you work on" (`not_a_sessions_computer` in `subscription_routes.rs`) and for a bot's computer. So Path B needs a separate running computer that hosts the gateway. Whether Grok Bot's desktop app can be driven from there is unverified. For a first look, do Path A, then do the Path B steps against a Sessions computer or read them as reference.

## 1. Start the subscription gateway

```bash
cd services/subscription-gateway
pnpm install
pnpm build
export SUBS_GATEWAY_STATE_DIR=~/.allternit/subscriptions/   # default
export SUBS_GATEWAY_KEYCHAIN=file                           # "file" or "keychain" (default keychain)
export SUBS_GATEWAY_TCP=1                                   # also listen on TCP (default: unix socket only)
export SUBS_GATEWAY_TCP_PORT=7788                           # default 7788, host default 127.0.0.1
export SUBS_GATEWAY_API_BASE=http://127.0.0.1:18013         # where allternit-api runs (loopback provider)
pnpm start                                                  # or `pnpm dev` (tsx watch)
```

The log says `subscription-gateway: listening on http://127.0.0.1:7788 (token required)`. Vendor adapters load at boot from `adapters/*/aai.ts`.

The bearer token is issued at first boot under account `cli-token`. Read it from the keychain, or with `SUBS_GATEWAY_KEYCHAIN=file` from `$SUBS_GATEWAY_STATE_DIR/keychain.json`. You can also pin one with `SUBS_GATEWAY_TOKEN`.

```bash
export TOKEN=...   # the gateway caller token
curl -s -H "Authorization: Bearer $TOKEN" http://127.0.0.1:7788/aai/providers | jq '.providers[] | {adapterId, disabled, error}'
```

Expected shape: `{"providers":[{"adapterId":"allternit-loopback",...},{"adapterId":"grok-bot",...},{"adapterId":"claude-desktop",...},{"adapterId":"claude-managed-agents",...},{"adapterId":"chatgpt-dots",...},{"adapterId":"openclaw",...}]}`, each with `disabled`, an optional `pacing`, and its capability `manifest` (or an `error`).

## 2. Check an adapter offline (no vendor needed)

```bash
curl -s -X POST -H "Authorization: Bearer $TOKEN" http://127.0.0.1:7788/aai/conformance/openclaw | jq '{adapterId, lane, guarantee, ok, summary}'
```

Shape: `{"adapterId":"openclaw","lane":"local","guarantee":"exact","ok":...,"summary":{"pass":n,"fail":n,"skippedUnsupported":n}}`. Without fixtures registered the route falls back to a bare `agentId`; the per-adapter fixtures in `adapters/<id>/fixtures/offline.ts` are what the test suite uses ([ADAPTERS.md](ADAPTERS.md#fixtures-offline-mode-and-conformance)).

## 3. Path A: talk to Grok Bot through AAI

Consent step, done by you: quit Grok Bot, then relaunch it with a debugging port. The adapter never restarts your app.

```bash
osascript -e 'quit app "Grok Bot"'
open -a "Grok Bot" --args --remote-debugging-port=9222     # 9222 is the default; override with SUBS_GATEWAY_GROK_BOT_CDP_PORT
```

Every AAI call is `POST /aai/call` with `{op, binding, input}`. AAI errors also answer HTTP 200 (`{ok:false,error}`); only a bad body or missing adapter is non-200.

```bash
BINDING='{"id":"b1","botId":"bot-1","type":"vendor","mode":"linked","vendor":"grok","adapterId":"grok-bot","state":"READY"}'
call() { curl -s -X POST -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' http://127.0.0.1:7788/aai/call -d "$1"; }

call "{\"op\":\"agent.list\",\"binding\":$BINDING,\"input\":{}}"
# {"ok":true,"value":[{"agentId":"grok-bot","displayName":"Grok Bot","vendor":"grok","state":"..."}, ...]}

call "{\"op\":\"agent.context.open\",\"binding\":$BINDING,\"input\":{\"agentId\":\"grok-bot\",\"title\":\"hello\"}}"
# {"ok":true,"value":{"contextId":"...","isolation":"shared","guarantee":"best_effort","resumed":false}}

call "{\"op\":\"agent.context.message\",\"binding\":$BINDING,\"input\":{\"contextId\":\"CTX\",\"correlationId\":\"c-1\",\"text\":\"say ok\"}}"
call "{\"op\":\"agent.events\",\"binding\":$BINDING,\"input\":{\"contextId\":\"CTX\"}}"
# {"ok":true,"value":{"events":[{"type":"agent.context.opened",...},{"type":"agent.message.completed",...}],"cursor":"..."}}
```

Notes:

- Grok Bot's "New chat" is a Bot picker. If it is showing, `agent.list` also returns one `grok-bot:<name>` agent per bot in the picker.
- Replaying the same `correlationId` returns the first result and does not send twice.
- If Grok Bot is not running with the port you get `VENDOR_UNAVAILABLE`; signed out gives `AUTH_REQUIRED`; a verification banner gives `LANE_BLOCKED` (latched).
- The loopback provider (`allternit-loopback`) is an Allternit bot reached through the same interface. Set `SUBS_GATEWAY_AAI_LOOPBACK_BOTS=bot-1` and `SUBS_GATEWAY_AAI_LOOPBACK_TOKEN=<allternit-api bearer>`.

## 4. Path B: start allternit-api

```bash
export ALLTERNIT_API_PORT=18013                  # dev default 18013; production pins 8013
export ALLTERNIT_ENCRYPTION_KEY=$(openssl rand -hex 32)   # needed to store user API keys; without a key, POST .../secret is 503
cargo run -p allternit-api                       # uses the shared CARGO_TARGET_DIR, see OPERATIONS.md
```

On startup the refinery migrations apply V198, V199 and V200 ([OPERATIONS.md](OPERATIONS.md#migrations)). Every request below needs `Authorization: Bearer <your user token>`, the same token the web app sends. Resources are owner-scoped: another user's id returns 404.

```bash
export API=http://127.0.0.1:18013/api/v1 ; export UT=...   # user token
api() { curl -s -H "Authorization: Bearer $UT" -H 'content-type: application/json' "$@"; }

# bind the Sessions computer that hosts the gateway (checked against the live gateway before it is stored)
api -X PUT $API/subscriptions/binding -d '{"computer_id":"<id>","guest_port":7788,"token":"<gateway token>"}'
```

(`PutBindingRequest` in `subscription_routes.rs` takes `computer_id`, optional `guest_port`, `token`. `GATEWAY_OFFLINE` 503 means no gateway is connected.)

## 5. Connect the vendor account and bind a bot

Request bodies use camelCase field names (`authType`, `accountBindingId`), responses too.

```bash
# account: a browser/desktop session, no secret stored
api -X POST $API/gateway/provider-accounts -d '{"vendor":"grok","authType":"desktop_session","displayName":"My Grok Bot"}'
# 201 {"account":{"id":"acc-1","vendor":"grok","authType":"desktop_session","state":"DISCONNECTED","hasSecretRef":false,...}}

# walk the state machine one legal hop at a time (409 lists the allowed next states)
for s in CONSENT_REQUIRED AUTHENTICATING VERIFYING CONNECTED; do
  api -X PATCH $API/gateway/provider-accounts/acc-1 -d "{\"state\":\"$s\"}" >/dev/null
done

# discover agents through the gateway
api $API/gateway/provider-accounts/acc-1/agents
# {"agents":[{"externalAgentId":"grok-bot","name":"Grok Bot"}]}

# bind an existing Allternit bot to it
api -X PUT $API/gateway/bots/BOT_ID/execution-binding \
  -d '{"type":"vendor","mode":"linked","vendor":"grok","adapterId":"grok-bot","accountBindingId":"acc-1","preferredLane":"ui_bridge","externalAgentId":"grok-bot"}'
# 201 {"binding":{"id":"...","botId":"BOT_ID","state":"UNBOUND"|"BOUND",...}}
api -X PATCH $API/gateway/bots/BOT_ID/execution-binding -d '{"state":"READY"}'   # legal moves only; BOUND -> READY
```

The web wizard does the same walk (`driveAccount`). `PATCH ... state READY` here stands in for the capability probe; a vendor bot that is not `READY` answers 409 `BINDING_NOT_READY` and never falls back to a native brain.

For an API-key vendor (Claude Managed Agents), create the account with `"authType":"api_key"`, then `POST .../secret` with `{"apiKey":"..."}` (sealed, never echoed).

## 6. Send a turn and watch events

Threads and sessions are the existing Allternit ones. A vendor turn is the normal message route.

```bash
api -X POST $API/agent-sessions/SESSION_ID/messages -d '{"text":"say ok","metadata":{"correlationId":"c-2"}}'
# 200 assistant message attributed to the vendor bot
#  409 BINDING_NOT_READY | CONTEXT_LOST | ...   428 APPROVAL_REQUIRED   429 RATE_LIMITED (retryAfterMs)   502/503

api -X POST $API/threads/THREAD_ID/gateway/sync          # {"events":n}: pull remote events now
api "$API/threads/THREAD_ID/events?after=0&limit=100"     # ascending by sequence
api $API/gateway/threads/THREAD_ID/remote-bindings        # one row per generation, with the frozen lane
```

Events carry `{id, sequence, type, actor, payload, sessionId, occurredAt}`. Poll with `after=<last sequence>`. The web app polls every 1.5 s while a thread is working and backs off to 15 s idle.

## 7. Approvals

Consequential turns need an Allternit approval first: send with `"metadata":{"consequential":true}`. The response is 428 with `approvalId`. A person approves, then the turn is resent with `allternitApprovalId`.

```bash
api "$API/threads/THREAD_ID/approvals?state=pending"
# {"approvals":[{"id":"...","authority":"allternit"|"vendor","action":"...","state":"pending",...}]}

api -X POST $API/gateway/approvals/APPROVAL_ID/respond -d '{"decision":"approve","actor":{"type":"user"}}'
# {"approvalId":"...","state":"approved"}     403 HUMAN_REQUIRED for any non-user actor; 409 ALREADY_RESOLVED
```

A vendor's own approval (`authority: vendor`) appears on the thread the same way and leaves it in `needs_you` until a person answers. An Allternit approval never resolves a vendor one.

## 8. Web app

```bash
cd allternit-ai && pnpm install && pnpm dev     # http://localhost:3013
```

Open a Bot, then its config tab **Agent Gateway** (`BotConfigTab.tsx`, tab id `gateway`). You get the vendor library, connection cards and the wizard. Vendor-bound threads show the provenance bar and, for Grok Bot and Claude, their look pack. See allternit-ai `docs/gateway-ui.md`.

## 9. The same flow with the SDKs

TypeScript (`@allternit/aai-sdk`):

```ts
import { AllternitAgents, ApprovalRequiredError } from "@allternit/aai-sdk";
const aai = new AllternitAgents({ baseUrl: "http://127.0.0.1:18013", token: userToken });

const { account } = await aai.request("POST", "/gateway/provider-accounts", { vendor: "grok", authType: "desktop_session" });
for (const s of ["CONSENT_REQUIRED", "AUTHENTICATING", "VERIFYING", "CONNECTED"]) await aai.accounts.setConnectionState(account.id, s as any);
const { agents } = await aai.accounts.discoverAgents(account.id);
await aai.request("PUT", `/gateway/bots/${botId}/execution-binding`, { type: "vendor", mode: "linked", vendor: "grok", adapterId: "grok-bot", accountBindingId: account.id, externalAgentId: agents[0].externalAgentId });
await aai.bots.setBindingState(botId, "READY");
try { await aai.threads.sendTurn(sessionId, "say ok"); }
catch (e) { if (e instanceof ApprovalRequiredError) console.log("needs a person:", e.approvalId); else throw e; }
for await (const ev of aai.threads.streamEvents(threadId, { after: 0 })) console.log(ev.sequence, ev.type);
await aai.approvals.respond(approvalId, "approve", { humanIntent: true });   // refuses without humanIntent
```

Python (`allternit-aai`):

```python
from allternit_aai import AllternitAgents, ApprovalRequiredError
aai = AllternitAgents("http://127.0.0.1:18013", token=user_token)

acct = aai.request("POST", "/gateway/provider-accounts", {"vendor": "grok", "authType": "desktop_session"})["account"]
for s in ["CONSENT_REQUIRED", "AUTHENTICATING", "VERIFYING", "CONNECTED"]:
    aai.set_connection_state(acct["id"], s)
agents = aai.discover_agents(acct["id"])["agents"]
aai.request("PUT", f"/gateway/bots/{bot_id}/execution-binding", {"type": "vendor", "mode": "linked", "vendor": "grok", "adapterId": "grok-bot", "accountBindingId": acct["id"], "externalAgentId": agents[0]["externalAgentId"]})
try:
    aai.send_turn(session_id, "say ok")
except ApprovalRequiredError as e:
    print("needs a person:", e.approval_id)
for ev in aai.stream_events(thread_id, after=0):
    print(ev["sequence"], ev["type"])
aai.respond_approval(approval_id, "approve", human_intent=True)
```

More in [FACADES.md](FACADES.md). If a step fails, [ARCHITECTURE.md](ARCHITECTURE.md#14-how-to-debug-a-vendor-turn) lists what to check at each hop.
