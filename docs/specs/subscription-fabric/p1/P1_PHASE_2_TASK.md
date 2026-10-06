# P1 Phase 2 Task — subscription-gateway: events spine + HTTP surface + boot

You are building Phase 2 of 2 of `services/subscription-gateway/`. Phase 1 is complete and reviewed: scaffolding, `src/config.ts`, `src/store/*` (db, migrations, queries), `src/security/*` (tokens, keychain, redact, navlock) exist with passing tests. **Build on them as-is; do not rewrite Phase 1 files.** Genuine defects: minimal fix + record under `deviations`.

## Normative sources (read first)

- `docs/specs/subscription-fabric/IMPLEMENTATION_PLAN.md` §1 layout lines 57–99, **P1 verify + gate at lines 138–143** (your completion bar)
- `docs/specs/subscription-fabric/HARDENING.md` — **D12 lines 96–104** (caller_outbox, acked delivery, terminal-state notify), D11 lines 90–94 (event shapes incl. `progress.heartbeat`), D3 line 21
- `docs/specs/subscription-fabric/REVIEW_CLAUDE.md` — **§A6 lines 480–491** (transport/Host/Origin rules, scopes), §A7 line 506 (SSE + ReplyEvent)
- Contracts: `@allternit/subscription-fabric-contracts` (`workspace:*`) — use its `AdapterEvent`, `TaskStatus`, scope types; do not redeclare.

## Exact deliverables

