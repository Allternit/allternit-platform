# Allternit CommRails

Unified system for **work execution under policy gates** across DAG/WIH/runs/leases/ledger/vault.

> Renamed from `allternit-agent-system-rails` (BA-0b). Package `allternit-commrails`, lib `allternit_commrails`, bins `allternit-commrails` / `allternit-commrails-service`. The old bin names (`allternit-rails`, `allternit-rails-service`, `rails`) remain as one-release shims pointing at the same sources.

## Naming Locks

- **Gate** = WIH policy enforcer for dag/node/run transitions and tool execution.
- Do **not** use “kernel” or “control plane” in this subsystem.
- Event `actor.type` uses `"gate"` (not `"kernel"`).

## Structure

All code for this system lives under this folder.

```
allternit-commrails/
  docs/
    architecture/      # layered breakdown + CLI command mapping
    runner/            # runner mutation catalog + README
    vendor-notes/      # harvested behavior from upstream ticket/mail tooling
  src/                 # implementation
  spec/                # locked invariants and contracts
  schemas/             # JSON schemas (event envelope + event payloads)
  projections/         # projection rules
```

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

## `commrails` CLI

The `commrails` binary (the `rails` bin name remains as a one-release shim) is the ticket/DAG workflow CLI:

```bash
cargo run -p allternit-commrails --bin commrails -- init
commrails ticket new "title" --description "..." --priority P1
commrails dag block <ticket> <blocker>
commrails ready --explain
commrails doctor
commrails memory learn "..." --tags api
commrails echo new "..."
commrails template new "name" --steps steps.json
commrails batch exec batch.json
commrails gate add <ticket> manual --description "..."
commrails lock acquire branch:main
commrails setup claude
commrails query --entity tickets status:open
commrails sync linear pull
commrails compact all
commrails kill status
commrails slo --window 60
commrails dolt status
```

`commrails` supports real pull/push/status sync with GitHub, Linear, Jira, Azure
DevOps, GitLab, and Notion. It also includes a kill switch, SLO metrics, and an
optional Dolt storage backend.

See [cli/README.md](./cli/README.md) for a product overview and
[cli/RAILS_CLI.md](./cli/RAILS_CLI.md) for the full command reference.

## Remote bridge (default off)

`allternit-commrails bridge serve` is a scoped listener that lets a remote agent
(e.g. Chief on the shared box, over the mesh) create/read plans and send/read
mail with a bearer identity from `allternit-commrails identity add`. Pickup,
close, leases, wait-gate resolution and gate decisions are never available to
it. Loopback only unless `--allow-remote`. Spec, threat model and enablement
steps: [spec/BRIDGE.md](./spec/BRIDGE.md); box client:
[`tools/commrails-bridge-client/`](../tools/commrails-bridge-client/).

See [docs/architecture/README.md](./docs/architecture/README.md) for a full feature/architecture breakdown before you run the test suites.
Hidden runtime stores (`.allternit/`) are documented in [docs/architecture/README.md](./docs/architecture/README.md#layer-c---ledger-bus-transports) and tracked during `allternit commrails init` and `commrails init`.
