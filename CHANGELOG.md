# Changelog

All notable changes to the Allternit platform, API, SDKs, and `gizzi-code` CLI are tracked in this file. For a higher-level, release-by-release summary, see [`docs/public/release-notes.md`](./docs/public/release-notes.md).

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- Computer access tiers (read/click/full per app, category ceilings, 12 h session grants via `resolve_access`/`request_access`), four action classes enforced in every permission path (`confirm_now` asks even in bypass mode), a reviewer that can only tighten, and `folder_access` with a `_to_delete/` redirect. Docs: `tools/computer-access`, `tools/action-review`.
- Cloud continuation **substrate** (E6): `compute.cloud` placement. session/e6-0914 adds laptop→always-on ingest (envelope + bounded folder copy), 409 without URL+token, remote routine create. Still not a closed laptop path unless an operator already runs `gizzi-cloud` on that always-on API.

- A:// P-T1 store-consolidation boundary (session/aproduct-0913): canonical lease-safe projection helpers in `allternit-cowork-runtime::sqlite_store` (`persist_run_record`, `persist_job_record`, `mark_job_queued_for_transport`, `transition_job_record`, `set_run_delegator`, `set_current_run_job`, `set_run_checkpoint`, `update_run_state_record`, `record_run_event_projection`) + 6 boundary tests; `cmd/allternit-cloud-api/src/db/store_boundary.rs` marking the Postgres cowork tables a product-local projection; gizzi-code `src/runtime/cowork/store-boundary.ts` gating all Cowork store writes when paired with a canonical API (+11 tests); `docs/architecture/A_STORE_BOUNDARY.md` per-store status.
- A:// P-T2 non-local compute placement (session/aproduct-0913): intent `compute` policy (`vm`/`local`/`byo`/`cloud`, `auto` neutral) resolves to mandatory job capabilities via `sqlite_store::compute_requirements`; `submit_intent` enqueues the claimable canonical job (Al-targeted parents excepted); `PUT /api/v1/fabric/transport/principals/:id/capabilities`; gizzi worker `GIZZI_COMPUTE_MODE=vm` executes steps in the Lima VM; seeded Gizzi principal gains `compute.local`. +5 placement tests.
- A:// P-T3 worker daemon packaging (session/aproduct-0913): `cmd/gizzi-code/src/runtime/fabric-transport/worker-daemon-entry.ts` (structured JSON logs, exponential claim backoff with jitter, SIGTERM/SIGINT graceful stop — in-flight job finishes, abandoned leases requeue via the sweeper; claim protocol unchanged); `packaging/launchd/com.allternit.gizzi-worker.plist`; `packaging/systemd/gizzi-worker.service`; `cmd/gizzi-code/docs/FABRIC_WORKER_DAEMON.md` end-to-end install.
- A:// P-T4 connector breadth (session/aproduct-0913): GitHub connector (`connector.github.read`/`connector.github.write` via `ALLTERNIT_BROKER_GITHUB_TOKEN`; write approval-gated, read auto-approves) and files/local connector (`connector.files.read`/`connector.files.write` via `ALLTERNIT_BROKER_FILES_ROOT`; path confinement under the root; write approval-gated) through the existing broker — secrets read system-side at invoke time only; `base64` dep added to `allternit-cowork-runtime`. +3 tests.
- A:// P-T5 Al persona runtime v0.1 (session/aproduct-0913): `POST /api/v1/cowork/al/chat` + `GET /api/v1/cowork/al/sessions/:id` (`al_persona_routes.rs`; transcript table `cowork_al_messages`, V172); model-assisted intent extraction reuses the model router/gateway (`run_completion`, shared with `/v1/responses` — no new LLM path) with deterministic fallback; target resolution via `resolve_delegation_rule` (shared with the orchestrator loop); Al acts under `a://principal/al`, zero-capability posture.
- A:// P-T6 Cowork protocol rendering (session/aproduct-0913): `GET /fabric/transport/principals`, delegation-rules CRUD (`GET`/`PUT`/`DELETE /fabric/transport/delegation-rules`), `GET /fabric/transport/connector-sessions`, and the attribution triple (initiator/delegator/executor) on `GET /runs/:id/events`; `FabricTransportView.tsx` gains principals/bots management (roles, capability chips, one-time token provisioning), a delegation rules editor, a connector sessions view, and a run-detail timeline interleaving attributed events with approval states (inline grant/deny).
- `cmd/allternit-api/migrations/V172__al_persona_messages.sql` — Al persona transcript table.
- `cmd/allternit-api/migrations/V173__connector_breadth_seeds.sql` — registers the P-T4 GitHub and files/local connectors with the broker (env-var-name references only, same invariant as V167).
- Fabric session artifacts in bot chat (2026-09-13, branch `ao/platform-console-agents`): artifact routes in `fabric_routes.rs` (+273), `InlineArtifactRenderer` + settled-bubble artifact rendering across the bot-chat transcript fold (adapter, types, tests), desktop `auth-manager` session persistence, and office add-in manifest updates.
- Bot-chat streaming UX (2026-09-13, branch `ao/bot-streaming-ux`): transient ActivityLine tool-activity fold, StreamMetricsTracker TTFT + windowed tok/s, SystemLine compaction divider backed by a new `context_compacted` SSE frame (gizzi agent-compat + Rust agent_chat_bridge), finish frames carrying `usage {inputTokens, outputTokens}` from `message.updated` events, opt-in first-response preamble for user/DM turns only, and a routine-pending status label (`nextRoutineLabel`) in the chat chrome. See commit `a035a354d`.
- Cowork production-quality pass (2026-09-13, branch `ao/platform-console-agents`): security scoping on all cowork runs routes (allternit-api V142 `user_id` ownership + cloud-api tenant scoping incl. new recover/handoffs routes with V016 `handoffs` table), approvals persistence (V143/V144 + POST decision endpoint), real `parse_prd` behind the gizzi gateway, runtime-offline UX across the cowork surface, and 60+ new tests. See commits `0fd578c8b`, `807c0c3af`, `2ce28cb3e`, `ff38427e3`.
- `cmd/allternit-api/migrations/V71__eval_runs.sql` — `eval_datasets` and `eval_runs` tables for agent evaluation.
- `cmd/allternit-api/src/eval_routes.rs` — `/api/v1/admin/eval/datasets`, `/api/v1/admin/eval/runs`, and `/runs/:id/scores` CRUD endpoints.
- `cmd/allternit-api/migrations/V72__fallback_credits.sql` — fallback credit policy, ledger, and `llm_usage_events.fallback_from_event_id`.
- `cmd/allternit-api/src/fallback_credit_routes.rs` — admin endpoints to configure fallback credit policy, reconcile credits, and apply credits to org spend.
- `docs/public/release-notes.md` and `docs/public/guides/migration-guides.md` — release notes index and migration guide landing page.

