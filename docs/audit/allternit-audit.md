# Allternit Platform — Comprehensive Read-Only Audit

> Repository: `~/Desktop/allternit-workspace/allternit`  
> Generated: 2026-08-27  
> Scope: catalog every major surface, service, component, and capability for 1:1 comparison with bb (getbb.app).

---

## 1. Top-level Layout

| Directory | Role |
|-----------|------|
| `cmd/` | CLI binaries, API servers, daemons, TUI/agent runtime entrypoints |
| `api/` | Backend API service crates (gateway, workspace, kernel/rails-api, cloud-backend) |
| `services/` | Long-running services: memory, voice, registry, orchestration, connectors, office-engine, page-agent, remote-control, etc. |
| `domains/` | Domain logic: agent workspace, computer-use, governance, kernel drivers/services |
| `infrastructure/` (symlink `infra/`) | Cloud providers, executors, bridges, schedulers, VPS nodes, mesh, chrome-stream, local compute |
| `platform/` | Contracts, protocols, SDK adapters, plugin runtime, shared types |
| `packages/@allternit/` | Internal SDK packages (office suite, plugin-sdk, provider-adapters, workflow-engine, etc.) |
| `sdk/` | Public SDK packages: `@allternit/sdk`, `capsule-sdk`, `computer-use`, Python SDK |
| `surfaces/` | User-facing apps: web (`ai.allternit.com`), desktop, mobile iOS, extensions, docs, VS Code, GitHub Action, office surface |
| `rails/` | Allternit Agent System Rails — work execution / DAG / ledger / gate engine |
| `scripts/` | Repo automation: A://Labs Canvas pipeline, build/deploy helpers, audits |
| `bin/` | Shell helpers and verification scripts |
| `tools/` | Agent-orchestrator, agent-swarm, MCP servers, cowork integration, deployment plists |
| `tests/` | Vitest/cargo integration, e2e, acceptance shell tests, benchmarks |
| `docs/` | Primary documentation hub (public + internal) |
| `research/` | ADRs, research notes, planning docs |
| `spec/` | Contract schemas and design specs |
| `alabs-generated-courses/`, `alabs-module-template/` | A://Labs courseware source and template |
| `archive/` | Retired modules and orphaned code |
| `config/`, `resources/`, `dev/`, `worktree-manager/` | Config, generated company.json, dev scripts, git-worktree crate |

Key layout docs:
- `REPO_STRUCTURE.md` — monorepo + satellite repo architecture
- `README.md` — platform overview, service ports, dev commands
- `DESIGN.md` — design system v2.0
- `GIZZI.md` — agent orientation for surfaces and backend split
- `AGENTS.md` — A://Labs pipeline + Rails peer mode

---

## 2. Surfaces / UI

