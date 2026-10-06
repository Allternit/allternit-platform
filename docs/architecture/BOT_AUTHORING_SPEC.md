# Bot Authoring Spec

**Status:** Mixed. Each area is marked **Implemented** (code cited), **Partial**, or **Specified / Planned**. This is the exact contract for creating a user-created bot (`a://workspace/{ws}/bot/{id}`) — what you must do today, and what the full profile will require.

Companion: `AL_IMPLEMENTATION_SPEC.md`, `GIZZI_WORKER_SPEC.md`, `A_PROTOCOL_SCHEMA.md`.

---

## 0. The two records every bot has

A bot spans **two records, now linked at creation** (V163):

1. **Product/agent record** — `agents` table
   (`cmd/allternit-api/migrations/V1__baseline_schema.sql`; API:
   `cmd/allternit-api/src/agent_routes.rs`). Existing columns: `id`, `user_id`,
   `name`, `description`, `type` (`'worker'` default), `parent_agent_id`,
   `model`, `provider`, `capabilities` (JSON), `system_prompt`, `tools`,
   `max_iterations`, `temperature`, `config`, `status`, `workspace_id`, `avatar`,
   `identity_key`, `last_run_at`.
2. **Execution principal** — `cowork_principals` (V152; registration via
   `POST /api/v1/fabric/transport/principals`, `fabric_transport_routes.rs`).
   Fields and auth: `A_PROTOCOL_SCHEMA.md` §1.

**Implemented invariant (V163):** creating an agent with a `workspace_id`
mints the fabric principal `a://workspace/{ws}/bot/{agent_id}` in the same
transaction (`agent_routes.rs::create_agent`), records it on
`agents.principal_id`, and returns the bearer token **exactly once** in the
creation response (`principal_token`). Rotations go through
`POST /api/v1/fabric/transport/principals/:id/provision-token` (user auth).

## 1. Identity — **Implemented (dual record) / Partial (linkage)**

- Canonical address: `a://workspace/{workspace}/bot/{bot_id}` — workspace-scoped, survives model swaps (§8.3).
- Persist the address in both records. Short alias `a://bot/{id}` only inside a resolved workspace; never persist it standalone.

## 2. Roles — **Specified / Planned**

The v0.1 role vocabulary (`orchestrator`, `worker`, `reviewer`, `observer`,
`human`, `system`) is defined in the contract but **no roles column exists**
on `agents` or `cowork_principals`. Type inference today comes from
`agents.type` (`worker` default) and hardcoded product roles (Al, Gizzi).
Planned: `roles TEXT (JSON)` on the bot profile, enforced at policy evaluation.

## 3. Capabilities — **Implemented**

- Declare on the principal record (`capabilities` JSON array).
- Jobs declare `required_capabilities` (`CreateJobRequest`, `routes_cowork.rs`);
  eligibility = all required ∈ principal's set (`claim_job`, `sqlite_store.rs`).
- Smallest-set rule: declare only what the bot actually uses; capability means
  *can*, policy means *may* (§8.6).

## 4. Model / router policy — **Partial**

- `agents.model` + `agents.provider` select a fixed model today; per-run
  `policy_profile` (`cowork_runs.policy_profile`, V5) exists as a string but
  has undefined semantics — no router policy engine consumes it for bots.
- Workspace-level routing exists at the LLM gateway
  (`cmd/allternit-api/src/llm_gateway/`, provider routing + data-residency
  pins), but that is request routing, not per-bot policy.
- **Specified / Planned:** `model: { policy: router:auto }` with per-bot
  overrides, delegating to the gateway's provider-routing policy.

## 5. Compute policy — **Specified / Planned**

`compute: { policy: local|vm|byo|cloud }` is vocabulary only. Execution
placement is currently decided by the caller (local engine vs VM drivers vs
desktop sandboxes — `cmd/allternit-api/src/computer_routes.rs`,
`bot_desktop_*`). Fabric Transport computes eligibility and lease ownership
but has no placement dimension. Planned: a placement field resolved against
workspace policy and available compute.

## 6. Approval policy — **Implemented (transport path)**

- Bindings: scope + lifecycle in `A_PROTOCOL_SCHEMA.md` §3.
- Risk policy: workspace overrides in `cowork_approval_policy` (V158);
  evaluation in `risk_policy.rs` (first-match rules, canonical risk table).
- A bot's protected actions gate on `check_approval` under its lease; expiry
  and invalidation semantics are the recovery contract, not suggestions.

## 7. Memory scope — **Implemented (v0.1 grants)**

- Entries carry `owner_principal` + `grants` (V165); `/cowork/memory` read/
  search/write accept a `principal` scope and enforce owner+grants server-
  side with default-deny (`A_PERMISSION_DENIED` on cross-principal writes
  without a grant). Legacy unowned entries stay visible (back-compat).
- Persona-level "Al's memory" continuity remains product work.



- Memory stores exist (`memory_router`/`memory_kernel`, sessions memory,
  `cowork_memory_entries`), all user/workspace-scoped today.
- **No per-principal memory scoping rule is enforced.** A bot's reads are
  whatever its tools can reach. Planned: scope tags (`principal`, `project`,
  `run`) with read grants evaluated at retrieval; Al's memory is never
  implicitly readable by bots (§9 of `COWORK_A_PROTOCOL_ARCHITECTURE.md`).

## 8. Connectors — **Partial (broker v0.1 landed)**

- Connectors and a credential vault exist (`connector_routes`,
  `allternit_vault`, `cloud_credentials_routes`); the broker flow of §8.5
  (scoped, attributed, expiring sessions in place of raw secrets) is **not
  implemented**. Planned: `Principal → capability request → broker → scoped
  session → external system`, with revocation and attribution.

## 9. Triggers — **Partial**

Implemented today:

- manual Cowork request (chat → run creation);
- schedule/cron — `Scheduler` (`allternit-cowork-scheduler`), backed by
  `cowork-schedules.db`, firing HTTP into the API (`main.rs::initialize_cowork_scheduler`);
- API call — `POST /runs` / `POST /runs/:id/jobs`.

**Specified / Planned:** webhook/connector events (`webhook_subscription_routes`,
`webhook_trigger_routes` exist but create their own pipeline, not canonical
Intent/Run work), file/system events, Factory peer messages, condition/watch
triggers. The convergence rule stands: every trigger must produce canonical
run/job state, not a parallel engine (§11 of the architecture doc).

## 10. Budget / concurrency — **Specified / Planned**

No per-principal budget or concurrency columns exist. Rate limits are
per-organization at the gateway (org rate limits, admin routes), not per-bot.

## 11. Authoring checklist (today)

1. Create the product record (`POST /api/v1/agents` family, `agent_routes.rs`)
   with a `workspace_id` — the execution principal is minted automatically
   and the token returned once. Store it in the bot's runtime secret store —
   never in the prompt, repo, or `config` JSON.
2. (Standalone principals, no agent record:) `POST /api/v1/fabric/transport/principals`.
3. Declare the workspace risk policy if the bot has protected actions (`cowork_approval_policy`, V158) and set `max_delegation_depth` if delegation chains apply (V164).
4. Submit work as canonical intents (`POST /api/v1/fabric/transport/intents`) or create runs/jobs directly with a `causation_chain` when delegating (§8.15; cycles/depth rejected at write).
5. Give the bot a claim loop per `FABRIC_TRANSPORT.md` §16.
6. Emit events with stable `event_id`s for any client-originated events (V157).
7. Surface state through Cowork (the `/fabric-transport` control view or the API) from canonical run/job rows — never from the bot's own narration.
