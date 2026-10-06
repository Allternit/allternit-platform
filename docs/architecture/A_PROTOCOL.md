# A:// Coordination Protocol

**Status:** Internal architecture contract. Implementation-backed where marked; not a public interoperability claim.

## 1. Purpose

A:// is Allternit's coordination protocol for humans, agents, bots, tools, models, services, runtimes, and compute.

A:// defines the common grammar for identity, addressing, intent, delegation, capabilities, policy, execution ownership, approvals, events, attribution, results, and transport-independent coordination.

A:// does **not** mean one omnipotent agent owns every action.

> A:// is not the intelligence. A:// is the protocol by which intelligence coordinates.

The user-facing Coworker is **A://**, pronounced **Al**. In technical specifications and code, use lowercase `a://` for the protocol namespace and reserve **Al** for the persistent user-facing principal/persona.

## 2. Architectural invariants

1. **Identity != model != session.** A principal may outlive any model process or conversation.
2. **Orchestration is a role, not a security root.** Al is the default orchestrator, not a mandatory ancestor of every action.
3. **Every actor is independently attributable.** Initiator, delegator, and executor are distinct when applicable.
4. **Capabilities and permissions are separate.** A worker may be technically capable of an action and still be denied by policy.
5. **Possession of an `a://` address is not authentication.** Principal identity must be proven by the execution transport.
6. **A component is not A:// conformant merely because it stores an `a://` identifier.** Conformance requires participation in the relevant lifecycle.
7. **Cowork is the control room, not the identity.** Cowork observes and governs runs, principals, approvals, artifacts, and events.
8. **Fabric Transport is the worker/lease execution transport, not the A:// protocol itself.**

## 3. Product identities

### 3.1 Al

Canonical user-facing identity: **A://**

Pronunciation: **Al**

Technical role:

```text
principal: a://workspace/{workspace}/principal/al
roles:
  - orchestrator
  - user-interface
```

Al receives work, creates or delegates intent, tracks runs, requests approval, and reports outcomes. Al does not automatically own every connector credential, every bot memory, every execution authority, or every audit event.

### 3.2 Gizzi

Gizzi is the primary technical worker below Al: Gizzi Code / terminal / repository / build / debugging / developer-environment work.

```text
principal: a://workspace/{workspace}/principal/gizzi
roles:
  - worker
  - code
  - terminal
```

Gizzi is not the Coworker identity.

### 3.3 User-created bots

Bots are first-class persistent worker principals with independent identity, permissions, credentials, memory scope, triggers, compute policy, model policy, budgets, and audit history.

Example:

```text
a://workspace/acme/bot/research
a://workspace/acme/bot/bookkeeper
```

## 4. Four-layer model

A:// rests on four separable layers:

```text
Participants / Principals
    identity, credentials, memory, policy, attribution

Roles
    orchestrator, worker, reviewer, observer, human, system

Transport
    Factory peer messaging and other transports carry semantic messages

Execution
    Run/DAG runtime + Fabric Transport lease ownership
```

No layer should silently absorb the responsibility of another.

## 5. Addressing

Canonical persisted addresses are workspace scoped.

```text
a://workspace/{workspace}/principal/{principal}
a://workspace/{workspace}/bot/{bot}
a://workspace/{workspace}/run/{run}
a://workspace/{workspace}/artifact/{artifact}
a://workspace/{workspace}/connector/{connector}
a://workspace/{workspace}/compute/{target}
```

Short aliases such as `a://bot/research` may exist after workspace resolution, but persisted identities must remain tenant safe.

The `a://` namespace is an **internal logical address space**. It does not promise an operating-system URI handler, DNS-like resolution, or public scheme interoperability.

## 6. Intent

A:// work should normalize into an Intent envelope before execution.

Conceptual shape:

