# A:// Developer Guide

This guide is for engineers adding workers, bots, Cowork features, schedulers, connectors, or execution paths that participate in A://.

Read first:

- `docs/architecture/A_PROTOCOL.md`
- `docs/architecture/FABRIC_TRANSPORT.md`
- `docs/architecture/COWORK_A_PROTOCOL_ARCHITECTURE.md`

## 1. Golden rule

Do not add another execution island.

Every new work-producing feature should converge onto the same identity, run/job, execution-ownership, approval, attribution, and result semantics.

## 2. Choose the actor correctly

Before coding, identify the principal that actually performs the action.

Examples:

```text
Al                    orchestrator/user-facing principal
Gizzi                 code/terminal worker
Research Bot          specialized worker
Bookkeeper Bot        sensitive business worker
human user            initiator/approver
system scheduler      trigger/system actor where applicable
```

Do not attribute a worker's action to Al merely because Al delegated it.

## 3. Canonical principal IDs

Use workspace-scoped IDs for persisted executable principals.

```text
a://workspace/{workspace}/principal/al
a://workspace/{workspace}/principal/gizzi
a://workspace/{workspace}/bot/{bot_id}
```

Do not make globally ambiguous persisted IDs such as only `a://bot/research` unless they are temporary aliases resolved inside a known workspace.

## 4. Register a worker

Fabric Transport currently exposes principal registration through:

```http
POST /api/v1/fabric/transport/principals
```

Example:

```json
{
  "id": "a://workspace/acme/bot/research",
  "workspace": "acme",
  "capabilities": [
    "web.read",
    "files.project.read",
    "artifact.create"
  ]
}
```

Store the returned token securely. The API persists only its hash.

Never treat the address string itself as proof of identity.

## 5. Define capabilities before execution

A worker should declare the smallest capability set it actually needs.

A job should declare mandatory `required_capabilities`.

Recommended naming pattern:

```text
resource.verb
namespace.resource.verb
```

Examples:

```text
web.read
shell.exec
files.project.read
files.project.write
connector.github.read
connector.github.write
gui.control
artifact.create
```

Do not encode authorization decisions inside the capability string. Capability means technical ability; policy decides permission.

## 6. Create work honestly

The product should distinguish these states:

```text
intent exists
run exists
job queued
job leased
job running
approval waiting
job completed/failed
```

A model saying "I assigned this" does not mean a job was dispatched or leased.

UI state must come from canonical runtime state.

## 7. Worker claim loop

The v0.1 worker contract is long-poll based.

Pseudo-code:

```text
while worker_active:
    lease = POST /fabric/transport/claim
    if no eligible work:
        continue

    restore checkpoint if present
    start heartbeat loop

    for step in job:
        verify lease still active
        execute step
        checkpoint committed boundary

    POST complete
```

Factory peer messaging or push notification can wake a worker faster, but correctness must not depend on receiving the push.

## 8. Never bypass lease ownership

A worker may act on a job only while it holds the active lease generation.

Every mutating worker endpoint should validate:

- authenticated principal;
- job ownership;
- lease ID;
- lease generation;
- server-side expiration;
- run cancellation state;
- relevant approval/policy state.

Do not create "fast paths" that mutate external state without these checks.

## 9. Heartbeat and renew

Heartbeats prove the executor is still alive.

Renew only when needed and before the server-issued expiration.

Never trust worker wall-clock time for expiry decisions. BYO/remote machines may have incorrect clocks.

## 10. Recovery-safe job design

Assume the worker can die at any point.

Design jobs as deterministic/replayable steps with explicit committed checkpoints.

Bad design:

```text
one giant opaque process with critical state only in RAM
```

Better design:

```text
step 1 -> commit/checkpoint
step 2 -> external effect -> record effect/checkpoint
step 3 -> commit/checkpoint
```

On lease reassignment, the replacement worker resumes from the last committed checkpoint, not from dead process memory.

## 11. External side effects

At-least-once delivery must not become duplicate external effects.

For payment, email send, booking, write, deployment, or other non-idempotent actions:

- use provider idempotency keys when available;
- persist the external action reference before advancing;
- checkpoint after committed side effects;
- bind approval to the current execution context;
- make completion idempotent;
- give client-emitted events a stable `event_id` (the events API dedupes on it, V157).

## 12. Protected actions and approvals

For a protected action, use Fabric Transport approval binding rather than a generic boolean.

The approval should be tied to:

```text
executor
capability
target
run
job
lease generation
```

If execution ownership changes, the old approval must not silently transfer to a different generation.

Current route family:

```text
/fabric/transport/jobs/:job_id/approvals/request
/fabric/transport/jobs/:job_id/approvals/check
/fabric/transport/approvals/:approval_id
/fabric/transport/approvals/:approval_id/grant
/fabric/transport/approvals/:approval_id/deny
```

Worker operations use worker bearer auth. Human decisions use normal user auth.

Requests take an optional `approval_ttl_secs` (default 300s). Expired requests
are swept to `expired` with attributed `approval.expired` events; late grants
fail `A_APPROVAL_INVALID`; `check` on an expired binding returns
`A_APPROVAL_REQUIRED` (recovery policy: re-request). Whether a binding is
needed at all is a risk-policy decision (`risk_policy.rs`, V158), not a
per-call choice — low-risk capabilities auto-approve with an attributed
ledger event; auto-deny fails `A_PERMISSION_DENIED`.

## 13. Attribution requirements

Every material execution event should preserve:

```text
initiator
optional delegator
executor
```

