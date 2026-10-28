# OpenBot CopilotKit adapter

Primary source: https://github.com/CopilotKit/OpenBot @ `6fb38c8bec70d1c29a6810e7c7152bf08eb7c626`.
Files read: `server/src/auth/index.ts` (Better Auth config, no custom cookie prefix), `server/src/auth/guards.ts` (`getSession` on the request cookies), `server/src/auth/organization.ts` (accepted cookie form `(__Secure-)?better-auth.session_token`), `server/src/agents/routes.ts` (`GET /` mounted at `/api/agents`), `server/src/channels/thread-routes.ts` (`POST /mint`, `GET /:threadId` mounted at `/api/threads`), `server/src/copilot.ts` and `server/src/app.ts` (CopilotKit runtime mounted at `/api/copilotkit`, Intelligence mode).
Client: own client `adapters/_shared/copilotkit-intelligence.ts`, wire-compatible with `@copilotkit/core` 1.75.0 `IntelligenceAgent` (run mode). Pinned sources it reproduces: `src/intelligence-agent.ts` of the published npm package `@copilotkit/core@1.75.0` (github.com/CopilotKit/CopilotKit, read from its `dist/index.mjs` + source map) (POST `/agent/:id/run` RunAgentInput, receipt `{threadId, runId, joinToken, realtime:{clientUrl, topic}}`, channel params `{stream_mode:"run", run_id}`, `ag_ui_event` pushes) and the Phoenix 1.8.15 JS client (`<clientUrl>/websocket?join_token=…&vsn=2.0.0`, `[join_ref, ref, topic, event, payload]` frames, `phx_join`/`phx_reply`/`phx_leave`, 30 s heartbeat). No npm dependency. Same Intelligence protocol as `adapters/opendots-copilotkit`.

## Credential

Your own session from your own sign-in to your OpenBot host, as JSON:

```json
{ "origin": "https://your-openbot-host", "sessionToken": "<value of your better-auth session cookie>" }
```

To get the value, sign in to OpenBot in a browser, open the site's cookies, and copy the value of `__Secure-better-auth.session_token` (HTTPS) or `better-auth.session_token` (loopback HTTP). The adapter sends it back under that same cookie name and never persists it.

`origin` must be an https origin, or a loopback http origin. Paths, query strings and credentials are refused.

## Routes

- Discovery: `GET /api/agents`, hidden bots skipped. Each bot is `openbot-copilotkit:<botId>`.
- Conversation: `POST /api/threads/mint` gives a fresh thread id per Allternit context.
- Send and reply: `GET /api/copilotkit/info` (must report `mode: "intelligence"` and expose the bot), then one Intelligence run through `POST /api/copilotkit/agent/:botId/run` and the realtime channel from its receipt. The client can reach only that bot's run route and thread on the approved origin. The reply is the newest assistant message from that run.
- The realtime WebSocket origin defaults to `wss://realtime.intelligence.copilotkit.ai` (a wss origin; an `https://` value never matches a `wss://` endpoint). Set `SUBS_GATEWAY_OPENBOT_COPILOTKIT_REALTIME_ORIGIN` to the host that your deployment's `INTELLIGENCE_GATEWAY_WS_URL` uses. A realtime origin that doesn't match is refused.

## Unsupported

No streaming, approvals, cancel, memory, tools or computer control. Contexts do not survive gateway restarts. There is no `/connect` replay or socket reconnect: a run that times out, errors or loses its socket returns VENDOR_UNAVAILABLE, and the conversation must be checked in OpenBot before retrying.

## Not verified live

Nothing has been run against a live OpenBot host. The realtime origin and the `/info` shape are inferred from the OpenDots adapter and from `copilot.ts`. Check the first live send and set the realtime origin for your deployment.
