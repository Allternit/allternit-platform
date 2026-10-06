# Fabric Transport and Lease Protocol

**Layer:** Execution transport under A://

**Canonical route prefix:** `/api/v1/fabric/transport/*`

**Primary implementation:**

- `cmd/allternit-api/src/rails/fabric_transport_routes.rs`
- `infrastructure/executor/cowork/cowork/allternit-cowork-runtime/src/transport.rs`
- `infrastructure/executor/cowork/cowork/allternit-cowork-runtime/src/sqlite_store.rs`
- `cmd/allternit-api/migrations/V152__cowork_principals.sql`
- `cmd/allternit-api/migrations/V153__cowork_job_lease_columns.sql`
- `cmd/allternit-api/migrations/V154__cowork_event_attribution.sql`
- `cmd/allternit-api/migrations/V155__cowork_approval_bindings.sql`
- `cmd/allternit-api/migrations/V156__cowork_approval_expiry.sql`
- `cmd/allternit-api/migrations/V157__cowork_event_idempotency.sql`
- `cmd/allternit-api/migrations/V158__cowork_approval_policy.sql`

> Migration numbering note: these were renumbered from V149–V151 to
> V152–V154 when main took V149 for an unrelated feature; historical prose
> may cite the old numbers.

## 1. What Fabric Transport is

Fabric Transport is the worker-facing execution ownership layer for A:// jobs.

It answers one question:

> Which authenticated principal owns the right to execute this job right now?

It does **not** define the full A:// protocol, model routing, bot personality, Cowork UX, or connector semantics.

## 2. Core contract

A queued job becomes executable only when an authenticated principal successfully claims a lease.

```text
queued job
   -> eligibility check
   -> persistence-level atomic claim
   -> lease grant
   -> running
   -> heartbeat / renew
   -> complete | expire
```

The lease is the execution authority.

## 3. Principal registration and authentication

Workers are registered as `cowork_principals` with:

- canonical principal ID
- workspace
- declared capabilities
- token hash
- status

The registration endpoint returns the bearer token once. Only its SHA-256 hash is persisted.

A worker must authenticate using:

```http
Authorization: Bearer <token>
```

Possession of an `a://` address alone is not authentication.

## 4. HTTP surface

### Principal registration

```http
POST /api/v1/fabric/transport/principals
```

Requires normal user authentication.

Request:

```json
{
  "id": "a://workspace/acme/bot/research",
  "workspace": "acme",
  "capabilities": ["web.read", "files.project.read"]
}
```

Response includes a bearer token exactly once.

### Claim

```http
POST /api/v1/fabric/transport/claim
```

Requires worker bearer authentication.

Request fields:

- `job_id` optional targeted claim
- `wait_secs` optional long-poll duration, capped by the server
- `lease_ttl_secs` optional lease lifetime

Successful claim returns a `LeaseGrant` containing:

- `job_id`
- `run_id`
- `lease_id`
- `lease_generation`
- `lease_expires_at`
- `payload`
- `required_capabilities`
- `current_checkpoint_id`
- `initiator`
- `delegator`

### Job view

```http
GET /api/v1/fabric/transport/jobs/:job_id
```

Returns the canonical persisted job view.

### Heartbeat

```http
POST /api/v1/fabric/transport/jobs/:job_id/heartbeat
```

Required fields:

- `lease_id`
- `lease_generation`

`worker_time` is advisory only. The server clock is authoritative.

### Renew

```http
POST /api/v1/fabric/transport/jobs/:job_id/renew
```

Validates principal, lease ID, generation, job state, and server-side expiry before extending ownership.

### Complete

```http
POST /api/v1/fabric/transport/jobs/:job_id/complete
```

Completion is accepted only from the authenticated executor holding the current valid lease generation.

The endpoint is idempotent for already committed terminal work and returns the canonical existing result instead of creating duplicate effects.

