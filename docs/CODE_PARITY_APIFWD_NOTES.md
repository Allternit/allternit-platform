# APIFWD notes — allternit-api passes code parity data to gizzi (2026-10-20)

## Built
All in `cmd/allternit-api/src/agent_session_routes.rs` unless noted.

1. **Create** `POST /api/v1/agent-sessions` accepts `environmentId` (or `environment_id`) and
   `repos: [{url, branch?}]`, top-level or inside `metadata`. Forwarded to gizzi `POST /v1/session`
   as `environmentId` / `repos` (Contract C names). Empty id / empty array is not forwarded.
2. **Reads** (get, list, `created` sync event): gizzi's `environment` object `{id,name,status,error?}`
   passes through unchanged as top-level `environment` AND `metadata.environment`.
   The `updated` sync event carries it the same way (top-level + `metadata.environment`).
3. **Sync SSE** `GET /api/v1/agent-sessions/sync` now relays:
   - gizzi `session.environment.updated` → `{ "type": "session_environment", "session_id", "environment": {id,name,status,error?} }`
     (uses `props.environment`; if gizzi sends the fields flat it uses props minus `sessionID`)
   - gizzi `session.prs.updated` → `{ "type": "session_prs", "session_id", "prs": LinkedPR[] }`
     — identical name/shape to PRLINK's agent-compat stream, so desktop-direct and cloud match.
4. **Linked PR proxies** (→ gizzi `/v1/session/...`, same auth as the other agent-session routes —
   they have no per-route ownership check beyond the API's auth middleware; placed sessions pass
   through via `placement::passthrough`):
   - `GET    /api/v1/agent-sessions/prs?ids=a,b` → `{ bySession }`
   - `GET    /api/v1/agent-sessions/:id/prs` → `{ prs }`
   - `POST   /api/v1/agent-sessions/:id/prs/refresh` → `{ prs }`
   - `POST   /api/v1/agent-sessions/:id/prs` body `{url}` → `{ prs }` (400 if url missing)
   - `DELETE /api/v1/agent-sessions/:id/prs?url=` → `{ prs }` (400 if url missing)
5. `cmd/allternit-api/src/placement.rs`: `session_in_path` excludes `prs` (like `sync`) so the bulk
   route is never treated as a placed session id; test lines added.

## Route overlap check
`/agent-sessions/prs` is a static segment next to `/agent-sessions/:id` (same pattern as the
existing `/agent-sessions/sync`, matchit prefers static). `/agent-sessions/:id/prs` and
`/:id/prs/refresh` exist nowhere else (grepped every router merged in `main.rs`: card_meta,
canvas_routes, thread_routes, placement). Uses `:id` like the rest of this router.
The router test at the bottom of agent_session_routes.rs builds the router, so the
orchestrator's single test run will catch any overlap panic.

## Docs
`surfaces/docs/api/agent-sessions.mdx`: session `environment` field, create `environmentId`/`repos`,
the two new sync event types, a Linked pull requests section with the five routes.
check_links.py: 0 problems; docs-lint: pass. No cross-links to the ENVAPI/PRLINK guide pages
because they are not on this branch yet (add after merge if wanted).

## Unfinished / assumptions
- factory/cp-envrt was not pushed when I worked, so the `session.environment.updated` payload
  shape is taken from Contract C (`{sessionID, environment}`), with a flat fallback.
- Not built/checked (code-only rule). Orchestrator: `cargo check -p allternit-api` + router test.
