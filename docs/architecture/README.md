# Allternit Agent/Cowork Architecture Index

This directory contains the canonical architecture documentation for the A:// coordination model and the Cowork/Bot execution stack.

## Start here

### `A_PROTOCOL.md`

**Normative architecture contract.**

Defines:

- what A:// is and is not;
- Al versus the `a://` protocol namespace;
- principal identity;
- addressing;
- intent/run concepts;
- delegation;
- attribution;
- conformance;
- terminology rules;
- implemented/partial/planned boundaries.

If another document disagrees with this one about A:// semantics, reconcile the conflict explicitly rather than silently keeping two models.

### `FABRIC_TRANSPORT.md`

**Normative execution-ownership contract for the current worker transport.**

Defines:

- worker authentication;
- capability eligibility;
- atomic claim;
- leases and generations;
- heartbeat/renew/expiry;
- restart recovery;
- approval-to-lease binding;
- attributed completion;
- stable error vocabulary;
- conformance/failure recovery test.

Fabric Transport is an implementation layer under A://. It is not synonymous with A:// itself.

### `COWORK_A_PROTOCOL_ARCHITECTURE.md`

**Product/runtime architecture.**

Defines the relationship between:

- Al / A:// user-facing identity;
- Gizzi;
- user-created bots;
- Cowork;
- reach surfaces;
- memory/connector boundaries;
- model and compute independence.

## Second-pass references (2026-09-13)

Added after a grounding pass against the implementation (V152–V158, transport
code, conformance tests). These are **reference** documents: they cite code
and mark anything not yet built as **Specified / Planned** — they do not
change the normative contracts above.

### `A_PROTOCOL_SCHEMA.md`

**Reference (wire shapes).** Exact JSON for Principal, LeaseGrant,
ApprovalBinding, Result, every standardized §8.22 event, the risk policy, the
`A_*` error table, the full `/api/v1/fabric/transport/*` HTTP surface with
per-endpoint auth, and the IntentEnvelope (marked Specified / Planned — the
open scorecard item).

### `COWORK_RUNTIME_STATE_MACHINES.md`

**Reference (state machines).** Every RunState/JobState/ApprovalBinding
transition, what causes it (endpoint, sweeper CAS, boot recovery), its
persistence effects, and how Cowork/UI must project canonical state.

### `BOT_AUTHORING_SPEC.md`

**Spec with honest status.** The two-record bot contract (`agents` +
`cowork_principals`), per-area Implemented/Partial/Planned status for roles,
capabilities, model/router, compute, approvals, memory, connectors, triggers,
budget — plus the authoring checklist that works today.

### `AL_IMPLEMENTATION_SPEC.md`

**Spec / build-out plan.** What Al owns vs never owns, orchestration policy,
delegation rules, the §12 never-inherit list — sharply separating what Al does
in code today (mostly: nothing yet) from what this spec defines.

### `GIZZI_WORKER_SPEC.md`

**Spec with honest status.** Gizzi's role, implemented sandbox posture
(bubblewrap on Linux, vfkit VM mode on macOS), the specified
fabric-transport capability set, and the not-yet-wired claim path.

### `A_PROTOCOL_CONFORMANCE_MATRIX.md`

**Proof index.** Maps every worker/control conformance claim, every §8.24
step, and the four locks to its concrete proof (test name, endpoint behavior,
live-demo evidence) — plus the adjacent-plumbing inventory of things that
store `a://`-shaped data without participating in the lifecycle.

## Developer guide

See:

```text
docs/development/A_PROTOCOL_DEVELOPER_GUIDE.md
```

Use it when adding a worker, bot type, trigger, connector, protected action, or Cowork view.

## Canonical code entry points

### Worker-facing HTTP transport

```text
cmd/allternit-api/src/rails/fabric_transport_routes.rs
```

### Transport types and wire errors

```text
infrastructure/executor/cowork/cowork/allternit-cowork-runtime/src/transport.rs
```

### Canonical persisted execution operations

```text
infrastructure/executor/cowork/cowork/allternit-cowork-runtime/src/sqlite_store.rs
```

### Run/job state machine and runtime manager

```text
infrastructure/executor/cowork/cowork/allternit-cowork-runtime/src/run.rs
```

### Startup, rehydration, and sweeper wiring

```text
cmd/allternit-api/src/main.rs
```

### Schema migrations

```text
cmd/allternit-api/migrations/V152__cowork_principals.sql
cmd/allternit-api/migrations/V153__cowork_job_lease_columns.sql
cmd/allternit-api/migrations/V154__cowork_event_attribution.sql
cmd/allternit-api/migrations/V155__cowork_approval_bindings.sql
cmd/allternit-api/migrations/V156__cowork_approval_expiry.sql
cmd/allternit-api/migrations/V157__cowork_event_idempotency.sql
cmd/allternit-api/migrations/V158__cowork_approval_policy.sql
```

## Runtime truth hierarchy

When implementation surfaces disagree, use this hierarchy:

```text
canonical persisted run/job state
 -> current valid Fabric Transport lease
 -> attributed run events / ledger
 -> Cowork UI projection
 -> conversational/model narration
```

A model response is never authoritative proof that work was dispatched, executed, approved, or completed.

## Terminology lock

Use:

- **A://** — branded mark / user-facing Coworker name; pronounced Al.
- **Al** — persistent user-facing principal/persona.
- **`a://`** — internal protocol namespace.
- **Fabric Transport** — worker/lease transport.
- **Factory peer messaging** — peer/message transport.
- **Cowork** — control room.
- **Gizzi** — code/terminal technical worker.
- **Bot** — independently permissioned persistent worker principal.

Do not reintroduce older `A:// dispatcher` naming for the HTTP/runtime worker transport. Historical commits may use it, but current terminology is **Fabric Transport**.

## Documentation change policy

Any PR changing one of the following should update the relevant documentation in the same change:

- principal model or address format;
- worker authentication;
- capability vocabulary/semantics;
- run/job lifecycle;
- lease rules;
- approval scope;
- attribution;
- error codes;
- route paths;
- source-of-truth storage;
- Al/Gizzi/Bot responsibility boundaries;
- Cowork's control-room contract.

## Conformance principle

The critical rule is:

> A component that only stores an `a://` identifier is not A:// conformant.

A:// conformance is behavioral. Identity, execution ownership, policy, attribution, recovery, and result semantics must actually be implemented for the declared role.
