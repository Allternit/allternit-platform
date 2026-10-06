# Allternit Repository Architecture

This document describes the production-grade repository setup for the Allternit ecosystem.

## Monorepo + Satellite Repos

We use a **monorepo + satellite repo** architecture:

- **`allternit-platform`** (this repo) — Core platform monorepo. Source of truth for the full stack.
- **Satellite repos** — only where a tool needs its own repo (release downloads, brew/scoop, GPL isolation). See below.

## Core Monorepo (`allternit-platform`)

The monorepo contains everything needed to build, test, and deploy the full Allternit platform.

```
allternit/
├── cmd/                      # CLI binaries and API servers
│   ├── allternit/
│   ├── allternit-api/        # Main API server (Rust)
│   ├── allternit-cloud-api/  # Cloud deployment API (Rust)
│   ├── allternit-cloud-wizard/
│   ├── allternit-mux/
│   ├── allternit-node/       # VPS edge agent
│   ├── allternit-computer-cloud/
│   ├── allternit-hosted-runtime/
│   ├── agent-daemon/
│   ├── cli/
│   ├── gizzi-code/           # Gizzi Code CLI source
│   ├── gizzi-core/
│   └── launcher/
├── services/                 # Long-running services (memory, voice, registry, orchestration; includes gateway/routing — allternit-tools-gateway Rust crate, workspace-service, ssh-bridge, replies-runtime; vendored: open-connector, docmost*)
├── domains/                  # Domain logic (agent, computer-use, governance, kernel; agent-swarm archived → archive/agent-swarm, live agent tools at tools/agent-swarm/; cowork compose stack moved to tools/cowork-integration/stack/, cowork runtime crates remain in infrastructure/executor/cowork/)
├── infrastructure/           # Cloud providers, executors, bridges (alias: infra/ symlink)
├── mcp/                      # Model Context Protocol servers and crates (computers-server, core, mcp-client, servers)
├── drivers/                  # VM and hardware drivers (firecracker, apple-vf)
├── commrails/                # CommRails agent communication/coordination substrate (Rust; crate allternit-commrails)
├── platform/                 # Contracts, protocols, Rust SDK, plugin runtime, shared packages
├── platform/packages/        # Internal @allternit/* TS libraries (39; consolidated from packages/@allternit/ on 2026-09-18, S3)
├── sdk/                      # Public SDK packages
├── surfaces/                 # Web apps and desktop surfaces
│   ├── allternit-desktop/    # Desktop shell (Electron)
│   ├── allternit-docs/       # Docs site content
│   ├── allternit-extensions/ # Browser extensions
│   ├── allternit-mobile/     # Mobile surface
│   ├── computer-embed/       # Embeddable computer-use surface
│   ├── gizzi-github-action/  # GitHub Action surface
│   ├── gizzi-vscode/         # VS Code extension surface
│   ├── office.allternit.com/ # Office surface
│   ├── phone-remote/         # Phone remote-control surface
│   ├── platform.allternit.com/ # Cloud console
│   └── docs/                 # Docs surface
├── vendor/                   # Vendored third-party code (harnessrouter-ce, session-migrate)
├── docs/                     # Documentation hub (archive/, gap-analysis/, learnings/, programs/, reports/, research/, specs/)
├── spec/                     # Contract schemas ONLY (Contracts/ — read from disk by validate_law.py, context-pack-builder, and the gateway service; the rest of the old spec/ moved to docs/specs/ 2026-09-18)
├── scripts/                  # Repo automation (A://Labs Canvas pipeline, builds; adhoc/ for one-offs)
├── bin/                      # Executable helpers (dev-up, ci-gate, ...)
├── dev/                      # Dev-ops + migration scripts
├── tests/                    # Acceptance/e2e/integration/load suites
├── tools/                    # Misc tooling (cowork-integration, deployment, mcp-servers)
├── config/                   # allternit.json + system config (read by live code)
├── resources/                # company.json (config:company:write output) + vm/
├── patches/                  # pnpm patchedDependencies
├── templates/                # System templates (system/ VM YAML: base-desktop, node-dev)
├── tmp/                      # Scratch evidence dirs + test scripts (tracked, session-scoped)
├── archive/                  # Retired material: card plugins, orphan crates, card-templates, alabs-curator
├── agent-ledger/             # Signed session record (LEDGER.md + summaries/) — stays at root by design
├── alabs-generated-courses/  # A://Labs courseware source of truth (+ demos/)
├── alabs-module-template/    # Shared HTML shell for course modules
└── worktree-manager/         # Git worktree management crate
```

