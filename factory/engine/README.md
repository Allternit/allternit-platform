# Allternit Factory engine (`allternit-factory-engine`)

Unified system for **work execution under policy gates** across DAG/WIH/runs/leases/ledger/vault.
It is the library half of the Allternit Factory; the binary is `allternit-factory`
(`cmd/allternit-factory`), which also runs the pane engine (`factory/pane`).

> Formerly CommRails (`allternit-commrails`, before that `allternit-agent-system-rails`).
> The old binaries (`allternit-commrails`, `allternit-rails`, `allternit-commrails-service`,
> `allternit-rails-service`, and the portable `commrails`/`rails` CLI) are gone, not aliased:
> `allternit-factory serve` is the service, the four parts (`agents`, `orchestration`,
> `workflows`, `workspace`) are the commands, and every maintenance command lives under the
> hidden `allternit-factory internal rails …`.

## Naming Locks

- **Gate** = WIH policy enforcer for dag/node/run transitions and tool execution.
- Do **not** use “kernel” or “control plane” in this subsystem.
- Event `actor.type` uses `"gate"` (not `"kernel"`).

## Structure

All code for this system lives under this folder. `src/` is grouped by job (SPEC §5):

```
src/
  core/           events, IDs, state: ledger, index, projections, replay, prompt, query, compact, dolt
  gate/           the only writer: gate, hook (spawn gate), policy, killswitch, egress, fence, constraints
  agents/         backend (the pane engine), spawn, registry, view (Agent[]), peer, execenv
  orchestration/  send (Delivery), mail, bus, attention, steer, observer
  workflows/      templates, kernel (the template compiler), drive, wake, wait_gates, leases, merge_locks, dependencies
  workspace/      work, wih, campaign, receipts, judge, verification, vault, lessons, context, memory, echoes
  api/            service (HTTP), factory (/api/factory), mcp, workspace-service client, cli
  remote/         bridge (scoped remote listener + identities)
  tickets/        internal foreign-repo ticket area: tickets, graph, sync, batch, doctor, setup, rails_id
```

`lib.rs` re-exports every module at its old flat path (`crate::ledger`, `crate::hook`, …) so
callers keep compiling.

```
factory/engine/
  docs/
    architecture/      # layered breakdown + CLI command mapping
    runner/            # runner mutation catalog + README
    vendor-notes/      # harvested behavior from upstream ticket/mail tooling
  src/                 # implementation
  spec/                # locked invariants and contracts
  schemas/             # JSON schemas (event envelope + event payloads)
  projections/         # projection rules
```

## Agents, send and `/api/factory`

- **One spawn path.** Every agent session the engine starts (`workflows drive`
  executors `ao:<harness>`, `agents up` later) runs in a pane of the pane engine
  (`factory/pane`) through `agents::backend::PaneBackend`, which the
  `allternit-factory` binary installs at startup. There is no tmux path. The
  spawn gate, WIH policy, execution env, fence and headless capture are applied
  first (`agents::spawn`). A gated harness's hook is this engine's own
  executable, `allternit-factory internal hook …` (never a binary found on
  `PATH`; `$ALLTERNIT_FACTORY_BIN` overrides it for tests and dev harnesses).
- **One registry.** `~/.allternit/factory/registry.json` (`$ALLTERNIT_FACTORY_HOME`
  moves it), migrated once from `~/.agent-orchestrator/state.json`. Engine
  spawns and `allternit-factory pane ao spawn` both record there. It is
  reconciled against the live panes when the server starts and on every
  `agents ps`: a session whose pane is gone is `dead`, a live agent pane with no
  record is adopted (`ao-pane-<paneId>` for panes a person opened), and every
  session belongs to a bot (`local:<slug>` placeholder until allternit-api
  binds it). `agents ps` is that view merged with the peers, as API.md
  `Agent[]`, plus `engine {running, error}`.
- **One send path.** `orchestration send` / `POST /api/factory/send` resolves
  the target and delivers: terminal → verified paste, else the mailbox
  (`pane_queue`); hosted → a turn in the bot thread's Gizzi session; vendor →
  a vendor ticket; `channel:<threadId>` → the thread's channel relay. Hosted,
  vendor and channel go through allternit-api (`$ALLTERNIT_FACTORY_API_URL`,
  or the `x-allternit-api-base` its proxy sends, as the calling user). Every
  send is recorded as mail plus a `factory.delivery` ledger event, and the
  `Delivery` says what really happened (`verified`, `queued`, `best_effort`,
  `read_only`, `failed`).
- **`/api/factory`** (`api::factory`) serves API.md §3 on the engine service.
  Events are Server-Sent Events replayed from the ledger by `Last-Event-ID`.
  Team boot, the board and proof upload answer `404 not_found` "… is not
  built yet".
- **Tests.** `tests/determinism/` covers SPEC §6 (replay, dry run, idempotent
  create, error codes, dead panes); `tests/send_bindings.rs` the remote
  deliveries; `cmd/allternit-factory/tests/factory_api.rs` the HTTP contract
  against `docs/specs/allternit-factory/API.md`.

## Scope

This system **reimplements** proven ticket-tracking and agent-mail workflow logic.
We reference upstream behavior for correctness but **do not** depend on any external
tool at runtime.

### Advanced Capabilities (V2)
The system has been enhanced with enterprise-grade features for swarm coordination, human-in-the-loop interaction, and deep observability.
See [spec/SPEC_OVERVIEW.md](./spec/SPEC_OVERVIEW.md) for details on:
- **Elicitation Protocol** (Interactive forms/prompts)
- **Swarm Handoffs** (Dynamic agent transitions)
- **Execution Sampling** (Pass-through LLM generation)
- **Signal Broadcasting** (High-performance coordination)
- **GenAI Telemetry** (Token tracking & OpenTelemetry compliance)
- **OAuth Vault** (Credential management)

## Ticket area (library only)

The portable ticket/DAG CLI (`commrails` / `rails`, with `install.sh`) was
removed when the engine became the Allternit Factory. Its library code stays in
`src/tickets/` (tickets, typed dependencies, ready-list, sync providers for
GitHub, Linear, Jira, Azure DevOps, GitLab and Notion, setup recipes, batch,
doctor), and the engine's own ticket verbs remain under
`allternit-factory internal rails ticket …`.

## Remote bridge (default off)

`allternit-factory internal rails bridge serve` is a scoped listener that lets a remote agent
(e.g. Chief on the shared box, over the mesh) create/read plans and send/read
mail with a bearer identity from `allternit-factory internal rails identity add`. Pickup,
close, leases, wait-gate resolution and gate decisions are never available to
it. Loopback only unless `--allow-remote`. Spec, threat model and enablement
steps: [spec/BRIDGE.md](./spec/BRIDGE.md); box client:
[`tools/commrails-bridge-client/`](../../tools/commrails-bridge-client/).

See [docs/architecture/README.md](./docs/architecture/README.md) for a full feature/architecture breakdown before you run the test suites.
Hidden runtime stores (`.allternit/`) are documented in [docs/architecture/README.md](./docs/architecture/README.md#layer-c---ledger-bus-transports).
