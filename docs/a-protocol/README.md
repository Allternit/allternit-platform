# A:// Architecture and Developer Documentation

**Status:** Canonical internal architecture guide  
**Contract:** [`../A_COORDINATION_CONTRACT_V0_1.md`](../A_COORDINATION_CONTRACT_V0_1.md)  
**Implementation layer:** Fabric Transport and Lease Protocol  
**Audience:** Allternit engineers, agent sessions, reviewers, platform integrators

## The one-sentence model

**A:// is not the intelligence. A:// is the coordination contract by which intelligence coordinates. Al is its default user-facing Coworker.**

This directory exists to prevent the A:// architecture from drifting back into several incompatible systems with similar names. The normative protocol language remains in `docs/learnings/A_COORDINATION_CONTRACT_V0_1.md`; these documents explain how that contract maps onto the repository and how developers extend it without creating another execution island.

## Naming is architectural

Use these terms consistently:

| Term | Meaning |
| --- | --- |
| `a://` | Technical coordination namespace / contract notation. Use lowercase in code and specifications. |
| **Al** | Persistent user-facing Coworker principal; default orchestrator and user-interface role. Al is not the protocol and is not a mandatory security root. |
| **A://** | Product/wordmark spelling and high-level protocol name. Do not write sentences such as “A:// decided…” in technical docs. A principal or policy engine made the decision. |
| **Principal** | First-class actor with identity, workspace, capabilities, policy, credentials and audit attribution. |
| **Role** | Behavior/authority assigned to a principal. `orchestrator` is a role, not an identity. |
| **Gizzi** | Technical/code/terminal worker principal. Gizzi is not the Coworker. |
| **Bot** | Durable user-created worker principal with its own identity and authority boundary. |
| **Cowork** | Control room/workbench for conversations, runs, approvals, timelines, artifacts and workers. Cowork is not an agent identity. |
| **Factory peer messaging** | Peer messaging / communication transport. It does not own run truth or lease exclusivity. |
| **Fabric Transport and Lease Protocol** | Durable worker-delivery layer implementing claim, lease, heartbeat, expiry, retry/reassignment, approval binding and completion. This supersedes the older “A:// dispatcher” wording. |
| **Run/DAG runtime** | Canonical durable execution/state-machine substrate. |
| **Fabric** | Compute/execution-placement layer. Placement must not change actor identity or attribution. |

## System model

```text
                               USER / TEAM
                                   |
                                   v
                         Al (a Principal)
                    default orchestrator + UI
                                   |
                     speaks the A:// contract
                                   |
        +--------------------------+--------------------------+
        |                          |                          |
        v                          v                          v
      Gizzi                  User-created Bots          Other Principals
 code/terminal worker       domain workers              humans/services
        |                          |                          |
        +--------------------------+--------------------------+
                                   |
                         Factory peer messaging
                         peer communication bus
                                   |
                                   v
                         Intent / Run / Jobs
                                   |
                                   v
                    Fabric Transport + Leases
                  auth -> claim -> lease -> execute
                 heartbeat -> approval -> completion
                                   |
                                   v
                         Run / DAG state machine
                                   |
                                   v
                                Fabric
                       local / VM / remote / cloud
```

The important property is that **the diagram is not a mandatory tree**. Al is the default orchestrator, but principals may communicate or delegate peer-to-peer when policy permits. Audit records identify the principal that actually performed an action, not merely the principal that routed it.

## Four layers

### 1. Participants

A principal is an independently attributable actor. Current Fabric Transport principal records include a canonical ID, workspace, capabilities and status; bearer-token authentication binds a worker process to that identity.

Canonical workspace-scoped examples:

```text
a://workspace/acme/principal/al
a://workspace/acme/principal/gizzi
a://workspace/acme/bot/research
a://workspace/acme/bot/bookkeeper
```

Short aliases such as `a://bot/research` are presentation conveniences only after a workspace is already resolved.

### 2. Roles and policy

Identity and role are separate. A principal may be an orchestrator, worker, reviewer, observer, system actor or human. Capabilities describe what a worker can technically do; policy determines whether it may do that action in the current context.

Never use model output as an authorization decision. Planning may be probabilistic; eligibility, approval, lease ownership and policy checks are deterministic system boundaries.

### 3. Transport and execution ownership

Factory peer messaging moves messages between peers. Fabric Transport owns durable execution handoff. They are intentionally different concerns.