Example:

```text
initiator = user
 delegator = Al
 executor  = Gizzi
```

When Gizzi delegates a subtask:

```text
initiator = original user
 delegator = Gizzi
 executor  = specialized worker
```

Do not flatten the chain to the top-level orchestrator.

## 14. Results

Completion should return/store typed result data rather than treating stdout as the contract.

A result can reference:

- artifacts;
- structured data;
- external action IDs;
- logs/metrics;
- summary;
- checkpoints.

Large content should usually be referenced as an artifact rather than embedded in events.

## 15. Error handling

Use stable `A_*` wire codes from `TransportErrorCode` for program logic.

Do not match on error prose.

Important concurrency/security outcomes include:

```text
A_AUTHENTICATION_FAILED
A_WORKSPACE_MISMATCH
A_CAPABILITY_MISSING
A_PERMISSION_DENIED
A_NO_ELIGIBLE_WORKER
A_JOB_ALREADY_LEASED
A_INVALID_LEASE
A_STALE_LEASE_GENERATION
A_LEASE_EXPIRED
A_APPROVAL_REQUIRED
A_APPROVAL_INVALID
A_RUN_CANCELLED
```

## 16. Adding a new bot type

A new bot type is primarily configuration + capabilities + worker implementation, not a new orchestration stack.

Checklist:

1. define durable principal identity;
2. define roles;
3. define capability set;
4. define memory scope;
5. define connector scope;
6. define approval policy;
7. define model/router policy;
8. define compute policy;
9. define triggers;
10. make jobs enter canonical run/job state;
11. use Fabric Transport execution ownership;
12. emit attribution/result events;
13. expose state through Cowork.

## 17. Adding a new trigger

A trigger should create/resolve Intent/Run work; it should not own a parallel execution engine.

Examples:

```text
cron
webhook
Slack event
email event
GitHub event
file event
manual Cowork request
API request
```

All should eventually converge on:

```text
trigger -> intent -> run -> jobs -> lease -> execute -> result
```

## 18. Adding a connector

Connector integration should preserve least privilege and attribution.

Preferred direction:

```text
principal requests connector capability
 -> policy evaluates principal/run/action
 -> connector broker creates scoped session
 -> principal acts
 -> event records executor + connector action
```

Do not hand long-lived raw service tokens directly to arbitrary worker processes when a brokered session can be used.

## 19. Adding Cowork UI

Cowork views should consume canonical state, not infer it from assistant prose.

Use runtime/event data for:

- progress;
- worker assignment;
- approval state;
- files/artifacts;
- completion/failure;
- recovery;
- audit history.

Conversation text may explain state but must not be the authoritative source for it.

## 20. Testing requirements

Every new execution path should test normal and adversarial behavior.

Minimum tests:

- correct principal can claim eligible job;
- wrong workspace cannot claim;
- missing capability cannot claim;
- concurrent claims have one winner;
- stale lease generation is rejected;
- expired lease cannot complete;
- restart rehydrates durable work;
- protected action rejects missing/stale approval;
- completion is idempotent;
- attribution names the real executor.

For new worker classes, include a kill/reassign test whenever recovery is expected.

## 21. Conformance checklist for a worker

A worker is A://-conformant for its declared scope only if all are true:

- [ ] canonical principal exists;
- [ ] authentication is enforced;
- [ ] workspace is enforced;
- [ ] capabilities are declared;
- [ ] jobs declare required capabilities;
- [ ] claim ownership is atomic;
- [ ] active lease is enforced;
- [ ] server clock controls expiry;
- [ ] stale generations fail closed;
- [ ] recovery/checkpoint behavior is defined;
- [ ] approvals bind to execution context where needed;
- [ ] completion is idempotent;
- [ ] attribution is correct;
- [ ] Cowork can observe canonical state.

## 22. Code entry points

Worker HTTP API:

```text
cmd/allternit-api/src/rails/fabric_transport_routes.rs
```

Transport types/error vocabulary:

```text
infrastructure/executor/cowork/cowork/allternit-cowork-runtime/src/transport.rs
```

Canonical SQLite execution store:

```text
infrastructure/executor/cowork/cowork/allternit-cowork-runtime/src/sqlite_store.rs
```

Runtime state machine / manager:

```text
infrastructure/executor/cowork/cowork/allternit-cowork-runtime/src/run.rs
```

Schema:

```text
cmd/allternit-api/migrations/V152__cowork_principals.sql
cmd/allternit-api/migrations/V153__cowork_job_lease_columns.sql
cmd/allternit-api/migrations/V154__cowork_event_attribution.sql
cmd/allternit-api/migrations/V155__cowork_approval_bindings.sql
cmd/allternit-api/migrations/V156__cowork_approval_expiry.sql
cmd/allternit-api/migrations/V157__cowork_event_idempotency.sql
cmd/allternit-api/migrations/V158__cowork_approval_policy.sql
```

(Numbering note: the cowork set was renumbered V149–V151 → V152–V154 when an
unrelated migration took V149; older prose may cite the old numbers.)

Startup/rehydration/sweeper wiring:

```text
cmd/allternit-api/src/main.rs
```

## 23. Documentation rule

If you change any of these semantics, update the docs in the same PR:

- principal identity/address format;
- capability model;
- lease lifecycle;
- approval binding;
- state machine meaning;
- attribution fields;
- error vocabulary;
- route paths;
- canonical store/source of truth;
- Al/Gizzi/Bot responsibilities.

Protocol documentation is part of the implementation contract, not optional commentary.