> The agent workspace surface (`ai.allternit.com`) is **not** in this repo — it moved to the private
> satellite `Gizziio/allternit-ai` in the 2026-09-15 OSS split. The root `ui` symlink and the old
> `rails/` directory were deleted the same week; the real communication substrate is `commrails/`.

Inside `docs/`:

```
docs/
├── ...                       # Existing documentation hub
├── audit/                    # Platform audit reports
├── design/                   # Design system + reference data (DESIGN.md at this level; ui-ux-pro-max/ was .shared/)
├── learnings/                # One-off docs with no program prefix (S7)
├── marketing/                # Brand/marketing templates and README
├── parity-reports/           # Competitive parity reports (was .parity-reports/)
├── parity-reports-archive/   # Archived parity scraper scripts (was .parity-reports-archive/)
├── pipeline/                 # Pipeline program docs and helper scripts (was .pipeline/)
├── programs/                 # Phase-organized program docs by filename prefix (S7: swarm/, rails/, gizzi/, ios/, cloud-agents/, ao/, acu/, media-plugins/)
├── research/                 # Active research & planning docs (+ adr/) — moved from repo root 2026-09-18
├── reports/                  # Dated reports — moved from repo root 2026-09-18
├── specs/                    # Specs (incl. provider-routing/, design/, python-heavy-agents/ — moved from repo root spec/ 2026-09-18)
├── upstream/                 # Upstream fork provenance (sources.yaml)
├── learning/
│   └── remix-content/        # Remix pipeline course content + plans/
└── projects/                 # Ephemeral project trackers (allternit-cloud/ holds MASTER_TRACKING.md + handoffs/; remote-control-gap-fix)
```

### Runtime-state directories that must stay at root

The following dot-directories are hardcoded into live code or required by `AGENTS.md`. They stay at the repository root:

| Directory | Why it stays |
|-----------|--------------|
| `.allternit/` | Runtime state: peers, WIHs, artifacts, context-packs. Referenced by `cmd/allternit-api/`, `sdk/allternit-sdk/`, `domains/computer-use/`, `dev/scripts/`, and `AGENTS.md`. |
| `.gizzi/` | gizzi-code runtime state and brand files. Referenced by `cmd/gizzi-code/src/runtime/context/config/config.ts` and tests. |
| `.steering/` | Steering checkpoint + hook system. Required by `AGENTS.md`. |