For v0.1, workers discover work by long-polling the Fabric Transport claim endpoint. Factory push may later reduce latency, but correctness does not depend on push delivery.

A job is not running because a chat message says it was delegated. A job is running only when an authenticated eligible principal holds the current valid lease generation and the runtime state reflects that fact.

### 4. Durable run state

The Rust Cowork runtime is the execution spine used by the v0.1 Fabric Transport implementation. Store-level compare-and-swap establishes claim exclusivity. Server time controls expiry. A replacement worker resumes by replaying from the last committed checkpoint under a new lease generation rather than inheriting dead process memory.

## Security model in one paragraph

Every material action must preserve `initiator`, `delegator` and `executor` as separate concepts. Delegation does not transfer all authority. Principal credentials prove identity; they do not grant arbitrary connector secrets. Approvals are scoped to the executor/action/run/job/current lease generation and expire with the execution context. A stale worker may not complete after ownership has moved to a new generation.

See [`security-and-principals.md`](security-and-principals.md).

## Current implementation map

The main implementation lives in:

```text
infrastructure/executor/cowork/cowork/allternit-cowork-runtime/
  src/transport.rs                    protocol types + stable A_* errors
  src/sqlite_store.rs                 canonical persistence/CAS/lease operations
  src/run.rs                          run/job state machine + recovery/sweeper integration
  src/risk_policy.rs                  approval risk-policy evaluation
  tests/transport_conformance_tests.rs

cmd/allternit-api/
  src/rails/fabric_transport_routes.rs worker-facing HTTP transport API
  src/rails/routes_cowork.rs           run events / Cowork-facing runtime routes
  src/main.rs                          runtime boot/recovery integration
  migrations/V155__cowork_approval_bindings.sql
  migrations/V156__cowork_approval_expiry.sql
  migrations/V157__cowork_event_idempotency.sql
  migrations/V158__cowork_approval_policy.sql
```

Migration numbers before V155 may contain the principal/lease schema introduced in the first proof slice; use the repository migration history rather than assuming a number from old planning documents.

## What is implemented vs contracted

Do not infer feature completion from a type or namespace existing. A component that only stores an `a://` identifier is **not** A:// conformant.

As of the current `main` implementation, the Fabric Transport vertical slice includes authenticated principal registration, long-poll claim, persistence-level lease ownership, server-clock heartbeat/renew/expiry, completion validation, stale-generation rejection, lease-bound approvals, approval expiry, event idempotency, boot recovery, risk-policy evaluation and transport conformance tests.

The broader A:// contract is larger. The following remain separate work unless/until the conformance matrix says otherwise: a universal IntentEnvelope entry path, connector credential brokerage, non-local compute placement, full bot lifecycle/product surfaces, Factory push wakeups, complete Cowork visualization of transport state, and external/public protocol interoperability.

See [`conformance.md`](conformance.md) before claiming parity or completion.

## Developer entry points

- **Understand the architecture:** this file.
- **Read normative semantics:** [`../A_COORDINATION_CONTRACT_V0_1.md`](../A_COORDINATION_CONTRACT_V0_1.md).
- **Modify worker transport:** [`fabric-transport.md`](fabric-transport.md).
- **Add principals/capabilities/approvals:** [`security-and-principals.md`](security-and-principals.md).
- **Add a bot/worker/transport integration:** [`development-guide.md`](development-guide.md).
- **Check what is actually shipped:** [`conformance.md`](conformance.md).

## Non-negotiable invariants

1. Identity is not a model, session, process or lease.
2. Al is a principal and default orchestrator, not the mandatory root of every trust domain.
3. Gizzi is a worker principal, not the Coworker identity.
4. Every material event preserves the actor that actually executed it.
5. Claim exclusivity lives in persistent storage, never only in an in-process lock.
6. Server time is authoritative for lease expiry.
7. A new lease generation invalidates stale execution authority and lease-bound approvals.
8. At-least-once message delivery must not become duplicate side effects.
9. Cowork reads canonical run state; it must not invent lifecycle state by parsing prose.
10. New intelligence features must either speak the A:// contract or document why they form a separate trust/execution domain.

## Review rule

When reviewing a new feature, ask:

> **Does this participate in A://, or is this creating another identity, policy, run-state or execution island?**

If it creates another island, that decision requires an explicit architecture rationale rather than happening accidentally.