### 2.1 `surfaces/ai.allternit.com` — Main web surface
- **Framework:** Vite + React 18 SPA (package name `@allternit/ai`).
- **Dev port:** `3013` (`pnpm dev:platform`).
- **Routing:** React Router DOM; unknown paths fall back to `index.html` for SPA routing.
- **Key views (36+):**
  - `src/views/ChatView.tsx`, `AgentView.tsx`, `AgentHub.tsx`
  - `src/views/AgentStudioView.tsx`, `NativeAgentView.tsx`
  - `src/views/LabsView.tsx`, `CertificationsPanel.tsx` (A://Labs)
  - `src/views/CanvasProtocolView.tsx`, `AllternitCanvasView.tsx`
  - `src/views/MarketplaceView.tsx`, `CatalogView.tsx`
  - `src/views/MemoryKernelView.tsx`, `HistoryView.tsx`
  - `src/views/cowork/`, `src/views/project/`, `src/views/settings/`, `src/views/playground/`
  - `src/views/code/`, `src/views/browser/`, `src/views/swarm/`, `src/views/model-lab/`, etc.
- **State / data:** Zustand stores, Redux Toolkit, TanStack Query, local SQLite via Drizzle/Prisma.
- **Component systems:** Radix UI primitives, Tailwind CSS, custom `allternit-design` token system, tldraw canvas, BlockSuite/BlockNote editors, AG Grid, Recharts, React-Three-Fiber.
- **Auth:** Clerk (`@clerk/clerk-react`).
- **Deploy target:** Cloudflare Pages (`wrangler.toml` at surface root; `.github/workflows/deploy-cloudflare-pages.yml`).

### 2.2 `surfaces/allternit-desktop` — Electron shell
- **Package:** `@allternit/desktop`.
- **Architecture:** Electron wrapper around the same Vite web UI; dev mode loads `localhost:3013`, packaged mode bundles a static platform build (`scripts/prepare-platform-static.cjs`).
- **Features:** local VM management via Apple Virtualization.framework, Lima, CUA driver, mesh node, office-engine sidecar.
- **Targets:** macOS (arm64 DMG/zip), Linux (AppImage/deb), Windows (NSIS) via `electron-builder`.
- **Key files:** `package.json` (build config), `src/main/`, `src/preload/`, `BUILD.md`, `AUDIT.md`.

### 2.3 `surfaces/allternit-mobile/ios` — iOS app
- Native Swift/SwiftUI app (separate codebase; shares Clerk identity and backend contracts).
- Requires `Frameworks/Mesh.xcframework` (gitignored, copied from working checkout).
- Build-time pairing mode defaults to Cloud; pass `ALLTERNIT_API_BASE_URL=http://127.0.0.1:8013/api/v1` for local backend.

### 2.4 `surfaces/allternit-extensions/`
- `allternit-extension` — Chrome MV3 side-panel extension built with WXT.
- `allternit-office-addin` — Excel/PowerPoint/Word task-pane add-in built with Vite + `office-addin-debugging`.
- `extension-shared/` — canonical `ExtensionSidepanelShell.tsx` + adapter interface.
- `native-host/` — native messaging host.

### 2.5 `surfaces/office.allternit.com`
- Standalone office surface (`@allternit/office-surface`) for Docs, Sheets, Slides, PDF; Vite + React.

### 2.6 `surfaces/docs`
- Mintlify documentation site (`@allternit/docs`); `docs.json` config.

### 2.7 `surfaces/gizzi-vscode`
- VS Code extension: `gizzi.openPanel`, `gizzi.explainCode`, `gizzi.refactorCode`, `gizzi.generateTests`, `gizzi.reviewCode`, `gizzi.fixErrors`.

### 2.8 `surfaces/gizzi-github-action`
- GitHub Action for AI review/generation (`action.yml`, `src/index.ts`).

---

## 3. CLI / Entrypoints

### Rust binaries (`cmd/`)
| Binary | Path | Purpose |
|--------|------|---------|
| `allternit-api` | `cmd/allternit-api/src/main.rs` | Local REST/WebSocket API server (port 8013) |
| `allternit-cloud-api` | `cmd/allternit-cloud-api/src/main.rs` | Centrally hosted cloud deployment API (Contabo VPS, systemd) |
| `allternit-cloud-wizard` | `cmd/allternit-cloud-wizard/` | Cloud setup wizard library/crate |
| `allternit-factory` | `cmd/allternit-factory/src/main.rs` | Factory engine; its pane engine (`factory/pane`) runs every terminal and agent PTY (replaced `allternit-mux`) |
| `allternit-platform` | `services/orchestration/platform-orchestration-service/src/main.rs` | Service orchestrator that starts all services |
| `allternit-platform-launcher` | `cmd/launcher/src/main.rs` | Single-binary launcher + static UI server |
| `voice-service` | `services/voice/src/main.rs` | Rust voice TTS/STT service (port 8001) |
| `allternit-simple-memory` | `services/memory/src/main.rs` | Rust memory HTTP adapter |
| `registry-apps` | `services/registry/apps-registry/src/main.rs` | App registry service |
| `allternit-commrails` / `rails` | `rails/cli/src/` | Rails ticket/DAG/agent execution CLI |

### TypeScript/Bun CLIs
| CLI | Path | Purpose |
|-----|------|---------|
| `gizzi` / `gizzi-code` | `cmd/gizzi-code/` (`package.json` bin) | AI terminal interface / agent harness / runtime server (port 4096) |
| `allternit` | `cmd/cli/` (`@allternit/cli`) | Allternit CLI (commander) |
| `@allternit/agent-daemon` | `cmd/agent-daemon/` | Local agent runtime daemon (WebSocket) |
| `@allternit/plugin` CLI | `packages/@allternit/plugin-sdk/bin/allternit-plugin.js` | Plugin SDK CLI |

### `bin/` helpers
- `dev-up` / `dev-down` — start/stop dev stack
- `verify-all.sh`, `verify-phase*.sh` — boot/phase verification
- `run-agent-service.sh`, `run-graph`, `start-shellui.sh`
- `generate-codebase-md`, `generate-gateway-config`
- `allternit-extension-load.sh`, `allternit-inject-local-config.sh`

### `gizzi-code` command surface (illustrative)
The TUI/CLI exposes a large command set under `cmd/gizzi-code/src/cli/commands/`, including:
`session`, `skills`, `plugin`, `runtime`, `brain`, `memory`, `tasks`, `mcp`, `desktop`, `mobile`, `remote`, `cost`, `usage`, `permissions`, `review`, `export`, `diff`, `doctor`, `plan`, `vault`, `theme`, `vim`, etc.

---

## 4. Backend / API

### 4.1 `cmd/allternit-api` — Local API server
- **Language:** Rust (Axum, Tokio, SQLite, WebSocket).
- **Port:** `8013` default.
- **Auth:** Clerk JWT verification via JWKS; public routes bypass Clerk.
- **Key route trees (from `src/main.rs` route assembly):**
  - `/api/v1/...` — protected V1 surface: agents, sessions, memory, files, inbox, workflows, SSH, swarm, cowork, canvas, tasks, marketplace, admin, billing, SCIM, etc.
  - `/api/agent-chat` — chat bridge
  - `/api/...` — tools, local brain/engine/studio, HAR API, web proxy, OAuth, onboarding, ACI, page agent, analytics, playground, checkpoints, office engine, provider, etc.
  - `/rails/*` and `/api/rails/*` — Rails system HTTP surface
  - `/stream/*` — event streams (WebSocket)
  - `/terminal/*` — terminal sessions
  - `/mcp/*` — MCP server/tunnel routes
  - `/metrics/*` — Prometheus metrics
  - `/v1/*` — OpenAI-compatible LLM gateway (public, virtual-key auth)
  - `/health`, `/status` — public health/status
- **Key modules:** `src/v1_routes.rs`, `src/llm_gateway/`, `src/rails/`, `src/cowork/`, `src/db.rs`, `src/auth.rs`, `src/config.rs`.
- **Migrations:** 92 numbered SQL migrations in `cmd/allternit-api/migrations/`.

### 4.2 `cmd/allternit-cloud-api` — Cloud API
- **Language:** Rust (Axum, SQLx, PostgreSQL only — `init_db` parses `PgConnectOptions`).
- **Port:** `8080` in `fly.toml`; `3001` default locally.
- **Hosted on:** Contabo VPS `mail` via systemd binary swap (`cmd/allternit-cloud-api/deploy-contabo.sh`, runbook `docs/Operations/CLOUD_API_VPS_DEPLOY.md`). The legacy Fly.io/Railway paths were removed (2026-09 dead-code cleanup) along with the broken `cmd/allternit-cloud-api/Dockerfile`.
- **Responsibilities:** multi-tenant users, device pairing, hosted runtimes, Clerk webhooks, billing entitlements, Fly runtime lifecycle, cost tracking, quota service, scheduler/executor services.
- **Key route files:** `src/routes/auth.rs`, `routes/instances.rs`, `routes/hosted_runtimes.rs`, `routes/gizzi_instances.rs`, `routes/providers.rs`, `routes/deployments.rs`, `routes/wizard.rs`, `routes/mesh.rs`, `routes/health.rs`.
- **Migrations:** 14 SQL migrations in `cmd/allternit-cloud-api/migrations_pg/`, embedded and applied via `sqlx::migrate!`.

### 4.3 `api/` directory
- `api/core/cloud-backend` — cloud backend components
- `api/gateway/routing` — `allternit-tools-gateway` (Rust gateway routing)
- `api/kernel/rails-api` — Rails API integration
- `api/services/workspace-service` — `allternit-workspace-service`
- `api/services/replies-runtime`, `api/services/ssh-bridge`

### 4.4 Gateway / Routing
- `infrastructure/gateway/gateway_registry.json`
- `api/gateway/routing/`
- `services/gateway/` (http/python/service/stdio/unified)

---

## 5. Services

| Service | Location | Notes |
|---------|----------|-------|
| **Memory** | `services/memory/` | Rust `allternit-memory-fabric` + TypeScript Always-On Memory Agent (`services/memory/agent/`) using SQLite + Ollama |
| **Voice** | `services/voice/` | Rust `voice-service` (TTS/STT/sessions) + Python FastAPI wrapper (`api/`) backed by XTTS/Piper/Whisper |
| **Registry** | `services/registry/apps-registry/`, `services/orchestration/control-plane/unified-registry/registry/` | App registry; unified registry for agents/skills/tools |
| **Orchestration** | `services/orchestration/` | Control plane, workflows, policy service, budget-metering, conflict-arbitration, node-registry, edge-runner |
| **Open Connector** | `services/open-connector/` | Vendored open-source connector gateway (OOMOL) for 1000+ providers/actions; Fly + Cloudflare deploy |
| **Docmost** | `services/docmost/` | Vendored wiki (excluded from pnpm workspace) |
| **Mailflare** | `services/mailflare/` | Vendored agent email rail (Cloudflare Workers + D1) |
| **Remote Control Push** | `services/remote-control-push/` | Cloudflare Worker push service for remote control |
| **Office Engine** | `services/office-engine/` | GenOffice Phase 0 prototype service (`@allternit/office-engine-service`) |
| **Page Agent** | `services/page-agent/` | Shared browser-automation package (`@allternit/page-agent`) |
| **Document Generator** | `services/document-generator/` | Python service with Dockerfile + OpenAPI spec |
| **Vault Viewer** | `services/vault-viewer/` | Standalone vault viewer with Dockerfile |
| **Local Engine** | `services/local-engine/` | Rust local engine crate |
| **Process Driver** | `services/process-driver/` | Rust process driver |
| **Self-Improve** | `services/self-improve/` | Rust service |
| **Session Manager** | `services/session-manager/` | Rust session manager |
| **VM Executor** | `services/vm-executor/` | Rust VM executor |
| **ETRID** | `services/etrid/` | Rust ETRID service |
| **ML** | `services/ml/` | `pattern-service`, `prompt-pack-service` |
| **Android Bridge** | `services/android-bridge/` | Python bridge |
| **Bonsai Local** | `services/bonsai-local/` | Local inference app |
| **Open Notebook** | `services/open-notebook/` | Python research notebook service |
| **Runtime** | `services/runtime/adapter/` | Runtime adapter |

---

## 6. Agent Runtime

### 6.1 `cmd/gizzi-code` — Gizzi Code
- **Runtime:** Bun/TypeScript; builds to `dist/gizzi-code`.
- **Roles:**
  1. Terminal/TUI agent interface (Ink + React 19).
  2. Local agent runtime server (port 4096) — platform UI bridges into it.
- **Key subsystems:**
  - `src/runtime/` — server, providers, memory, tools, skills, plugins, verification, artifacts, brain, voice
  - `src/cli/` — command parsing, Ink TUI components, hooks
  - `src/shared/` — shared tools (`FileReadTool`, `GrepTool`, `GlobTool`, `SkillTool`, `WorkflowTool`, etc.)
  - `src/runtime/tools/` — agent tools: `BashTool`, `FileEditTool`, `REPLTool`, `ScheduleCronTool`, `SendMessageTool`, `ListPeersTool`, `TeamCreateTool`, etc.
  - `src/runtime/providers/` — provider adapters/discovery (OpenAI, Anthropic, Google, Ollama, etc.)
  - `src/runtime/memory/` — session memory service
  - `src/runtime/server/routes/` — HTTP routes consumed by the platform
  - `src/runtime/gizzi-core/` — core runtime services, Rails peer integration
- **Database:** SQLite via Drizzle (`~/.local/share/gizzi-code/gizzi.db`); migration files in `cmd/gizzi-code/migration/`.

### 6.2 `rails/` — Allternit Agent System Rails
- **Crate:** `allternit-commrails`.
- **Purpose:** deterministic work execution under policy gates (DAG, WIH, runs, leases, ledger, vault, mail, tickets).
- **Key modules:** `src/gate.rs`, `src/ledger.rs`, `src/work.rs`, `src/tickets.rs`, `src/doctor.rs`, `src/peer.rs`, `src/steer.rs`, `src/mail.rs`, `src/vault.rs`, `src/memory.rs`, `src/orchestrator/`, `src/receipts/`.
- **CLI:** `rails init`, `rails ticket new`, `rails dag block`, `rails ready`, `rails doctor`, `rails memory learn`, `rails gate add`, `rails sync linear pull`, etc. (`rails/cli/`).
- **HTTP surface:** mounted in `allternit-api` at `/rails` and `/api/rails`.

### 6.3 `domains/agent/` — Agent workspace
- Client-side 5-layer workspace architecture (markdown mirrors of kernel state).
- `allternit-agent-workspace/` Rust crate, `allternit-embodiment/` crate, agent profiles, prompts, roles, templates.

### 6.4 `tools/agent-orchestrator/` & `tools/agent-swarm/`
- Orchestrator skill, agent browser, autoland, tool registry.

---

## 7. Data / Persistence

### Primary stores
| Store | Used By | Location / Config |
|-------|---------|-------------------|
| **SQLite** | `allternit-api` local state | `data_dir/allternit.db` (`cmd/allternit-api/src/main.rs:183`) |
| **SQLite** | `allternit-cloud-api` | `DATABASE_URL` → `sqlite:///data/api.db` (`fly.toml`) or PostgreSQL |
| **SQLite** | Web platform (Drizzle) | `surfaces/ai.allternit.com/data/allternit.db` (`drizzle.config.sqlite.ts`) |
| **SQLite (Prisma)** | Web platform | `surfaces/ai.allternit.com/prisma/schema.prisma` (same SQLite, models for SSH/artifacts/workflows/sessions/conversations) |
| **SQLite** | Always-On Memory Agent | `services/memory/agent/` (`memory.db`) |
| **SQLite** | Gizzi Code | `~/.local/share/gizzi-code/gizzi.db` (`cmd/gizzi-code/drizzle.config.ts`) |
| **D1** | Mailflare (Cloudflare) | `services/mailflare/drizzle/migrations/` |
| **PostgreSQL** | Registry apps-registry (optional) | `services/registry/apps-registry/Cargo.toml` (`sqlx` with postgres feature) |

### Other data systems referenced
- **Redis** — used by unified registry (`services/orchestration/control-plane/unified-registry/registry/Cargo.toml`) and web surface (`ioredis`, `@upstash/redis`).
- **Qdrant** — vector DB dependency in workspace `Cargo.toml`.
- **Upstash** — Redis used by web surface and remote-control push.

### Key schema files
- `surfaces/ai.allternit.com/prisma/schema.prisma`
- `surfaces/ai.allternit.com/src/lib/db/schema-sqlite.ts`
- `surfaces/ai.allternit.com/drizzle.config.sqlite.ts`
- `cmd/allternit-api/migrations/V*__*.sql` (92 migrations)
- `cmd/allternit-cloud-api/migrations_pg/*.sql` (14 migrations, embedded via `sqlx::migrate!`)
- `services/mailflare/drizzle/migrations/`

---

## 8. Plugin / SDK Ecosystem

### Public / published SDKs (`sdk/`)
| Package | Path | Exports |
|---------|------|---------|
| `@allternit/sdk` | `sdk/allternit-sdk/` | AI runtime, providers, ACP, OpenAPI client, computer-use, harness, tools |
| `capsule-sdk` | `sdk/capsule-sdk/` | Capsule runtime SDK |
| `computer-use` | `sdk/computer-use/` | Computer-use SDK (JS + Python) |
| `allternit-python` | `sdk/allternit-python/` | Python SDK (`pyproject.toml`) |

### Internal SDK packages (`packages/@allternit/`)
- `plugin-sdk` — universal plugin runtime (`@allternit/plugin-sdk`)
- `provider-adapters` — LLM provider adapters
- `workflow-engine`, `orchestrator`, `cowork-engine`, `executor-core`
- `api-client` — TypeScript API client
- `visual-state`, `ix`, `viz`, `form-surfaces`
- Office suite: `office-docs-app`, `office-docx-engine`, `office-pptx-engine`, `office-xlsx-engine`, `office-file-parse`, `office-pdf-app`, `office-sheets-app`, `office-slides-app`, etc.
- `replies-contract`, `replies-reducer`, `request-scorer`, `browser-tools`, `computer-use-protocol`, `os-contracts`, `parallel-run`, `types`, `util`

### Platform plugin runtime
- `platform/plugins/` (`@allternit/plugins`) — plugin system crate/package, built-in plugins under `surfaces/ai.allternit.com/src/plugins/built-in/`

### Satellite repos (published independently)
| Repo | NPM |
|------|-----|
| `allternit-sdk` | `@allternit/sdk` |
| `allternit-plugin-sdk` | `@allternit/plugin-sdk` |
| `allternit-api-client` | `@allternit/api-client` |
| `gizzi-code` | `@allternit/gizzi-code` |
| `allternit-plugins` | `@allternit/*-plugin` |

---

## 9. Infrastructure / Deployment

### Containers / Docker
Key Dockerfiles (the cloud API has none — it deploys as a static binary; its
old Dockerfile was removed 2026-09 after the Contabo/systemd migration):
- `cmd/allternit-hosted-runtime/Dockerfile`
- `cmd/gizzi-code/Dockerfile`
- `infrastructure/0-infra/docker/Dockerfile.api`
- `infrastructure/0-infra/docker/Dockerfile.shell`
- `infrastructure/0-infra/docker/docker-compose.prod.yml`
- `infrastructure/0-infra/docker-compose/development.yml`
- `infrastructure/chrome-stream/Dockerfile`
- `services/open-connector/docker-compose.yml`
- `services/open-connector/docker/Dockerfile`
- `services/vault-viewer/Dockerfile`
- `services/document-generator/Dockerfile`
- `services/voice/Dockerfile.voice`

### Fly.io
- Removed earlier (Contabo migration, pre-2026-09): root `fly.toml` and
  `deploy-cloud-api-fly.yml` no longer exist. `services/open-connector/fly.toml`
  remains for OpenConnector.

### Railway
- Removed 2026-09 (dead deploy path): `railway.json` built the broken
  `cmd/allternit-cloud-api/Dockerfile` and `.github/workflows/deploy-cloud-api-railway.yml`
  deployed it with no test gate. The live path is the Contabo workflow
  (`deploy-cloud-api-contabo.yml`) per `docs/Operations/CLOUD_API_VPS_DEPLOY.md`.

### Cloudflare
- `wrangler.toml` (root) — Cloudflare Pages for `ai.allternit.com`.
- `surfaces/ai.allternit.com/wrangler.toml`
- `surfaces/office.allternit.com/wrangler.toml`
- `services/remote-control-push/wrangler.toml`
- `services/mailflare/wrangler.jsonc`
- Workflows: `deploy-cloudflare-pages.yml`, `deploy-office-cloudflare.yml`, `deploy-remote-control-cloudflare.yml`, `deploy-remote-control-push.yml`, `deploy-docs-cloudflare.yml`.

### GitHub Actions
Selected workflows in `.github/workflows/`:
- `deploy-cloud-api-contabo.yml` (live cloud API deploy; the fly/railway workflows were removed)
- `deploy-cloudflare-pages.yml`, `deploy-office-cloudflare.yml`, `deploy-remote-control-cloudflare.yml`, `deploy-remote-control-push.yml`, `deploy-docs-cloudflare.yml`
- `ci-desktop.yml`, `release-desktop.yml`, `publish-gizzi-code-npm.yml`, `release-gizzi-code.yml`
- `publish-hosted-runtime.yml`
- ~~`sync-platform-export.yml`~~ — retired 2026-09-11 (see `docs/Operations/CLOUDFLARE_MAPPING.md`).
- `visual-verification.yml`, `typography-validation.yml`, `ci-docs.yml`
- `discovery-blog.yml`, `discovery-briefings.yml`, `discovery-features.yml`
- `configure-clerk-organization.yml`, `build-office-addin.yml`

### Virtualization / sandbox drivers
- `drivers/apple-vf/` — Apple Virtualization.framework driver
- `drivers/firecracker/` — AWS Firecracker driver
- `drivers/firecracker-guest-agent/` — Firecracker guest agent
- `drivers/opensandbox/` — OpenSandbox driver
- `infrastructure/chrome-stream/` — Chrome streaming with Firecracker support
- `infrastructure/vps-node/` — VPS node provisioning and runtime templates
- `infrastructure/executor/` — self-hosted container executor (Docker/bollard)

### Mesh / networking
- `infrastructure/mesh/headscale/` — Headscale coordination server
- `infrastructure/mesh/tsnet-ios/` — iOS Tailscale integration

---

## 10. Key Configuration Files

| File | Purpose |
|------|---------|
| `package.json` | Root workspace scripts, Vitest, dev dependencies |
| `pnpm-workspace.yaml` | Workspace package globs + exclusions (open-connector, docmost, mailflare) |
| `Cargo.toml` | Root Rust workspace with 100+ member crates |
| `bunfig.toml` | Bun scope config |
| `rust-toolchain.toml` | Rust 1.94.1 |
| `Makefile` | `make api`, `make build`, `make dev`, `make test` |
| `wrangler.toml` | Cloudflare Pages deployment |
| ~~`fly.toml`~~ / ~~`railway.json`~~ | removed 2026-09 — cloud API deploys to the Contabo VPS via `deploy-cloud-api-contabo.yml` |
| `.mcp.json` | MCP server registry (rails, dak-runner, sequential-thinking, context7, superpowers, remotioncard, iosappbuild) |
| `config/system/`, `resources/company.json` | Company config |
| `surfaces/ai.allternit.com/.env.example`, `.env.local`, `.env.production` | Web surface env |
| `cmd/gizzi-code/gizzi.json` | Gizzi runtime config |

---

## 11. Tests / QA

### Test runner matrix
| Category | Tool | Location |
|----------|------|----------|
| Rust unit/integration | `cargo test` | crate-level `tests/` and `#[cfg(test)]` |
| TypeScript unit/integration | Vitest | `tests/`, package `__tests__/`, `surfaces/ai.allternit.com/tests/` |
| E2E browser | Playwright | `tests/e2e/`, `surfaces/ai.allternit.com/playwright.config.ts`, `surfaces/allternit-desktop/playwright.config.ts` |
| Acceptance | Shell scripts | `tests/acceptance/` (60+ shell tests) |
| Performance | Vitest bench | `tests/benchmarks/performance.bench.ts` |
| Load | k6 | `tests/load/workflow_api.js` |

### Notable acceptance tests
- `tests/acceptance/test_cli_*.sh` — CLI/registry/gate behavior
- `tests/acceptance/test_memory_*.sh` — memory policy/replay
- `tests/acceptance/test_gateway_*.sh` — gateway routing
- `tests/acceptance/test_ui_*.sh` — UI-required panels
- `tests/acceptance/test_exec_*.sh` — deterministic execution/receipts

### QA scripts
- `scripts/launch-audit.ts` — A://Labs course audit
- `scripts/docs-lint.cjs`
- `scripts/validate-typography.py`
- `.github/workflows/visual-verification.yml`
- `.github/workflows/typography-validation.yml`

---

## 12. Documentation

### Top-level docs
- `README.md` — platform overview, ports, build instructions
- `REPO_STRUCTURE.md` — monorepo + satellite repos
- `DESIGN.md` — design system v2.0
- `GIZZI.md` — agent orientation (web/desktop/iOS/gizzi surfaces)
- `AGENTS.md` — A://Labs curriculum pipeline + Rails peer mode
- `PORTING_PROVEN_PATTERNS_INTO_GIZZI.md`
- `TODO-remote-control-gap-fix.md`

### `docs/` hub
- `docs/MASTER_INDEX.md` — unified documentation index
- `docs/Core_System/01-Reality/` — active architecture specs
- `docs/Core_System/02-Target/` — aspirational specs (SYSTEM_LAW, kernel, workflow, memory, security)
- `docs/Core_System/03-Gaps/` — gap analysis and migration plans
- `docs/Future_Blueprints/` — robotics, RLM, intent compiler, mobile parity
- `docs/Operations/` — deployment, BYOC, computer-use guides, port registry
- `docs/Business_Strategy/` — brand authority, manufacturing strategy
- `docs/Product_and_Content/` — A://Labs catalog, skills/templates
- `docs/public/` — public docs: API reference, CLI, SDK, tools, ACI, guides, parity matrix
- `docs/agent-activity-design/`, `docs/agent-tasks/` — agent UX/task specs
- `docs/architecture/` — ADRs
- `docs/Audits_and_Research/` — competitor audits, CrewAI mapping, MCP integration

### Surface READMEs
- `surfaces/ai.allternit.com/README.md`
- `surfaces/allternit-desktop/README.md`, `BUILD.md`, `AUDIT.md`
- `surfaces/allternit-extensions/README.md`
- `cmd/allternit-api/README.md`
- `cmd/allternit-cloud-api/API.md`, `QUICKSTART.md`, `RUNTIME_PAIRING.md`
- `services/memory/README.md`, `services/voice/README.md`, `services/open-connector/README.md`
- `rails/README.md`, `rails/cli/README.md`, `rails/cli/RAILS_CLI.md`

---

## 13. Notable Capability Map (for parity comparison)

| Capability | Where implemented |
|------------|-------------------|
| Multi-modal chat / agent sessions | `surfaces/ai.allternit.com/src/views/chat/`, `cmd/gizzi-code/src/runtime/session/`, `cmd/allternit-api/src/agent_session_routes.rs` |
| LLM gateway / virtual keys / model routing | `cmd/allternit-api/src/llm_gateway/` |
| Computer-use / browser automation | `domains/computer-use/`, `sdk/computer-use/`, `cmd/gizzi-code/src/runtime/utils/computerUse/`, `surfaces/ai.allternit.com/src/views/browser/`, `drivers/apple-vf/`, `drivers/firecracker/` |
| Memory / vector search | `services/memory/`, `services/memory/agent/`, `allternit-memory-fabric` |
| Workflows / DAGs | `packages/@allternit/workflow-engine/`, `rails/src/graph.rs`, `cmd/allternit-api/src/workflow_routes.rs` |
| Cowork / team execution | `infrastructure/executor/cowork/`, `cmd/allternit-api/src/cowork/`, `surfaces/ai.allternit.com/src/views/cowork/` |
| Connector ecosystem | `services/open-connector/`, `cmd/allternit-api/src/connector_routes.rs` |
| MCP server integration | `.mcp.json`, `mcp/`, `cmd/gizzi-code/src/runtime/services/mcp/`, `surfaces/ai.allternit.com/src/lib/ai/mcp/` |
| Plugin SDK / card plugins | `packages/@allternit/plugin-sdk/`, `platform/plugins/`, `surfaces/ai.allternit.com/src/plugins/` |
| Office document editing | `packages/@allternit/office-*`, `services/office-engine/`, `surfaces/office.allternit.com/` |
| Voice TTS/STT | `services/voice/`, `surfaces/ai.allternit.com/src/services/voice/` |
| Remote desktop / remote control | `services/remote-control-push/`, `cmd/allternit-api/src/remote_control_routes.rs`, `surfaces/ai.allternit.com/src/remote-control/` |
| Cloud deployment / VPS provisioning | `cmd/allternit-cloud-api/`, `infrastructure/cloud/`, `infrastructure/vps-node/`, `infrastructure/providers/hetzner/` |
| Billing / quotas / entitlements | `cmd/allternit-cloud-api/src/routes/hosted_entitlements.rs`, `services/orchestration/budget-metering/` |
| A://Labs learning platform | `alabs-generated-courses/`, `scripts/sync-incremental.ts`, `surfaces/ai.allternit.com/src/views/labs/` |
| Agent email | `services/mailflare/`, `cmd/allternit-api/src/agent_email_routes.rs` |

---

*End of audit.*
