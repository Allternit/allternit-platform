# Task ENVRT — apply an environment when a cloud session starts (gizzi-code)
Read docs/CODE_PARITY_MAP.md first. Your tag: ENVRT. Build Contract C in `cmd/gizzi-code`. PRLINK works in the same package on PR detection — avoid the tool-output PR detector and `/prs` routes; coordinate only via the contract.

1. Session create accepts `environmentId` and `repos` (route + session schema + persisted). Find how gizzi knows the cloud API base + runtime-device token on a paired/hosted runtime (search runtime pairing / ALLTERNIT_CLOUD / device token); use it to call `GET /api/v1/code-environments/:id/resolve`.
2. Before the first agent turn of such a session: status connecting → setting_up; clone repos into the session workspace (one dir each, branch if given; reuse existing git helpers); parse .env text (support quoted multiline values) into the tool-process env; run setup_script with bash in the workspace, 10 min timeout, stream output into the session as a visible "Setup" step; then ready (or failed with error, and the turn reports it clearly instead of running the agent blind).
3. Secrets: add values to tool-process env only; add a redactor that replaces every secret value (and base64 of it) with `[secret:NAME]` in tool outputs before they are stored, sent to the model, or emitted on the bus.
4. Network policy for tool processes: none → set HTTP(S)_PROXY/ALL_PROXY to a local proxy that refuses everything; limited → local CONNECT/HTTP proxy allowing allowed_hosts + secret hosts (wildcards), refusing others with a clear 403 message; full → nothing. Small proxy inside gizzi (Bun/Node net), one per session, closed when the session ends.
5. `GET /v1/session/:id` gains `environment` status object; bus event `session.environment.updated`.
6. Local sessions (no environmentId) must behave exactly as before.
7. Docs: add a "How a cloud session starts" section to the cloud environments doc (ENVAPI writes the page; you write your section in a separate file `surfaces/docs/code/cloud-environment-runtime.mdx` or the matching folder, linked from nav, so you don't conflict).

## What this can break
cmd/gizzi-code/src → Desktop, Factory, gizzi CLI/TUI, Memory Drive, Voice; journeys "Send a message, get a streamed reply" (5,6), "Text your bot" (5). Any failure in environment setup must fail only that session, never the server.