1. `src/events/log.ts` — append-only event ledger over the `events` table (mirrors the repo's `bot_events` pattern): `appendEvent(db, { task_id, kind, payload, callers })` → assigns `event_id` (uuid), monotonic per-task `seq`, fans out to `caller_outbox` rows for each subscribed caller + the SSE hub. Kinds = the §A1 AdapterEvent tags + `progress.heartbeat` (D11) + lifecycle (`task.created`, `task.status`).
2. `src/events/sse.ts` — in-process pub/sub hub: `subscribe(taskId, listener)` / `publish(event)` / per-thread subscription support. Backpressure: drop-oldest beyond 1000 queued per subscriber, with a `gap` marker event.
3. `src/events/outbox.ts` — **the D12 completion-push spine**: `enqueue(event, callerId)`, `deliverUndelivered(callerId, sinceEventId?)`, `ack(callerId, eventId)`, `replay(callerId)` on reconnect. At-least-once delivery, **idempotent on `event_id`** (re-ack is a no-op; replay after ack yields nothing). Retention: prune acked rows older than 7 days (function exists; scheduling later).
4. `src/events/notify.ts` — terminal-state push (D12): on `completed | partial | needs_user | failed`, notify the requester through its channel of record. Channels: Factory peer message (`POST {apiBase}/api/factory/peers/:name/send`, apiBase from config, default `http://127.0.0.1:18013`), desktop-shell notification (write a `.allternit/notifications/` drop file — the shell picks it up; document this), MCP resource update (stub interface + no-op impl with a TODO comment — MCP surface is P5). **Never throw into the event path**: notify failures are logged to the ledger as `notify.failed` and swallowed. Injectable fetch for tests.
5. `src/http/server.ts` — express app factory `createServer(deps)` (deps injected: db, config, keychain, eventLog, outbox, sseHub, notifier). Transport per §A6.1:
   - UDS default: `app.listen(config.udsPath)`, `chmod 0600` after listen, unlink stale socket before bind.
   - Optional TCP `127.0.0.1:7788` only when `config.tcpEnabled` — **always** token-authed.
   - **Host validation**: reject (403) any request whose `Host` isn't `127.0.0.1:<port>` / `localhost:<port>` / the UDS placeholder `localhost` (defeats DNS rebinding).
   - **Origin validation**: reject (403) any request carrying an `Origin` header not on an explicit allowlist (default: empty — reject all origins). Never send CORS `*`.
   - Bearer auth middleware on everything except `GET /v1/health`: `verifyToken`, scope enforcement per route. 401 without valid token.
6. `src/http/routes_tasks.ts` — `POST /v1/tasks` (scope `tasks:submit`; validate body against the contracts `taskSchema` subset — full worker execution is later phases, so tasks persist as `queued` and a `task.created` event is appended), `GET /v1/tasks/{id}` (scope `tasks:read`), `POST /v1/tasks/{id}/cancel` (scope `tasks:submit`; only from `queued`/`needs_user`, else 409).
7. `src/http/routes_events.ts` — `GET /v1/tasks/{id}/events` (scope `tasks:read`): SSE stream from the hub, **starting with outbox replay** for the requesting caller (D12: a reconnected caller receives missed events before live ones, idempotent on `event_id`), heartbeat comment every 15 s. Also `POST /v1/events/ack` (body: `{ event_ids: string[] }`) → outbox ack.
8. `src/http/routes_artifacts.ts` — `GET /v1/artifacts/{id}` metadata (scope `artifacts:read`; row lookup only — store/export/download land with real artifacts in P3+; return 404 with a clear body when absent).
9. `src/http/routes_accounts.ts` — `GET /v1/accounts` + `GET /v1/accounts/{id}/status` (scope `accounts:manage` OR `tasks:read` for status), `POST /v1/accounts` connect placeholder (scope `accounts:manage`; creates the account row with `session_health: auth_required` and a `needs_user` entry — real browser flows are P3). Bots never get `accounts:manage` — that's enforced at token issue, not here.
10. `src/http/routes_capabilities.ts` — `GET /v1/capabilities` (any valid token) → **live registry view**: for P1 the registry is empty (no adapters loaded), so it returns `[]` — this exact behavior is in the P1 smoke verify. Also `GET /v1/health` (unauthenticated): `{ ok: true, name: "subscription-gateway", version }`.
11. `src/router/resolve.ts` — **static stub**: implements `CapabilityRouter` from contracts; `resolve` returns a `RouteDecision` with `primary: null`, `fallbacks: []`, `rejected: [{ adapter_id: "*", reason: "capability_not_offered" }]`, `explain: "no adapters registered (router lands in P4)"`. Real routing is P4.
12. `src/main.ts` — boot sequence, in order: `loadConfig` → `requireKeychain()` (**refuse boot on `KeychainUnavailable`** — log one line, `process.exit(1)`; D3 structural rule) → open store + run migrations → wire events/http → listen UDS (+ TCP if enabled) → log the bound address(es). Graceful SIGTERM/SIGINT: close server, checkpoint WAL, exit 0.
13. `test/` — vitest, supertest against the express app **and** a real UDS listener:
    - UDS end-to-end: boot on a temp socket, request over it with a valid token → 200.
    - TCP without token → 401; valid token → 200.
    - Bad `Host` → 403. Any `Origin` (default allowlist) → 403.
    - Scope enforcement: `tasks:read`-only token POSTing `/v1/tasks` → 403.
    - **Outbox replay (D12 gate)**: submit task, generate events, simulate a subscriber that disconnects mid-stream, reconnect, assert it receives exactly the missed events **exactly once** (idempotent on `event_id`), then ack and assert a second replay yields nothing.
    - Terminal-state notify: force a task to `completed` via the event log, assert the notifier fired through the injected fetch (Factory peer payload shape) and that a notify failure produces a `notify.failed` ledger row, not a crash.
    - Keychain-refusal boot: invoke the boot function with a keychain backend that throws → exits/refuses (test the exported `boot()` with injected deps, not a real process).

## Hard gates (the P1 gate from IMPLEMENTATION_PLAN lines 138–143)

- `pnpm -F subscription-gateway build` and `pnpm -F subscription-gateway test` PASS.
- Keychain-refusal boot path tested (D3). Migrations idempotent (Phase 1 test still green). No caller-polling needed for terminal state — proven by the outbox replay test.
- No provider-name literals anywhere in the service.
- No comments narrating code; one-line spec pointers only.

## Completion sentinel

Write `docs/specs/subscription-fabric/p1/P1_PHASE_2_NOTES.md` with YAML frontmatter (`status`, `files_changed`, `deviations`, `remaining`, `verify`) then prose. That file existing = done.
