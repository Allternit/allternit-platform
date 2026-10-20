# Task APIFWD — allternit-api passes code parity data through to gizzi
Read docs/CODE_PARITY_MAP.md first. Your tag: APIFWD. On cloud, the web app talks to allternit-api `/api/v1/agent-sessions*`, which proxies to gizzi. Contracts A and C are built in gizzi (branches factory/cp-prlink, factory/cp-envrt — you can read them with `git show origin/factory/cp-prlink:<path>` after `git fetch`), but allternit-api drops the new fields.

1. `cmd/allternit-api/src/**/agent_session_routes.rs` `create_session`: forward `environmentId` and `repos` (accept camelCase and snake_case) to gizzi's session create.
2. Session reads (get/list): pass gizzi's `environment` object `{id,name,status,error?}` through unchanged.
3. Sync SSE (`/api/v1/agent-sessions/sync` and whatever the web subscribes to): relay gizzi bus events `session.environment.updated` and `session.prs.updated` (PRLINK also forwards `session_prs` in gizzi's agent-compat — read origin/factory/cp-prlink `cmd/gizzi-code/src/runtime/server/routes/agent-compat.ts` and match the event names the web will see).
4. Proxy Contract A routes: `GET /api/v1/agent-sessions/prs?ids=`, `GET /api/v1/agent-sessions/:id/prs`, `POST /api/v1/agent-sessions/:id/prs/refresh`, `POST /api/v1/agent-sessions/:id/prs` {url}, `DELETE /api/v1/agent-sessions/:id/prs?url=` → the matching gizzi `/v1/session/...` routes, same auth/ownership checks as the other agent-session routes. Register `/prs` before `/:id` style matchers. CHECK for route overlap: overlapping axum routes crash-loop production (this happened before) — read the router carefully.
5. Note in NOTES the exact paths/events the web should use.
Code only, no cargo build/check/test. Commit, push, NOTES + sentinel per the map.
