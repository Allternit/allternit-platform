# CopilotKit OpenDots native AAI adapter

Primary source: https://github.com/CopilotKit/OpenDots/tree/625452e06cde74cb25b0ce319e2c1be0488f5a5f
Protocol reference: src/server/headless.ts, platform.ts, runtime-scope.ts and workspace-routes.ts in that source revision. Client: own client `adapters/_shared/copilotkit-intelligence.ts`, wire-compatible with `@copilotkit/core` 1.75.0 `IntelligenceAgent` (run mode). Pinned sources it reproduces: `src/intelligence-agent.ts` of the published npm package `@copilotkit/core@1.75.0` (github.com/CopilotKit/CopilotKit, read from its `dist/index.mjs` + source map) (POST `/agent/:id/run` RunAgentInput, receipt `{threadId, runId, joinToken, realtime:{clientUrl, topic}}`, channel params `{stream_mode:"run", run_id}`, `ag_ui_event` pushes) and the Phoenix 1.8.15 JS client (`<clientUrl>/websocket?join_token=…&vsn=2.0.0`, `[join_ref, ref, topic, event, payload]` frames, `phx_join`/`phx_reply`/`phx_leave`, 30 s heartbeat). No npm dependency.

Uses the OpenDots runtime at http://127.0.0.1:4310. The vendor must enforce OWNER_TOKEN; an unauthenticated workspace probe must be rejected before Allternit accepts this connection. Supply that owner token through the sealed account credential. SUBS_GATEWAY_OPENDOTS_COPILOTKIT_URL and _TOKEN are deployment overrides. Never substitute OPENAI_API_KEY or INTELLIGENCE_API_KEY for the owner token.

GET /api/workspace discovers Dots. POST /api/conversations creates a bound isolated thread. The client obtains scoped realtime credentials from the vendor's /api/copilotkit/agent/:id/run endpoint and joins the Intelligence WebSocket channel. The adapter validates thread identity and the realtime origin, and returns only this run's new assistant messages (events for another thread or run are refused). It does not assume raw SSE.

Default realtime origin: wss://realtime.intelligence.copilotkit.ai. An administrator may explicitly configure SUBS_GATEWAY_OPENDOTS_COPILOTKIT_REALTIME_ORIGIN for a trusted deployment. Runtime-provided arbitrary origins and foreign-thread receipts are refused. Owner credentials are sent only to the approved loopback runtime; scoped join credentials go only to the approved realtime socket.

No native approval forwarding, cancellation or memory/computer control is advertised. Each send leaves the channel and closes the socket when its run ends; this never stops a vendor run. There is no `/connect` replay or reconnect, so a dropped socket is reported as unconfirmed, not retried. Send ID/text pairs and uncertain outcomes are retained in process. Contexts do not resume across gateway restarts.

Source-based transport fixtures and a fake Phoenix server (test/fake-phoenix.ts) pass. Live vendor-account verification is pending.
