# CopilotKit OpenDots native AAI adapter

Primary source: https://github.com/CopilotKit/OpenDots/tree/625452e06cde74cb25b0ce319e2c1be0488f5a5f
Protocol reference: src/server/headless.ts, platform.ts, runtime-scope.ts and workspace-routes.ts in that source revision. SDK: @copilotkit/core pinned to 1.75.0.

Uses the OpenDots runtime at http://127.0.0.1:4310. The vendor must enforce OWNER_TOKEN; an unauthenticated workspace probe must be rejected before Allternit accepts this connection. Supply that owner token through the sealed account credential. SUBS_GATEWAY_OPENDOTS_COPILOTKIT_URL and _TOKEN are deployment overrides. Never substitute OPENAI_API_KEY or INTELLIGENCE_API_KEY for the owner token.

GET /api/workspace discovers Dots. POST /api/conversations creates a bound isolated thread. The SDK obtains scoped realtime credentials from the vendor's /api/copilotkit/agent/:id/run endpoint and joins the Intelligence WebSocket channel. The adapter validates thread identity and the realtime origin, and returns only this SDK run's new assistant messages. It does not assume raw SSE.

Default realtime origin: wss://realtime.intelligence.copilotkit.ai. An administrator may explicitly configure SUBS_GATEWAY_OPENDOTS_COPILOTKIT_REALTIME_ORIGIN for a trusted deployment. Runtime-provided arbitrary origins and foreign-thread receipts are refused. Owner credentials are sent only to the approved loopback runtime; scoped join credentials go through the SDK.

No native approval forwarding, cancellation or memory/computer control is advertised. Detaching closes the client's SDK subscription without stopping a vendor run. Send ID/text pairs and uncertain outcomes are retained in process. Contexts do not resume across gateway restarts.

Source-based transport fixtures and real pinned SDK import/API checks pass. Live vendor-account verification is pending.