### Changed

- The Rails cowork REST surface (`cmd/allternit-api/src/rails/routes_cowork.rs`) now persists all run/job/event state through the canonical `sqlite_store` projection helpers — the raw upserts that could overwrite `lease_owner` / clobber leased job state are removed; `POST …/jobs/:job_id/state` reports `projection_applied: false` when the boundary refuses a leased-job write (A:// P-T1).

### Fixed

- **Fresh-DB migrations were broken on main**: V142/V143/V144 each existed twice (the 2026-09-13 cowork production pass collided with the earlier runtime-settings/agent-hardening/deployment migrations), so refinery panicked (`UNIQUE constraint failed: refinery_schema_history.version`) on every fresh database, and the cowork variants were silently skipped on existing databases (their `user_id`/approvals columns never applied). The cowork trio is now applied once — on this branch they land as `V168__cowork_runs_ownership.sql`, `V169__cowork_approvals.sql`, `V170__cowork_approvals_decision.sql` (origin/main renumbered them independently; the merge keeps origin/main's numbering and this branch's P-T4/P-T5 additions take V172/V173). Fresh DBs now migrate V1→V173 clean (session/aproduct-0913 + ao/platform-console-agents follow-up).
- Live-path fixes from the A:// product-depth evidence run (session/aproduct-0913): fabric-transport job inserts now populate `dag_node_id` (the API schema's NOT NULL column the runtime test schema defaults) — `submit_intent`/`enqueue_job` previously 500'd on the API schema; intent-created runs are stamped with the authenticated owner via new `sqlite_store::set_run_owner` so the V168 owner-scoped cowork reads see them; `register_principal` and the principals list normalize `a://workspace/…` URIs to the canonical stripped workspace id (matching the built-in seeds and `cowork_runs.workspace_id`) so claims and list filters actually match; `GET /runs/:id/jobs` falls back to the canonical store when the legacy in-memory mirror has no rows, and `POST …/jobs/:job_id/state` only drives the mirror when the mirror holds the job — canonical-only jobs go straight to the lease-safe projection helper.

### Known build gotcha (not a runtime bug)

- `refinery::embed_migrations!` in `cmd/allternit-api/src/db.rs` is a proc macro and cannot emit `cargo:rerun-if-changed`: adding a file under `cmd/allternit-api/migrations/` does **not** trigger a rebuild. `touch cmd/allternit-api/src/db.rs` (or any clean build) before running, or the binary silently embeds the previous migration set.
- Updated `~/Desktop/allternit-parity-handoff.md` to reflect Phase 9 slices 9 and 10.

## [2026.08.09] — Phase 9 admin control plane closes (in progress)

### Added

- Admin MCP tunnels management API with mTLS/OAuth validation.
- Outcome rubrics DSL, scoring, and global templates.
- Admin analytics endpoints for cost, token usage, request volume, and per-user cost.
- Managed agent quickstart checklist API.
- Federation issuer/rule CRUD.
- Normalized cross-provider LLM refusal detection.

## [2026.07.31] — Phase 8 managed agents / admin / MCP / files

### Added

- Service account CRUD with scoped API keys and rotation.
- Spend limits and increase-request approval flow.
- User profile enrollment with signed URLs.
- Server-side MCP dispatcher and SDK connectors.
- Agent permission policy DSL and approval store.
- Session-scoped agent file store.

## [2026.07.15] — Phase 7 SDKs / skills / webhooks / hooks

### Added

- Vertex AI provider adapter.
- Signed webhook delivery with HMAC-SHA256.
- Skill registry loader and `SKILL.md` format.
- Python SDK with provider transforms.
- Inference hooks execution engine.

## [2026.06.30] — Phase 6 PDF / citations / OpenAI-compat

### Added

- PDF content blocks and file API.
- Citations service and RAG attribution fallback.
- `pdf_process` Tool Belt tool.
- OpenAI batch shim and SDK citation annotations.

## [2026.06.15] — Phase 5 SDK / API parity closes

### Added

- Model registry metadata and `service_tier: 'flex'` support.
- Context window editing and warnings.
- Vision input and tool-use-with-images support.
- SDK middleware hook system.
- Batch worker execution and error handling.

## [2026.05.31] — Phase 4 Docs / GTM

### Added

- Public API reference and provider parity matrix.
- Gizzi CLI and SDK quickstart documentation.
- Admin, security, and self-hosting documentation.

## [2026.05.15] — Phase 3 Enterprise control plane

### Added

- Public API idempotency and rate-limit middleware.
- Session-scoped resources.
- MCP tunnel security scaffold.
- SCIM v2 endpoints and compliance API.

## [2026.04.30] — Phase 2 User surfaces & distribution

### Added

- SDK retry/backoff interceptor and rate-limits endpoint.
- Deployment, work-queue, and memory-store endpoints.
- `gizzi config profile` and permission profiles.
- Workspace and RBAC admin endpoints.

## [2026.04.15] — Phase 1 Agent runtime & tools

### Added

- Batch Messages API and token-counting endpoint.
- WebSocket session events and interrupt endpoint.
- Native web search/fetch, text editor, bash, and memory tools.
- Agent runtime dispatcher.

## [2026.04.01] — Phase 0 Core request/response parity

### Added

- OpenAI-compatible chat completions gateway.
- Anthropic/OpenAI/Kimi provider adapters.
- Reasoning, cache control, JSON schema response format, and tool choice.
- Admin API keys, vault, and idempotency.