Event delivery is idempotent too: `POST /api/v1/runs/:id/events` accepts an
optional client-supplied `event_id` (V157) and returns the canonical existing
event (`duplicated: true`) on retry instead of double-writing.

## 5. Approval binding

Protected execution can use lease-scoped approval bindings.

### Worker endpoints

```text
POST /fabric/transport/jobs/:job_id/approvals/request
POST /fabric/transport/jobs/:job_id/approvals/check
GET  /fabric/transport/approvals/:approval_id
```

### Human decision endpoints

```text
POST /fabric/transport/approvals/:approval_id/grant
POST /fabric/transport/approvals/:approval_id/deny
```

Approval scope includes:

- executor
- capability
- target
- run
- job
- lease ID
- lease generation

When a lease generation expires, its approval bindings are invalidated. A replacement worker under a new generation must re-obtain approval for the protected action.

Requests carry a server-clock `expires_at` (V156; `approval_ttl_secs` on the
request, default 300s). The sweeper and boot pass expire stale requests with
attributed `approval.expired` events; a grant arriving after expiry is rejected
(`A_APPROVAL_INVALID`) and an expired binding checks as `A_APPROVAL_REQUIRED`
(recovery policy: re-request, deny-by-default).

Whether a protected action needs a binding at all is decided by one risk-policy
evaluation (`allternit-cowork-runtime/src/risk_policy.rs`, V158
`cowork_approval_policy`): the same rule model as the cowork-engine
`ApprovalGate` — first match on action-type prefix + risk level, low-risk
auto-approve by default. Auto-decisions are attributed in the ledger
(`decided_by: risk-rule (…)`); `check` is read-only and writes no events.

## 6. Atomic claim rule

Claim atomicity lives in persistence, not only in `RunManager`.

The invariant is equivalent to a compare-and-swap:

```text
UPDATE queued job
SET owner, lease_id, lease_generation = lease_generation + 1, expires_at
WHERE job is still queued / eligible
```

Exactly one active generation may own a job at a time.

The in-memory runtime is a mirror used for execution/runtime coherence; it is not allowed to create a second source of ownership truth.

## 7. Lease generations

`lease_generation` is a monotonic execution-ownership counter.

Example:

```text
Worker A -> generation 1
A disappears
lease 1 expires
job requeued
Worker B -> generation 2
```

A late completion from Worker A must fail with stale-generation semantics.

## 8. Heartbeat and expiry

Workers heartbeat while executing. Heartbeat proves liveness, not progress.

Lease expiry is decided by the server clock.

On expiry, the runtime applies recovery policy:

```text
leased
  -> expired
  -> approval bindings invalidated
  -> retry count advanced
  -> queued OR dead_letter
```

The sweeper handles runtime expiry. Boot recovery also expires leases that died while the server was offline.

## 9. Restart recovery

Startup recovery performs three important actions:

1. expire leases whose server-issued deadline passed during downtime;
2. rehydrate persisted Cowork runs;
3. rehydrate persisted Cowork jobs, including queued and leased jobs.

The runtime must not silently lose durable work after restart.

## 10. Run-state honesty

A run must not become `running` simply because it was created or planned.

Current contract:

```text
created -> planned -> queued
claim succeeds -> running
```

`RUNNING` means execution ownership exists.

## 11. Checkpoint recovery model

Recovery does not promise resurrection of process memory or in-flight tool calls.

The portable unit is the last committed checkpoint.

After reassignment:

```text
new lease generation
   -> load canonical job/run state
   -> resolve last committed checkpoint
   -> replay/resume deterministic work from checkpoint
```

Workers should design jobs so externally committed effects and checkpoint boundaries are explicit.

## 12. Capability eligibility

The principal declares capabilities. The job declares `required_capabilities`.

A worker is not eligible if required capability strings are absent.

Capability checks are dispatcher/transport enforcement, not model judgment.

## 13. Attribution

Material run events support the attribution triple:

- `initiator`
- `delegator`
- `executor`