```yaml
version: a/0.1
intent_id: intent_...
workspace: a://workspace/acme
initiator:
  principal: a://workspace/acme/user/joe
target:
  principal: a://workspace/acme/bot/research
action:
  type: research
  description: Compare three vendors.
permissions:
  requested:
    - web.read
    - files.project.read
compute:
  policy: auto
model:
  policy: router:auto
approval:
  policy: risk-gated
return:
  channel: cowork
```

An Intent describes requested work. It does not imply that execution has begun.

## 7. Runs, jobs, and execution ownership

The canonical execution path is:

```text
Intent
  -> Run
  -> DAG / Jobs
  -> queued Job
  -> Fabric Transport claim
  -> active lease generation
  -> execution
  -> heartbeat / renew
  -> completion | failure | expiry | reassignment
  -> Result
  -> attributed events / ledger
```

A UI must not report work as running merely because a model said it delegated the task. Running work requires a valid runtime state and execution ownership.

## 8. Fabric Transport and Lease Protocol

Fabric Transport is the current worker-facing implementation of durable execution ownership.

Current API surface is under:

```text
/api/v1/fabric/transport/*
```

The transport implements the worker lifecycle around:

- principal registration/authentication
- long-poll claim
- capability eligibility
- persistence-level atomic claim
- lease ID and lease generation
- server-authoritative expiry
- heartbeat
- renew
- expiry/requeue
- stale-generation rejection
- attributed completion

### 8.1 Canonical correctness rules

- Claim exclusivity must be enforced in the persistence layer, not only with an in-process lock.
- `lease_generation` increases when execution ownership changes.
- Lease expiry is evaluated using the server clock.
- A stale worker cannot complete work using an expired generation.
- Recovery resumes from the last committed checkpoint; in-memory process state is not assumed portable.
- v0.1 worker discovery uses long-poll claim. Factory push may optimize wake-up but is not required for correctness.

## 9. Principal authentication

Executable principals are workspace-scoped identities. Workers authenticate before claiming or mutating leased jobs.

The current local proof slice stores hashed bearer credentials in `cowork_principals`. Raw tokens are not the principal identity; the authenticated token resolves to one.

Future transports may use local peer identity, short-lived worker credentials, mTLS, or enrolled-machine identity, but all mechanisms must bind the runtime process to the principal that acts.

## 10. Capabilities and policy

Capabilities are flat, namespaced strings in v0.1.

Examples:

```text
web.read
files.project.read
files.project.write
shell.exec
git.read
git.write
gui.observe
gui.control
browser.navigate
connector.github.read
connector.github.write
artifact.create
```

Workers declare capabilities. Jobs declare required capabilities. The transport may only grant execution to an eligible principal.

Capability means **can**. Policy means **may**.

## 11. Delegation and causation

Delegation preserves provenance.

```text
User -> Al -> Research Bot -> Browser Bot
```

Material events should preserve initiator, delegator, and executor.

Delegation must not implicitly transfer all credentials or permissions from the delegator to the executor.

Delegation chains must reject cycles. A configurable maximum depth should be enforced when multi-hop delegation is enabled.

## 12. Approvals

Approval is scoped to the actual action and execution context. At minimum, protected approval should be bindable to executor, capability, target, run, job, and lease generation.

When execution ownership changes, stale approval must not silently authorize a new executor or a new lease generation unless an explicit reusable policy says otherwise.

## 13. Attribution

A:// audit semantics distinguish who asked, who delegated, and who acted.

```yaml
initiator: a://workspace/acme/user/joe
delegator: a://workspace/acme/principal/al
executor: a://workspace/acme/bot/research
```

Al must not appear as executor merely because Al coordinated the work.

The event/ledger layer should allow an auditor to answer:

- who requested this?
- who delegated it?
- who executed it?
- under what policy?
- with which capabilities/connectors?
- on which compute?
- what result was committed?

## 14. Cowork

Cowork is the control room for A:// activity.

It should expose canonical runtime truth for conversation, plans, runs and jobs, principals and bots, delegations, approvals, timelines, artifacts, connectors, compute, and event/audit history.

