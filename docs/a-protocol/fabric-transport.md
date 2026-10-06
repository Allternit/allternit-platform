# Fabric Transport and Lease Protocol — Developer Guide

**Normative contract:** `docs/learnings/A_COORDINATION_CONTRACT_V0_1.md`, §8  
**Implementation:** `allternit-cowork-runtime` + `allternit-api` worker routes  
**Terminology:** This is the implementation layer previously referred to as the “A:// dispatcher.” Use **Fabric Transport** in new code and docs.

## Why this layer exists

Fabric Transport turns durable queued work into execution ownership. It is responsible for deciding whether a worker is eligible, atomically granting temporary ownership, proving liveness, expiring dead ownership, retrying/reassigning safely, binding approvals to the execution context, and committing terminal results exactly once.

It does **not** perform model reasoning and it is not Al. Al or any other authorized principal can originate/delegate work; Fabric Transport is the deterministic system boundary that turns queued work into an executable lease.

## Canonical execution path

```text
validated intent / run
        |
        v
queued job
        |
        v
worker long-polls claim endpoint
        |
        v
principal authentication
        |
        v
eligibility check
        |
        v
PERSISTENCE-LEVEL CAS CLAIM
        |
        v
lease generation N
        |
        +---- heartbeat / renew ----+
        |                            |
        +---- approval if needed ----+
        |                            |
        v                            |
complete / fail                      |
        |                            |
        v                            |
terminal result + events             |
                                     |
if worker dies ----------------------+--> lease expires --> requeue/dead-letter
                                                   |
                                                   v
                                           new generation N+1
```

## Source map

| Concern | Canonical implementation |
| --- | --- |
| Wire types / A_* errors | `infrastructure/executor/cowork/cowork/allternit-cowork-runtime/src/transport.rs` |
| Persistence, claim CAS, lease operations | `.../allternit-cowork-runtime/src/sqlite_store.rs` |
| Run/job state + sweeper/recovery integration | `.../allternit-cowork-runtime/src/run.rs` |
| Risk policy | `.../allternit-cowork-runtime/src/risk_policy.rs` |
| Worker HTTP API | `cmd/allternit-api/src/rails/fabric_transport_routes.rs` |
| Run event API | `cmd/allternit-api/src/rails/routes_cowork.rs` |
| Boot recovery | `cmd/allternit-api/src/main.rs` |
| Conformance tests | `.../allternit-cowork-runtime/tests/transport_conformance_tests.rs` |

## Worker-facing API

The Fabric Transport router is mounted under the v1 API and currently defines the following worker-facing operations:

```text
POST /fabric/transport/principals
POST /fabric/transport/claim
GET  /fabric/transport/jobs/:job_id
POST /fabric/transport/jobs/:job_id/heartbeat
POST /fabric/transport/jobs/:job_id/renew
POST /fabric/transport/jobs/:job_id/complete

POST /fabric/transport/jobs/:job_id/approvals/request
POST /fabric/transport/jobs/:job_id/approvals/check
GET  /fabric/transport/approvals/:approval_id
POST /fabric/transport/approvals/:approval_id/grant
POST /fabric/transport/approvals/:approval_id/deny
```

Confirm the router mount prefix in `allternit-api` before constructing absolute URLs; the list above is the router-local path contract.

### Principal registration

Registration is user-authenticated. A worker bearer token is returned once; only its hash is persisted.

A worker token is authentication material, not an all-purpose connector credential.

### Claim

`POST /fabric/transport/claim` accepts an optional `job_id`, `wait_secs`, and lease TTL. Workers may long-poll. The current implementation caps the request wait window; clients should retry after a clean no-work response/error rather than assuming push delivery.

The claim path must preserve this split:

1. authenticate the principal;
2. evaluate eligibility;
3. let persistent storage atomically establish ownership;
4. return the resulting `LeaseGrant`.

Never replace step 3 with an in-process mutex. Multiple processes/services may eventually contend for the same job.

## LeaseGrant

`LeaseGrant` is the worker's proof of temporary execution ownership. Important fields include:

```text
job_id
run_id
lease_id
lease_generation
lease_expires_at
payload
required_capabilities
current_checkpoint_id
initiator
delegator
```

`lease_generation` is monotonic for ownership changes. Generation N and generation N+1 are different execution authorities even if they refer to the same job.

`lease_expires_at` is server-authoritative. Never use the worker's wall clock to decide lease validity.

## Heartbeats and renewal

Heartbeat proves that the current worker is alive. It does not prove useful progress and should not be used as a substitute for run events/checkpoints.