Example:

```text
initiator: a://workspace/acme/user/joe
delegator: a://workspace/acme/principal/al
executor:  a://workspace/acme/bot/research
```

Execution must be attributed to the principal that actually acted.

## 14. Error vocabulary

Stable wire errors are defined by `TransportErrorCode`.

Current examples include:

```text
A_AUTHENTICATION_FAILED
A_PRINCIPAL_NOT_FOUND
A_WORKSPACE_MISMATCH
A_CAPABILITY_MISSING
A_PERMISSION_DENIED
A_NO_ELIGIBLE_WORKER
A_JOB_ALREADY_LEASED
A_INVALID_LEASE
A_STALE_LEASE_GENERATION
A_LEASE_EXPIRED
A_RUN_CANCELLED
A_APPROVAL_REQUIRED
A_APPROVAL_INVALID
A_RESULT_ALREADY_COMMITTED
A_JOB_NOT_FOUND
A_STORE_ERROR
```

Callers should branch on stable wire codes, not human-readable error text.

## 15. Environment configuration

Current server defaults can be overridden with:

```text
ALLTERNIT_FABRIC_TRANSPORT_LEASE_SECS
ALLTERNIT_FABRIC_TRANSPORT_SWEEP_SECS
```

Treat these as operational tuning knobs, not semantic changes to the protocol.

## 15a. Managed launch (consumer desktop)

In the consumer-packaged desktop the operator never touches a terminal: the
signed desktop main process owns the worker lifecycle end-to-end (phase P1,
2026-09-14, `session/coworkp1-0914`).

- **Credential provision** — once per launch the desktop calls
  `POST /api/v1/fabric/transport/local/ensure-worker-principal` (loopback
  API, gated on the spawn-time `ALLTERNIT_DESKTOP_ACCESS_TOKEN` secret; the
  endpoint does not exist for callers without that secret). The route
  ensures the canonical `a://workspace/{ws}/principal/gizzi` principal,
  rotates its token, and returns the fresh token **exactly once** (only its
  hash is stored). The desktop keeps it in the macOS Keychain (Electron
  `safeStorage`; AES-GCM local envelope in dev).
- **Launch** — the bundled gizzi-code sidecar runs
  `gizzi-code fabric-worker` (the daemon entry ships inside the single-file
  Bun build; the desktop spawns it with `ALLTERNIT_GIZZI_TOKEN` and
  `ALLTERNIT_API_URL`). Readiness is the daemon's structured
  `worker.daemon_start` log line.
- **Resilience** — crash respawn with exponential backoff + jitter
  (mirroring the API backend manager); graceful quit is SIGTERM, after
  which the worker finishes its in-flight claim and exits 0 (abandoned
  leases requeue via the sweeper).
- **Status** — the desktop publishes one aggregate engine status
  (API / gizzi / fabric worker / office engine) to the app chrome; a dead
  worker is red there, never silently absent.

The manual flow (`ALLTERNIT_GIZZI_TOKEN=… bun worker-daemon-entry.ts`, or
`bun worker-entry.ts`) remains for operators and development.

## 15b. Reach — mobile approvals + routines (consumer P4)

- **iOS approvals inbox** — `FabricTransportClient` + `FabricApprovalsView`
  talk to `/api/v1/fabric/transport/approvals` (grant/deny, decided-by
  recorded server-side) and `/api/v1/runs` / `/runs/:id/events` for run
  status. Legacy Cowork task clients are untouched. Entry: Cowork
  launchpad radio-waves button.
- **Routines** — `POST/GET/DELETE /api/v1/cowork/routines` plus
  `POST /cowork/routines/:id/run` (run now). Schedule grammar is
  `*/N` minutes, `@hourly`, `@daily`. The 30s tick submits one canonical
  intent per due row (initiator = owning user, delegator = Al, target
  from the principal override or the same delegation-rule lookup Al uses
  for a chat turn; payload carries `agentic.task` = the routine message).
  Failed fires back off 5 minutes and do not skip.

