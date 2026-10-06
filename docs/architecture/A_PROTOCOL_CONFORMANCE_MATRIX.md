# A:// Conformance Test Matrix

**Status:** Reference / proof index. Every row names its concrete proof:
a test in `infrastructure/executor/cowork/cowork/allternit-cowork-runtime/tests/transport_conformance_tests.rs`,
an endpoint behavior, or live-demo evidence (run 2026-09-12 against a real
server, recorded in the PR #429/#436 bodies and the session attestations).

The rule being enforced (§16 of `A_PROTOCOL.md` / §16 of the index README):

> A component that only stores an `a://` identifier is not A:// conformant.

See the last section for the adjacent-plumbing inventory.

---

## 1. Worker conformance (§17 list)

| Claim | Proof | Status |
|---|---|---|
| Resolve/authenticate a principal identity | `authenticate_principal` exercised in every test via `auth()`; failure paths live (bad token → `A_AUTHENTICATION_FAILED`) | ✅ |
| Declare and satisfy capabilities | Eligibility rejection asserted: `test_adversarial_two_worker_failover` (no-shell principal → `A_CAPABILITY_MISSING`); concurrent winner test | ✅ |
| Claim work under canonical execution-ownership rules | Store-level CAS: `test_concurrent_claims_single_winner` (8 threads, exactly 1 winner); targeted claim conflict → `A_JOB_ALREADY_LEASED` | ✅ |
| Maintain or lose a lease correctly | Heartbeat + renew asserted (`test_adversarial_two_worker_failover`); renew of stale gen → `A_STALE_LEASE_GENERATION`; expiry by server clock in both failover tests | ✅ |
| Emit attributable events | Every event-row assertion checks the initiator/delegator/executor triple (`event_rows` helper); live ledger dumps in PR #429 | ✅ |
| Obey approval/policy boundaries | `test_full_824_sequence_with_approval` (request→grant→gate→invalidate→re-obtain); `test_approval_expiry_and_late_grant_rejected`; `test_risk_policy_single_evaluation_path` (auto-approve/auto-deny/requires) | ✅ |
| Reject stale execution authority | Ghost completion → `A_STALE_LEASE_GENERATION` **even on terminal jobs**; ghost heartbeat rejected; stale approval → `A_APPROVAL_INVALID` (live: "approval … is bound to lease generation 1, current is 2") | ✅ |
| Commit a result exactly once | Duplicate completion → `already_committed` + identical `result_id`; `test_event_post_idempotency` extends the same guarantee to client events (3 deliveries → 2 rows) | ✅ |

## 2. Control-surface conformance (§17 list)

| Claim | Proof | Status |
|---|---|---|
| Create or observe intents | `test_intent_submission_idempotent` (idempotent on intent_id, canonical run, attribution, version/cycle rejection) + `POST/GET /fabric/transport/intents` live | ✅ |
| Observe canonical run state | `GET /runs`, `GET /runs/:id/jobs`, `GET /fabric/transport/jobs/:id` read the store; state-machine reference in `COWORK_RUNTIME_STATE_MACHINES.md` | ✅ (API) |
| Display approvals | `GET /fabric/transport/approvals` inbox (workspace/status filter) + grant/deny; `GET /cowork/approvals` user-filtered (#422); `/fabric-transport` control view renders the inbox with decided_by reasons (auto-approvals show WHY, not silent) | ✅ |
| Display attributed events | `GET /runs/:id/events` + SSE stream return attribution columns (V154) | ✅ (API; UI partial) |
| Display completion/failure | CompleteOutcome + job view expose state/result; run advanced to terminal; rendered in `/fabric-transport` run detail | ✅ |

## 3. §8.24 proof-of-protocol sequence

| Step | Proof |
|---|---|
| 1–4 intent → run → job → queue | Run/job routes; `start_run` stops at `queued` (§8.2 honesty, #429) |
| 5–7 eligibility, auth, claim gen 1 | Adversarial test + live demo (worker A, `lease_89b1…`) |
| 8–12 protected action → approval → grant → resume | `test_full_824_sequence_with_approval` + live demo (`appr_3709…` granted, payment step executed) |
| 13–17 worker killed → expiry → approvals invalidated → requeue | Sweeper CAS asserted (state, retry_count, lease cleared); `approval.invalidated` with executor attribution; live server-log line captured |
| 18–20 replacement claims gen 2, checkpoint replay, protected action re-evaluated | Grant carried `current_checkpoint_id`; replay from step 1 (live) / step boundary (test); re-check → `A_APPROVAL_INVALID` |
| 21–23 re-approval, completion, exactly-once result | Binding 2 under gen 2 granted; `result.created` exactly once; duplicate → canonical `result_id` |
| 24 canonical state observable | Job view + run `completed` in store |
| 25 ledger triple | `initiator=user/joe`, `delegator=principal/al`, `executor=A→B` on every material event (test assertions + live dump) |
| Killed worker cannot complete later | Ghost completion `A_STALE_LEASE_GENERATION` (asserted post-completion, live 409) |

## 4. The four locks (§8.0)

| Lock | Proof |
|---|---|
| 1. Canonical store = Rust runtime SQLite as wired in cmd/allternit-api | All protocol fns take `rusqlite::Connection` over the API's DB (`DbHandle::connect` in handlers; same file the sweeper/rehydration uses); conformance tests run the identical store code on temp DBs |
| 2. Claims atomic at the persistence layer | `UPDATE cowork_jobs … WHERE id=? AND state='queued' RETURNING lease_generation` in an immediate transaction; concurrency test; no in-process lock anywhere in the claim path |
| 3. Recovery = replay from last committed checkpoint; server clock authoritative | `current_checkpoint_id` in LeaseGrant; sweeper/boot expiry judged on stored `lease_expires_at` vs `Utc::now()` — `worker_time` is accepted but never read for expiry; downtime test |
| 4. v0.1 discovery = long-poll claim | `wait_secs` loop in the claim handler; documented as the only correctness-relevant discovery; no push machinery in the protocol path |

## 5. Adjacent plumbing, NOT conformance

These store `a://`-shaped data or touch the vocabulary without participating
in the lifecycle. Per §16 they must not be cited as A:// conformance:

| Component | Why it is not conformance | Reference |
|---|---|---|
| `cowork_executions` dead-end inserts (`cowork_routes.rs` run-agent/team-execute) | Rows are inserted and never read back; no run/job lifecycle, no lease | Contract Appendix A |
| gizzi-code fake remote modes | Simulated remote execution without principal auth, leases, or attributed events | Out of scope per slice |
| cloud-api cowork models (`cmd/allternit-cloud-api`) | A second, separate run/job store (Postgres/sqlx) that does not participate in the canonical SQLite spine | Lock 1; Appendix A |
| `agents` / `cowork_personas` tables | Store model/provider/prompt and persona data — product records, not execution principals; unlinked to `cowork_principals` | `BOT_AUTHORING_SPEC.md` §0 |
| Webhook subscription/trigger routes | Emit/ingest events into their own pipeline; they do not create canonical Intent/Run work | `BOT_AUTHORING_SPEC.md` §9 |
| Artifact `a://` address display in Cowork/chat | An address string rendered in UI; no lifecycle participation | artphase2 ledger entry |
| `cowork_memory_entries`, schedules DB | Scoped data/trigger substrate; durable but not execution-ownership state | — |
| Factory ledger mirroring of runtime events | Transport substrate carrying already-attributed events; carries semantics, does not define them | `A_PROTOCOL.md` §15 |

## 5b. Task-DAG items A-T1–A-T5 (closed)

| Item | Proof |
|---|---|
| A-T1 handoff chains + ack | `test_handoff_ack_completes_linked_job` (linked job terminated with typed result, executor attribution, idempotent replay); cyclic handoff chains rejected at the route; `POST /runs/:id/handoffs/:hid/ack` |
| A-T2 per-principal memory grants | `test_memory_principal_grants` (owner/grantee/stranger visibility, default-deny write check); `/cowork/memory` with `principal`/`grants` |
| A-T3 Al orchestration loop v0.1 | `test_al_orchestration_loop` (rule-based delegation, attributed delegation.created/rejected/completed, parent-run mirroring, no double delegation); boot tick in `main.rs` |
| A-T4 Gizzi claim loop | `cmd/gizzi-code/src/runtime/fabric-transport/worker.ts` — long-poll claim, `Sandbox.wrap` posture, heartbeat, checkpoints, typed Result; typechecked; live demo evidence |
| A-T5 connector broker v0.1 | `test_connector_broker_sessions` (secret-free sessions, principal-bound, approval-gated critical capabilities, honest simulated invoke, attributed connector.invoked); live demo evidence |

## 5c. Product-depth items (P-T1 closed; P-T2…P-T6 tracked in [MASTER_TRACKING](../projects/allternit-cloud/MASTER_TRACKING.md))

| Item | Proof |
|---|---|
| P-T1 multi-store consolidation boundary | `store_boundary_tests.rs` (6 tests: projection preserves active lease, leased transition refused + transport completes, unleased projection applies, queue marking sets caps/attribution, run roundtrip + delegator, event ownership); Rails cowork REST routes write only via canonical `sqlite_store` projection helpers (lease-clobbering raw upserts removed; `transition_job` reports `projection_applied`); cloud-api `store_boundary.rs` + `cowork_models.rs` header mark the Postgres tables product-local projection (not removable this pass — documented); gizzi `store-boundary.ts` gates all 11 Cowork write paths (`cowork-store-boundary.test.ts`, 11 tests); `A_STORE_BOUNDARY.md` per-store status |
| P-T2 non-local compute placement | `compute_placement_tests.rs` (5 tests: policy→capability mapping, vm job refuses local worker / wins on vm worker, local policy refuses vm-only worker, `submit_intent` enqueues claimable job with caps + idempotent replay, Al-targeted parent creates no bypass job, `set_principal_capabilities` update + missing error); `PUT /fabric/transport/principals/:id/capabilities`; gizzi worker `GIZZI_COMPUTE_MODE=vm` (Lima executor); live claim evidence in session notes |
| P-T3 worker daemon packaging | `worker-daemon-entry.ts` (structured JSON logs, exponential claim backoff + jitter, SIGTERM/SIGINT graceful stop — in-flight job finishes, abandoned leases requeue via the sweeper, no protocol change); `packaging/launchd/com.allternit.gizzi-worker.plist`; `packaging/systemd/gizzi-worker.service`; `docs/FABRIC_WORKER_DAEMON.md` end-to-end install |
| P-T4 connector breadth | `connector_breadth_tests.rs` (3 tests: seeded capabilities, GitHub read auto-approves / write approval-gated + payload validation without network, files confined write/read on disk + `..`/absolute escapes refused + attribution on every invocation + honest simulation when root unset); secrets read system-side from env at invoke time only |
| P-T5 Al persona runtime v0.1 | `al_persona_routes.rs` unit tests (fallback extraction, model JSON parsing, target canonicalization, envelope posture: delegator=al, chain [user, al], never executor); `POST /cowork/al/chat` + `GET /cowork/al/sessions/:id` (V168); model extraction reuses `run_completion` (shared with `/v1/responses`) — no new LLM path; `resolve_delegation_rule` shared with the orchestrator |
| P-T6 Cowork protocol rendering | New control endpoints: `GET /principals`, delegation-rules CRUD, `GET /connector-sessions`, attribution triple on `GET /runs/:id/events`; `FabricTransportView.tsx`: principals/bots management (roles, capability chips, one-time token provisioning), delegation rules editor, connector sessions view, run-detail timeline interleaving attributed events with approval states (grant/deny inline); frontend typecheck clean |

## 6. Honest gaps in the conformance story

- **Non-local compute** — ✅ closed by P-T2: placement resolves through
  required capabilities (`compute.vm` etc.); conformance proofs above.
- **Al/Gizzi as live workers** — Gizzi's claim loop ships and is daemon
  packaged (P-T3; launchd/systemd), but continuous operation is an operator
  install, not a CI claim. Al's deterministic loop (A-T3) plus the persona
  runtime (P-T5) both delegate through the same canonical path.
- **UI conformance depth** — ✅ substantially closed by P-T6: principals
  management, delegation rules editor, connector sessions, and the
  attributed + approvals-interleaved run timeline all render canonical
  state. DAG visualization remains future work.
- **Delegation chain enforcement coverage** — chains are validated on intent
  submission and job creation; enforcement on other write paths (handoffs)
  is not universal.

Proven in this pass: `test_default_principal_seeding_and_token_provisioning`
(Gap 1), agent creation minting the principal in-transaction (Gap 2, build +
live evidence), `test_delegation_chain_cycle_and_depth` (Gap 3),
`test_intent_submission_idempotent` (Gap 4), `/fabric-transport` view +
approvals inbox (Gap 5, typechecked).