Al may be conversationally present in Cowork, but Cowork must not collapse into a transcript that hides execution state.

## 15. Transport independence

A:// defines coordination semantics. Factory peer messaging, HTTP, WebSocket, local IPC, Slack adapters, email adapters, or future transports can carry those semantics.

Transport does not redefine principal identity, run state, attribution, or approval meaning.

## 16. Current implementation status

### Implemented / backed by code

- workspace-scoped `cowork_principals` (incl. `roles`, V162)
- default Al/Gizzi principal minting per workspace, idempotent, credential-provisioned once
- bearer token hash authentication for Fabric Transport proof slice
- principal capability storage
- job lease ID / generation / expiry / claimed-at fields
- required job capabilities
- initiator / delegator / executor attribution fields
- long-poll worker claim API
- persistence-level atomic claim path
- heartbeat / renew / complete API
- lease sweeper / expiry path
- stale lease-generation protection
- typed result storage on jobs
- Fabric Transport terminology and `/api/v1/fabric/transport/*` routes
- approval↔lease bindings scoped to (executor, capability, target, run, job, lease_generation), invalidated on lease expiry
- approval request timeout with sweeper-driven expiry and late-grant rejection
- single risk-policy evaluation path shared with the cowork-engine rule model
- boot-time downtime lease recovery and full run/job rehydration
- client-supplied event idempotency keys on the run-events API
- bot product/agent record ↔ execution principal linkage (agents.principal_id, V163)
- delegation causation chains with cycle rejection and configurable depth (default 4, V164)
- canonical IntentEnvelope submission, idempotent on intent_id (cowork_intents, V164)
- handoff delegation chains + handoff acknowledgment (completion) path (V165)
- per-principal memory grants with default-deny cross-principal access (V165)
- deterministic Al orchestration loop (delegation rules → child intent → monitor → record; V166)
- connector broker v0.1: lease+policy validated sessions, system-side invocation, no raw secrets to workers (V167)

### Partial / still needs hardening

- approval-to-lease binding coverage across all protected tools/connectors
  (the binding mechanism is implemented; tool/connector coverage is not universal)
- complete capability vocabulary across all workers

### Implemented in the product-depth pass (P-T1, 2026-09-13)

- **canonical multi-store consolidation boundary** — the Rails cowork REST
  surface persists RunManager state through canonical, lease-safe projection
  helpers in `allternit-cowork-runtime` (raw lease-clobbering upserts
  removed); cloud-api Postgres tables are an explicitly-marked product-local
  projection (`store_boundary.rs`); gizzi-code Drizzle cowork writes are
  gated by a store-boundary module when paired with a canonical API. See
  `A_STORE_BOUNDARY.md` for per-store status and honest deferrals.

### Implemented in the product-depth pass (P-T2…P-T6, session/aproduct-0913)