## 15d. Cloud continuation substrate (E6 — NOT a finished feature)

Status: **open**. Placement and handoff APIs exist. A user cannot close the
laptop and have the job keep running. Do not describe this as shipped.

What is in the tree:

- `compute: { "policy": "cloud" }` → mandatory `compute.cloud`
- laptop `gizzi` has `compute.local` only, so it cannot claim those jobs
- `a://workspace/{ws}/principal/gizzi-cloud` (`kind=cloud`) is the
  intended always-on claimant
- `POST …/continue-in-cloud` and `…/continuation/handoff-all` retag
  in-flight jobs on **this** API

What now happens when a target is configured:

- Quit calls `handoff-all`, which POSTs each intent envelope plus a
  bounded copy of granted folders (skip symlinks, skip files > 1 MiB,
  cap 20 MiB) to `{url}/api/v1/fabric/transport/continuation/ingest`
  with `x-allternit-continuation-token`.
- Enabling the preference or creating a cloud routine without URL+token
  returns 409.
- Cloud-continuation routines are inserted on the remote API (local
  copy is `enabled=0` so the laptop does not tick them).

Always-on worker: a data-plane allternit-api that is **not** the laptop
sidecar (no `ALLTERNIT_DESKTOP_ACCESS_TOKEN`) starts `gizzi-code
fabric-worker --compute-mode cloud` itself. Desktop quit calls
`/api/v1/continuation/ensure` then relays ingest through cloud-api to a
provisioned or paired node — never the sleeping laptop. If only a laptop
node is registered, ensure returns 428.

Folders larger than 20 MiB / files larger than 1 MiB are skipped.

## 15c. Release engineering (consumer P5)

- **Updater feed** — electron-builder `build.publish`,
  `updateElectronApp({ repo })`, and `PLATFORM_MANIFEST.update.desktopFeedUrl`
  all point at `Allternit/desktop`. `allternit/desktop` does not exist (404).
  `scripts/release-preflight.mjs` fails the release if those three diverge.
- **Signing / notarization** — owner action. `build.mac.identity` stays
  `null` so unsigned local builds keep working; CI already forwards
  `APPLE_ID` / `APPLE_ID_PASSWORD` / `APPLE_TEAM_ID` into
  `scripts/notarize.cjs`, which hard-fails without them by design. Cut
  `desktop-v1.2.0` only after those secrets are provisioned.
- **Windows / Linux** — `release-desktop.yml` already builds NSIS +
  AppImage on tag `desktop-v*`. Smoke those artifacts from the tag; do
  not treat an untagged local build as a release.

## 16. Worker reference loop

A basic worker should behave like:

```text
authenticate
loop:
  long-poll claim
  if no work: continue
  load checkpoint if present
  execute deterministic step(s)
  heartbeat while active
  renew before expiry when needed
  request/check approval for protected action
  checkpoint after committed step boundaries
  complete with typed result
```

Never continue executing a job after losing the active lease generation.

## 17. Conformance test

The defining failure-recovery test is:

```text
Worker A claims generation 1
A starts work
A dies
heartbeats stop
lease generation 1 expires
bound approvals are invalidated
job requeues
Worker B claims generation 2
B resumes from last committed checkpoint
A submits late completion -> rejected
B completes -> exactly one canonical terminal result
ledger preserves attribution
```

Any future transport implementation must preserve these semantics even if its network mechanism differs.

## 18. Not Fabric Transport's responsibility

Do not put these concerns into Fabric Transport merely because work passes through it:

- Al personality or prompt
- bot product configuration
- model selection logic
- user memory policy
- connector secret storage
- Cowork presentation
- CommRails peer messaging semantics
- public `a://` resolution

Fabric Transport should remain narrow: authenticated execution ownership, lease lifecycle, protected action binding, and attributable completion.