Renewal extends a valid current lease only when identity, lease ID, generation, run/job state and policy still permit execution.

A renewal must never resurrect an expired generation.

Configuration currently uses the Fabric Transport environment naming (for example `ALLTERNIT_FABRIC_TRANSPORT_LEASE_SECS`). Do not reintroduce the old `ALLTERNIT_DISPATCH_*` prefix.

## Expiry and reassignment

When a worker stops heartbeating, the server-side expiry path decides that ownership has ended. Depending on retry policy, the job is requeued or dead-lettered.

The critical safety rule is:

> Once a new lease generation exists, a worker holding an earlier generation no longer has authority to commit the result.

A late completion from the old worker must be rejected as stale even if the worker successfully performed external computation.

## Recovery semantics

Recovery is **not** continuation of another process's in-memory state.

The contract is:

```text
new worker + new lease generation
        |
        v
load last committed checkpoint
        |
        v
replay/resume deterministic step sequence
        |
        v
continue execution
```

Any worker integration that cannot make its side effects idempotent or resumable must explicitly declare the non-replayable boundary and require stronger approval/checkpoint semantics around it.

## Completion

Completion validates the authenticated executor, lease ID, lease generation and ownership before committing terminal state.

The completion operation is idempotent: retries return the canonical already-committed result rather than causing duplicate side effects.

Do not use “the process exited successfully” as the protocol definition of completion. A protocol result is a typed terminal result attached to the canonical job/run state.

## Approval binding

Protected actions are not approved globally for a principal. Approval is bound to an execution context that includes the executor, capability, target, run/job and lease generation.

A lease expiry/reassignment invalidates approvals associated with the stale generation. The replacement execution must re-evaluate the protected action and obtain a new approval when required.

Approval requests also expire using server time. A late decision must not resurrect an expired approval.

## Risk policy

`risk_policy.rs` is the Rust-side policy evaluation path for Fabric Transport protected actions. It mirrors the ApprovalGate rule model rather than inventing a second incompatible policy language.

When adding a protected capability:

1. add/confirm a stable capability string;
2. define risk classification/policy behavior;
3. ensure approval scope includes the exact target;
4. test grant, deny, expiry and stale-generation behavior;
5. make sure the ledger records who requested, who decided and who executed.

## Event idempotency

At-least-once delivery is expected. Event clients may supply an idempotency key (`event_id` / persisted `client_event_id` per run). Retrying the same event must resolve to the existing canonical event rather than write a duplicate.

Do not invent independent idempotency behavior in each caller. Use the canonical event-store operation.

## Stable errors

Fabric Transport exposes stable `A_*` error vocabulary through `TransportErrorCode`, including authentication, principal/workspace mismatch, capability failure, no eligible worker, lease conflicts, stale generation, expired lease, approval requirements and already-committed results.

Client code should branch on stable machine-readable error codes/statuses, not parse human error messages.

## State ownership

For the v0.1 proof slice, the Rust runtime SQLite store is canonical for run/job/event/lease truth. Other surfaces may maintain caches/mirrors, but they must not independently decide execution ownership.

If/when the canonical store moves to another persistence technology, claim atomicity and all conformance tests must move with it. The architectural requirement is the single authoritative persistence boundary, not SQLite specifically forever.

## Conformance test that matters most

A valid implementation must survive this sequence:

```text
Worker A authenticates
Worker A claims job -> generation 1
Worker A starts work
Worker A dies
server detects expiry
job is requeued
lease-bound approvals from generation 1 are invalidated
Worker B authenticates
Worker B claims -> generation 2
Worker B resumes from committed checkpoint
Worker A returns late -> completion rejected
Worker B completes -> result committed once
ledger preserves initiator/delegator/executor
```

This is the semantic heart of the transport layer. A green happy-path claim test alone is insufficient.

## Things Fabric Transport must not become

- a second orchestration persona;
- a model/planner;
- an authorization-by-prompt system;
- a second Factory peer-messaging implementation;
- an in-memory queue whose database is merely observational;
- a place where connector secrets are handed directly to workers;
- a UI-derived state machine.

## Changing this layer

Any change to claim ownership, lease generation, expiry, approval binding, idempotency or completion semantics should update all of:

1. `docs/learnings/A_COORDINATION_CONTRACT_V0_1.md` if the semantic contract changes;
2. runtime types/store logic;
3. worker-facing HTTP routes if the wire contract changes;
4. transport conformance tests;
5. [`conformance.md`](conformance.md) if implementation status changes.

Do not silently change protocol semantics only in code.