- **non-local compute placement (§8.8)** — intent `compute` policy resolves
  to mandatory job capabilities (`vm`/`local`/`byo`/`cloud`; `auto` stays
  capability-neutral); `submit_intent` enqueues the claimable job (Al-
  targeted parents excepted so delegation can't be bypassed); workers
  declare placement caps via
  `PUT /fabric/transport/principals/:id/capabilities`; gizzi runs VM mode
  through the Lima executor (`GIZZI_COMPUTE_MODE=vm`). Identity/attribution
  unchanged.
- **worker daemon packaging** — `worker-daemon-entry.ts` (structured logs,
  exponential backoff, graceful stop with sweeper requeue), launchd plist,
  systemd unit, end-to-end install doc.
- **connector breadth** — GitHub (`connector.github.read/write`,
  approval-gated write) and files/local (`connector.files.read/write`,
  root confinement, approval-gated write) through the existing broker;
  secrets system-side only.
- **Al persona runtime v0.1** — `POST /cowork/al/chat` +
  `GET /cowork/al/sessions/:id`; model-assisted extraction reuses the model
  router/gateway; zero-capability posture.
- **Cowork protocol rendering** — principals management, delegation rules
  editor, connector sessions view, attributed + approvals-interleaved run
  timeline in `/fabric-transport`.

### Implemented in the deliverables phase (consumer desktop P3, 2026-09-14)

- **deliverable pipeline** — agent-filled markdown → office-engine render
  (report/sheet/deck templates) → persisted finished documents attached to
  the canonical run (registry = attributed `deliverable.created` events),
  previewed/exported over `/cowork/runs/:id/deliverables`; the agentic
  worker's `deliverable` tool attaches them under its principal bearer.
- **worker auth fix** — `atok_` principal tokens pass the auth middleware to
  the fabric routes' own authentication (latent P1 production bug).
- **run detail as document timeline** — FabricTransportView renders
  Deliverables cards above the attributed event/approval timeline.

### Implemented in the chat-drives-A:// phase (consumer desktop P2, 2026-09-14)

- **chat drives A://** — `POST /cowork/al/chat/stream` (SSE): the same Al
  delegation as `/cowork/al/chat`, streamed (`delegation` → `run_state` →
  `approval`/`approval_decision` → `result` → `finish`, narration as
  `content_block_delta` text). Al's payload carries the agentic job kind
  (`payload.agentic.task`). The SPA routes Cowork chat through it behind
  `NEXT_PUBLIC_ALLTERNIT_COWORK_CHAT_VIA_AL`; the legacy `/api/agent-chat`
  relay stays the default until parity is proven.
- **agentic job kind** — the gizzi worker runs a bounded model-agent loop
  (`src/runtime/fabric-transport/agentic.ts`) through the EXISTING model
  router (`/v1/chat/completions` with the operator key; no new LLM path),
  with `fs_read`/`fs_write`/`bash` tools, per-step checkpointing, and
  step/token budget caps from the job payload.
- **trusted-folder confinement** — worker file tools default-deny outside
  `ALLTERNIT_WORKER_TRUSTED_FOLDERS` (grants the desktop passes at spawn
  from `/cowork-preferences`); symlink escapes refused.
- **approval cards** — SSE approval frames surface as grant/deny cards in
  the app chrome (`ApprovalToastHost`) against the existing fabric
  approvals endpoints; decided-by is recorded server-side.

### Implemented in the managed-runtime phase (consumer desktop P1, 2026-09-14)

- **managed worker lifecycle** — `POST /fabric/transport/local/ensure-worker-principal`
  (desktop access-token gated, token returned once, hash stored) + the
  desktop's Keychain-backed credential store and `gizzi-code fabric-worker`
  managed spawn (backoff respawn, SIGTERM graceful quit). See
  `FABRIC_TRANSPORT.md` §15a and `GIZZI_WORKER_SPEC.md` §7.
- **engine status surface** — one aggregate green/yellow/red indicator
  (API / gizzi / fabric worker / office engine) in the app chrome, fed by
  the desktop main process over the preload bridge.

### Implemented in the consumer reach + release pass (P4/P5, 2026-09-14)

- **iOS fabric approvals** — Swift sources exist (`FabricTransportClient` +
  `FabricApprovalsView`). Not simulator-verified. Not a shipped iOS
  approvals inbox.
- **Routines** — `/api/v1/cowork/routines` tick fires a canonical
  attributed intent per due schedule; Cowork Fabric Transport view
  creates/runs/deletes them.
- **Updater feed lock** — `Allternit/desktop` is the single publish +
  auto-update target; preflight refuses a mismatch. Signed/notarized
  `desktop-v1.2.0` remains an owner action (Apple secrets).

### Cloud continuation (E6)

Desktop quit retags in-flight jobs, packs granted folders, calls
`/api/v1/continuation/ensure`, and relays ingest through cloud-api to a
**non-local** node (provisioned, then paired). That node's allternit-api
auto-starts `gizzi-cloud` when it is not a desktop sidecar. If the account
only has a laptop node, ensure is 428 — the laptop is not an always-on
host.

Live behavioral evidence for all six items (vm-job claim grant/refusal,
boundary projection refusal, brokered files read/write + approval gate +
path-confinement refusal, Al chat fallback + end-to-end delegation, daemon
claim → execute → SIGTERM stop, control-surface reads) was captured against
a fresh-migrated dev database on 2026-09-13: `tmp/aproduct-evidence/LIVE_EVIDENCE.md`
(rerun via `tmp/aproduct-evidence/run.sh`). The same pass fixed pre-existing
main breakage: duplicate migration versions V142–V144 (renumbered
V169–V171) and the embed_migrations no-rebuild gotcha — see CHANGELOG
[Unreleased] → Fixed.

### Implemented in the cowork-DAG integration pass (session/adocs2-0913)

- **cowork sessions participate in the A:// lifecycle (§7/§16)** — session
  row creation submits the session's canonical intent (target: the
  workspace Gizzi principal; return channel `cowork`), creating the
  session's run; the linkage lives in the session row's `metadata`
  (`a_intent_id` / `a_run_id` / `a_native_session_id`) and the agent-chat
  bridge resolves (or lazily backfills) it per turn. Gizzi tool executions
  during a turn are recorded as lightweight, attributed job rows on the
  session run (`job_created` / `job.completed` / `job.failed`); turn
  completion writes a typed `turn.completed` Result and an A-T2 memory
  entry owned by the user's principal with an explicit grant to Al;
  session completion finalizes the run (`run.completed`). Conversational
  tool jobs are not Fabric-Transport-leased work (§8.2), so they are
  recorded directly without a claim and carry the `cowork.chat` capability
  on the session run's placeholder job so no transport worker claims it.