> Reorganized 2026-07-22: removed `plugins/` (empty; card plugins live in `archive/plugins/`, runtime in `platform/plugins/`), root `src/`, `data/`, `public/`, `proof/`, `output/`, `dispatch-screenshots/`, `Desktop/` (accidental commit), and merged `analysis/` → `docs/gap-analysis/`, `reports/` → `docs/reports/`, `alabs-demos/` → `alabs-generated-courses/demos/`, `remix-plans/` → `remix-content/plans/`, `agent/`/`templates/`/`alabs-curator/` → `archive/`.
>
> Reorganized 2026-08-27: removed improperly-linked nested worktrees (`allternit-session-grok-bot-0-18-integration`, `allternit-session-multica-runtime-align`) and scratch `.tmp-*` entries from the index; deleted `.beads/`; moved `marketing/`, `upstream/`, `remix-content/`, `.pipeline/`, `.parity-reports/`, `.parity-reports-archive/`, and `.shared/` into `docs/`; moved ad-hoc root scripts into `scripts/audit/`.
>
> Reorganized 2026-09-18 (S6/S7): root `reports/`, `research/`, and most of `spec/` moved into `docs/reports/`, `docs/research/`, and `docs/specs/`; root `MASTER_TRACKING.md` and the `ALLTERNIT_CLOUD_*HANDOFF*.md` pair moved to `docs/projects/allternit-cloud/` (+ `handoffs/`); `DESIGN.md` → `docs/design/`, `AGENT_CREATION_CHECKLIST.md` → `docs/`, `ANTHROPIC_TO_ALABS_MAPPING.md` → `docs/learnings/`; loose `docs/` depth-1 program docs filed into `docs/programs/<program>/` (S7). Deliberate root exceptions, kept because live code reads them from repo root: `spec/Contracts/` (validate_law.py, context-pack-builder, gateway service), `GIZZI.md` (workspace-instruction file loaded by allternit-api/gizzi-code from cwd), `THIRD-PARTY-NOTICES.md` (electron-builder extraFiles in the desktop release).
>
> Dissolved roots (2026-09-18 reorg): `api/` → `services/` (S2, PR #595); `packages/@allternit/*` → `platform/packages/*` (S3, PR #597, 39 packages); `platform/sdk` → `platform/rust-sdk` (S4, PR #584).

## Ownership rules (source of truth)

Where new code goes — adopted 2026-09-18 (S0 of the folder reorganization):

- **`cmd/`** = executables (CLI binaries, API servers, daemons).
- **`services/`** = things that run (long-running services).
- **`platform/`** = contracts, protocols, types, plugins, and internal `@allternit/*` TypeScript libraries.
- **`sdk/`** = public SDK surface only. **Location frozen** — do not move without an explicit decision.
- **`domains/`, `infrastructure/`, `surfaces/`** = existing semantics (domain logic; cloud providers/executors/bridges; web + desktop apps).
- **No new root-level markdown.** The sanctioned root docs are exactly six — `README.md`, `AGENTS.md`, `REPO_STRUCTURE.md`, `SECURITY.md`, `CHANGELOG.md`, `LICENSE` — plus the two documented live-code exceptions (`GIZZI.md`, `THIRD-PARTY-NOTICES.md`). Every other doc goes to `docs/` per the `docs/README.md` taxonomy. Docs conventions are linted by `scripts/docs-lint.cjs` (the docs lint gate — run it when touching `surfaces/docs/`).

`packages/@allternit/` was dissolved into `platform/packages/` on 2026-09-18 (S3): all internal `@allternit/*` TypeScript libraries now live there (workspace resolution is by package `name`, so consumers were unaffected). The former `api/` root was dissolved into `services/` on 2026-09-18 (S2): `gateway/routing` (allternit-tools-gateway), `workspace-service`, `ssh-bridge`, and `replies-runtime` now live under `services/`.

## Eight-plane module ownership (Kernel ABI 1.0.0)

Adopted with the Agency Kernel (WP2). Authority: `spec/Contracts/kernel/v1`. New kernel code belongs to exactly one plane; "Where" is the current home, not a promise to move.

| # | Plane | Owns | Where today |
|---|-------|------|-------------|
| 1 | Experience / API | one external agent identity, SDK/API, streams, threads, artifacts | `cmd/`, `services/`, `sdk/` |
| 2 | Agent | TaskIR, AgentState, Agent ISA, capability semantics | `commrails/src/kernel/{registry,isa}.rs`, `spec/Contracts/kernel/v1` |
| 3 | Work Orchestration | ComputeGraph, node lifecycle, leases, campaigns, wait/wake, WIH projection | `commrails/src/kernel/{lifecycle,graph,projection}.rs`, `domains/kernel/drivers/dag-wih-integration` |
| 4 | Cognition | S0-S3, Decision and Judge runtimes, verification cognition | `domains/kernel/drivers/` |
| 5 | Context + State | ContextCompiler, ChunkStore, memory, cognitive state | `domains/kernel/drivers/context-pack-builder`, `services/memory` |
| 6 | Execution / Model | execution router, model pool, tool runtimes, fabric placement | `domains/kernel/drivers/allternit-providers`, `services/tools` |
| 7 | Governance + Observability | policy, receipts, replay, evals, provenance, budgets | `domains/kernel/drivers/system-law`, `platform/` |
| 8 | Product Operations | attention, approvals, channels, remote control, needs-you queue | `surfaces/` |

Q2: the Work Runtime ledger owns lifecycle. Q3: ComputeGraphIR is authoritative; the WIH DAG is a one-way disposable projection.

## Three SDKs

The repo carries three distinct SDKs — keep them straight:

| SDK | Location | Language | Release tag |
|-----|----------|----------|-------------|
| Public Allternit SDK | `sdk/` | TypeScript | `sdk/v*` |
| Rust SDK | `platform/rust-sdk/rust/` (crates `sdk-core`, `sdk-transport`, `sdk-policy`, `sdk-functions`, `sdk-apps`) | Rust | cargo workspace |
| Gizzi SDK | `cmd/gizzi-code/packages/sdk/` | TypeScript | `gizzi-sdk/v*` |

> NOTE: `platform/sdk/` → `platform/rust-sdk/` rename is **done** (2026-09-18, S4, PR #584) so `sdk/` (public TS SDK) is unambiguous.

## Satellite Repos

All source lives in this monorepo. The other repos exist only where a tool needs its own repo:

| Repo | Purpose |
|------|---------|
| [`allternit-ai`](https://github.com/Gizziio/allternit-ai) (private) | Workspace UI for ai.allternit.com, m.allternit.com and the Desktop app |
| [`allternit-websites`](https://github.com/Gizziio/allternit-websites) (private) | Marketing, services, labs and docs-adjacent sites |
| [`desktop`](https://github.com/Gizziio/desktop) | Allternit Desktop release downloads (Mac + Windows). Built by `release-desktop.yml` here |
| [`gizzi-code`](https://github.com/Gizziio/gizzi-code) | Hosts the `hosted-runtime-*` Linux binary the cloud runtime downloads (`cmd/allternit-hosted-runtime`) |
| [`homebrew-tap`](https://github.com/Gizziio/homebrew-tap) | `brew install --cask allternit` and the gizzi formula |
| [`scoop-bucket`](https://github.com/Gizziio/scoop-bucket) | Windows `scoop install gizzi-code` |
| [`allternit-tts`](https://github.com/Gizziio/allternit-tts) | GPL-licensed Kokoro TTS child process, kept separate for licensing |

The older extracted copies (`allternit-sdk`, `allternit-api-client`, `allternit-docs`,
`gizzi-code-docs`, `allternit-assets`) stopped tracking this repo in September 2026 and are
not maintained. Do not push to them; the code here is current.

## Development Workflow

1. All development happens in `allternit-platform` through a PR to `main`.
2. Releases are tags on this repo: `gizzi-code/v*` (CLI, also published to npm),
   `sdk/v*`, `gizzi-sdk/v*`. Desktop releases are published to the `desktop` repo.
3. `allternit-bot-latest` is a rolling release for vendor-agent sandboxes. It is never
   marked Latest, because the gizzi installer reads this repo's Latest release.

## NPM Organization

All packages are published under the **`@allternit`** scope:

```bash
npm install @allternit/sdk
npm install @allternit/plugin-sdk
npm install @allternit/api-client
npm install -g @allternit/gizzi-code
npm install -g @allternit/marketresearchcard-plugin
```
