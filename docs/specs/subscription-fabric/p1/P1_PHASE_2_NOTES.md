---
status: complete
files_changed:
  - services/subscription-gateway/package.json (added zod ^3.25.0 dependency)
  - services/subscription-gateway/src/config.ts (additive: apiBase field)
  - services/subscription-gateway/src/store/queries.ts (additive: listAccounts, getTaskByIdempotency, getArtifact)
  - services/subscription-gateway/src/events/log.ts
  - services/subscription-gateway/src/events/sse.ts
  - services/subscription-gateway/src/events/outbox.ts
  - services/subscription-gateway/src/events/notify.ts
  - services/subscription-gateway/src/http/server.ts
  - services/subscription-gateway/src/http/routes_tasks.ts
  - services/subscription-gateway/src/http/routes_events.ts
  - services/subscription-gateway/src/http/routes_artifacts.ts
  - services/subscription-gateway/src/http/routes_accounts.ts
  - services/subscription-gateway/src/http/routes_capabilities.ts
  - services/subscription-gateway/src/router/resolve.ts
  - services/subscription-gateway/src/main.ts
  - services/subscription-gateway/test/helpers.ts
  - services/subscription-gateway/test/http.test.ts
  - services/subscription-gateway/test/events.test.ts
  - services/subscription-gateway/test/outbox-replay.test.ts
  - services/subscription-gateway/test/notify.test.ts
  - services/subscription-gateway/test/boot.test.ts