- **seam fixes** — intent-created runs are owner-stamped at insert (visible
  to the V142/V169-scoped run surface; job-postable like any run); the Al
  orchestrator matches the documented short alias `principal/al` exactly as
  the canonical long form (`targets_al`); store-direct orchestrator child
  runs are mirrored into the RunManager within one tick.

Delegation-rule convention (orchestrator + P-T5 persona runtime):
`cowork_delegation_rules.workspace` is the **bare workspace id** (the
`a://workspace/` prefix stripped), matching the store's workspace column —
an `a://`-form value silently matches no intent.

### Planned / not implied by v0.1

- public URI scheme registration
- public interoperability standard
- every Allternit subsystem being A:// conformant
- every bot or connector using Fabric Transport automatically

## 17. Conformance

A worker may claim A:// worker conformance only when it can:

1. resolve/authenticate a principal identity;
2. declare and satisfy capabilities;
3. claim work under the canonical execution-ownership rules;
4. maintain or lose a lease correctly;
5. emit attributable events;
6. obey approval/policy boundaries;
7. reject stale execution authority;
8. commit a result exactly once.

A surface may claim A:// control conformance only when it observes canonical runtime state instead of deriving authority from model text.

## 18. Naming rules

Use these names consistently:

- **A://** — branded user-facing mark; pronounced **Al** when referring to the Coworker persona.
- **Al** — the persistent user-facing principal/persona.
- **`a://`** — protocol namespace in code/spec prose.
- **Fabric Transport** — worker-facing execution transport and lease mechanism.
- **Factory peer messaging** — peer/message transport layer.
- **Run/DAG runtime** — durable execution state machine.
- **Cowork** — human control room.
- **Gizzi** — technical/terminal/code worker.
- **Bot** — independently permissioned persistent worker identity.

Avoid saying "A:// decided" in implementation docs when the actor is actually Al, a bot, a policy engine, or the runtime.

## 19. Source-of-truth rule

When prose and code disagree, do not silently preserve both descriptions. Update this document or explicitly mark the implementation as non-conformant.

Every future Cowork, Bot, Fabric, or agent feature should answer:

> Does this participate in A://, or are we creating another execution island?