deviations:
  - zod ^3.25.0 added to package.json dependencies — the routers validate with
    contracts schemas and pnpm's isolation was resolving a mismatched zod 4.
    Required defect fix, not a design change.
  - config.ts gained an additive `apiBase` field (env SUBS_GATEWAY_API_BASE,
    default http://127.0.0.1:18013) because notify.ts's contract is "apiBase
    from config". No Phase 1 behavior changed.
  - Host validation compares the Host header's port against the port the
    connection actually arrived on (req.socket.localPort) rather than a fixed
    configured port — equivalent DNS-rebinding defense, and it keeps the guard
    correct for ephemeral test ports and the UDS `localhost` placeholder.
  - outbox.deliverUndelivered takes an options object { sinceEventId?, taskId?,
    limit? } (superset of the spec's positional signature). taskId scoping
    exists so per-task SSE replay doesn't mark other tasks' events delivered
    without sending them; sinceEventId marks everything up to the cursor
    delivered and hands out only later events.
  - Account-scoped needs_user ledger entries use the synthetic task_id
    `account:<id>` because the events table is task-keyed (task_id NOT NULL).
  - requester.kind on task submit defaults to "bot" and is overridable via the
    body's optional requester_kind (tokens carry no kind; §A6.2 assigns kinds
    to callers, not tokens).
  - RouteDecision from the static router also carries decision_id (uuid) and
    policy_version "p1-static" — both are required by the contracts
    routeDecisionSchema; the spec's primary/fallbacks/rejected/explain values
    are verbatim.
remaining:
  - Port 7788 was already present in docs/Operations/QUICK_REFERENCE.md and
    PORT_REGISTRY.md port tables — no edit needed (verified 2026-09-26).
  - MCP notify channel is NoopMcpNotifier with a TODO(P5) per the task spec.
  - pruneAckedOlderThan exists but is unscheduled — call-site owns scheduling.
  - Master key (getOrCreateMasterKey) still unused for at-rest encryption.
  - Worker/queue/adapters (P2/P3), real router (P4), MCP surface (P5).
verify:
  - pnpm -F subscription-gateway build — PASS (tsc -b, 0 errors)
  - pnpm -F subscription-gateway test — PASS (10 files, 77 tests)
  - grep -rniE "chatgpt|claude|kimi|gemini|grok|deepseek|openai|anthropic"
    services/subscription-gateway — PASS (no matches)
  - Live daemon smoke (real keychain gate, real UDS + TCP binds):
    UDS /v1/capabilities with token → 200 []; TCP without token → 401;
    TCP with token → 200 []; bad Host → 403; any Origin → 403;
    socket mode 0600; /v1/health unauthenticated → 200.
---

# P1 Phase 2 — Subscription Gateway events spine + HTTP surface + boot

## What was built

- **events/log.ts** — `EventLog.append({ task_id, kind, payload, callers })`:
  ledger insert with monotonic per-task `seq`, fan-out to `caller_outbox` for
  each caller, publish to the SSE hub (task + thread channels), and
  terminal-status detection that triggers the notifier. Kinds cover the §A1
  AdapterEvent tags plus `progress.heartbeat` (D11), `task.created`,
  `task.status`, and internal `notify.failed`.
- **events/sse.ts** — in-process hub. Per-subscriber queues, cap 1000:
  drop-oldest beyond the cap and deliver a `gap` marker (`{ kind: "gap",
  dropped }`) when the consumer drains. Task and per-thread subscriptions.
- **events/outbox.ts** — the D12 spine: enqueue (INSERT OR IGNORE),
  deliverUndelivered/replay with cursor + task scoping, ack (re-ack no-op),
  markDelivered for live SSE writes, pruneAckedOlderThan(7d).
- **events/notify.ts** — terminal push: bot requesters get a Factory peer
  message (`POST {apiBase}/api/factory/peers/:name/send`, body `{ body, from }`
  matching the Factory peers route); user/cli/system requesters get a JSON drop file
  in `.allternit/notifications/`; MCP via stub interface + NoopMcpNotifier.
  Failures never throw into the event path — they land as `notify.failed`
  ledger rows. Injectable fetch; `drain()` for tests/shutdown.
- **http/server.ts** — `createServer(deps)` with express.json → hostGuard
  (loopback hostnames only, port must match the arrival port) → originGuard
  (default empty allowlist: any Origin → 403) → unauthenticated
  `GET /v1/health` → bearer auth on everything else → routers. `listenUds`
  unlinks stale sockets and chmods 0600; `listenTcp` for the optional
  loopback TCP transport.
- **http/routes_tasks.ts** — submit (scope tasks:submit, contracts-schema
  validation, idempotency-key replay returns the existing task, persists
  `queued` + `task.created` event), get (tasks:read), cancel (tasks:submit,
  only from queued/needs_user else 409).
- **http/routes_events.ts** — SSE `GET /v1/tasks/:id/events`: outbox replay
  first (missed events, marked delivered), then live hub events (enqueued to
  the caller's outbox and marked delivered only on successful write — a dead
  connection leaves rows for the next replay), 15 s heartbeat comment, gap
  markers. `POST /v1/events/ack` for batch acks.
- **http/routes_artifacts.ts** — metadata lookup (artifacts:read), 404 with
  `{ error: "artifact_not_found" }` when absent.
- **http/routes_accounts.ts** — list (accounts:manage), status
  (accounts:manage OR tasks:read), connect placeholder creating an
  `auth_required` row + `needs_user` ledger entry.
- **http/routes_capabilities.ts** — `GET /v1/capabilities` → `[]` (the P1
  smoke-pinned behavior), plus unauthenticated `/v1/health`.
- **router/resolve.ts** — `StaticRouter implements CapabilityRouter` returning
  `primary: null`, empty fallbacks, `rejected: [{ adapter_id: "*", reason:
  "capability_not_offered" }]`, explain "no adapters registered (router lands
  in P4)"; `onAttemptFailed` → "stop".
- **main.ts** — `boot(deps)`: loadConfig → requireKeychain (D3: logs one line,
  exit(1), rethrows for tests) → openDatabase+migrations → events/http wiring
  → UDS (+TCP when enabled) → logs bound addresses. `close()` drains
  notifier, closes servers, WAL-checkpoints. SIGTERM/SIGINT graceful
  shutdown in the CLI entry path.

## Verification evidence

- 77/77 tests across 10 files, including the **D12 gate**: a caller connects
  over SSE, receives replay + live events, disconnects mid-stream, and on
  reconnect receives exactly the two missed events exactly once by event_id —
  then acks and a further replay yields nothing. No caller polling required
  to learn a terminal state.
- Keychain-refusal boot tested via exported `boot()` with injected backend,
  logger, and exit (D3). Phase 1 migration-idempotency test still green.
- Live smoke against the real daemon (booted via `src/main.ts` through the
  real macOS keychain gate): every transport/auth/host/origin assertion from
  the P1 plan verify confirmed with curl over both UDS and TCP. The scratch
  smoke driver, temp state dir, and process were removed afterward.
