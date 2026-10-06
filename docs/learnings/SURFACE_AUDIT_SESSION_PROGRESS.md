# Surface Audit — Session Progress Tracker

Started: 2026-08-03
Branch: `fix/review-decision-event-type` (issue #16/#19 fixes) + `docs/surface-audit-progress` (tracker updates)

**Process (same as established):** each item is re-investigated against the live codebase before any code is written. Items from the original audit that turn out to be stale or already implemented are corrected; only real, confirmed-live gaps are built.

**Legend:**
- `[ ]` Not started this session
- `[~]` Re-verification / build in progress
- `[x]` Verified and resolved (shipped, closed as stale, or documented as out-of-scope)
- `REAL` = confirmed live gap worth building
- `STALE` = audit claim was wrong; already exists, renamed, or dead code
- `DEFER` = real but intentionally out of scope / needs separate phase

---

---

**Tracker correction (this session):** the original audit tracker incorrectly marked every item `[x]` after triage. `[x]` now means the item is *resolved* (shipped, stale, or deferred/documented). REAL items that still need building are `[ ]` (pending) or `[~]` (active).

## Core Chat/Home (3)

| # | Item | Status | Finding | Action | PR |
|---|------|--------|---------|--------|-----|
| 1 | Projects (GAP → gizzi-code) | `[x]` | STALE — project model already exists: web `views/ProjectView.tsx`, gizzi-code `project/instance.ts`, iOS `Features/Projects/` | Closed as stale | |
| 2 | Artifacts Library (PARTIAL → upgrade) | `[x]` | STALE — web `views/library/LibraryView.tsx` ("Artifacts Library") + iOS `Features/Artifacts/Views/ArtifactsLibraryView.swift` both exist | Closed as stale | |
| 3 | Automation Tasks (Goals/Routines/Loops/Cron) (GAP → iOS) | `[x]` | PR #9 merged to main; cron jobs shipped on iOS | Closed as shipped | #9 |

## Cowork (15)

| # | Item | Status | Finding | Action | PR |
|---|------|--------|---------|--------|-----|
| 4 | Cowork workspace (CoworkRoot) (PARTIAL → iOS) | `[x]` | REAL — phase 1 shipped: launchpad + active-session shell + progress panel on iOS (`feat/ios-cowork-workspace`) | Shipped phase 1; phase 2/3 deferred | `feat/ios-cowork-workspace` |
| 5 | Cowork Runs view (PARTIAL → iOS) | `[x]` | STALE — RunsView.tsx registered but zero dispatchers in Cowork nav; only reachable via unrelated Products Discovery "Workflows" card | close (defer if re-scoped) | |
| 6 | Cowork Drafts view (GAP → iOS) | `[x]` | STALE — DraftsView.tsx registered, zero dispatchers anywhere; dead code | close | |
| 7 | Cowork Cron view (GAP → iOS) | `[x]` | STALE — renamed/merged into Automation Tasks (live, reachable via shared Home rail); iOS Automation Tasks itself absent from this checkout despite PR #9 claims | close; reconcile iOS branch state | |
| 8 | Cowork Project view (GAP → gizzi-code) | `[x]` | REAL — phase 1 shipped: `cowork-project` command lists Cowork projects, shows tasks by project, and creates tasks (`feat/gizzi-cowork-project`) | Shipped phase 1; phase 2/3 deferred | `feat/gizzi-cowork-project` |
| 9 | Cowork Documents view (GAP → iOS) | `[x]` | STALE — DocumentsView.tsx only reachable via unrelated Code-mode Skills Registry path, not Cowork nav | close (defer if re-scoped) | |
| 10 | Cowork Tables view (GAP → iOS) | `[x]` | STALE — TablesView.tsx registered, zero dispatchers; dead code | close | |
| 11 | Cowork Files view (GAP → iOS) | `[x]` | STALE — FilesView.tsx registered, zero dispatchers; dead code | close | |
| 12 | Cowork Exports view (PARTIAL → iOS) | `[x]` | STALE — ExportsView.tsx registered, zero dispatchers; dead code on web | close (defer if re-scoped) | |
| 13 | Cowork Insights panel (GAP → iOS, gizzi-code) | `[x]` | STALE — InsightsView.tsx registered, zero dispatchers; dead code on web too | close (defer if re-scoped) | |
| 14 | Cowork Activity panel (GAP → iOS, gizzi-code) | `[x]` | STALE — ActivityView.tsx registered, zero dispatchers; dead code | close | |
| 15 | Cowork Goals panel (PARTIAL → iOS) | `[x]` | STALE — GoalsView.tsx dead; real goals UI is a tab inside AutomationTasksView (duplicate of item #7) | close | |
| 16 | Cowork Wiki section viewer (PARTIAL → iOS) | `[x]` | STALE (recategorize) — WikiSectionViewer.tsx real/live but mounted app-wide via ArtifactSidecar, not Cowork nav | close under Cowork; defer under Artifacts Library | |
| 17 | Cowork Audit log viewer (GAP → iOS, gizzi-code) | `[x]` | STALE — AuditLogViewer.tsx only rendered inside dead TasksView.tsx; transitively unreachable | close (defer if re-scoped) | |
| 18 | Intelli-Schedule panel (GAP → iOS) | `[x]` | REAL — phase 1 shipped: Swift `IntelliScheduleEngine`, `IntelliScheduleStore`, `IntelliSchedulePanel`, entry from Cowork-mode chrome (`feat/ios-intelli-schedule`) | Shipped phase 1 | `feat/ios-intelli-schedule` |
| 19 | Harness Config panel (GAP → iOS) | `[x]` | STALE (recategorize) — HarnessConfigPanel.tsx real/live but mounted in OperatorBrowserView/DesignModeView, not Cowork | close under Cowork; defer under Operator/Design | |

## Code (17)

| # | Item | Status | Finding | Action | PR |
|---|------|--------|---------|--------|-----|
| 20 | Code workspace (CodeRoot) (PARTIAL → upgrade) | `[x]` | STALE — fully implemented, live (`CodeRoot.tsx`→`CodeSurfaceRouter.tsx`→`CodeThreadView`/`CodeCanvasView`) | Closed as stale; no defined "upgrade" scope | |
| 21 | Code Explorer (GAP → iOS) | `[x]` | STALE — `ExplorerView.tsx` (430 lines) live via `CodeModeAgentSession.tsx` explorer tab | Closed as stale; iOS port has a real reference impl | |
| 22 | Code Git panel (GAP → iOS) | `[x]` | STALE — `GitView.tsx` (412 lines) live via `CodeModeAgentSession.tsx` git tab | Closed as stale; iOS port has a real reference impl | |
| 23 | Code Skills view (GAP → iOS) | `[x]` | REAL — web `SkillsView.tsx` now fetches `/api/v1/team-skills`; iOS `CodeSkillsView` + `TeamSkillsClient` + store shipped (`feat/ios-code-skills`) | Shipped phase 1 | `feat/ios-code-skills` |
| 24 | Code Project view (PARTIAL → upgrade) | `[x]` | STALE — `CodeProjectView.tsx` (473 lines) live via `code-project` route + `ProjectDetailRouter.tsx` | Closed as stale; no defined "upgrade" scope | |
| 25 | Code Canvas (live preview split view) (PARTIAL → iOS) | `[x]` | REAL — phase 1 shipped: workspace shell + session tile grid + entry from CodeModeView (`feat/ios-code-canvas`). Full infinite canvas, drag/resize, non-session tiles deferred. | Shipped phase 1 | `feat/ios-code-canvas` |
| 26 | Code Preview Pane (GAP → iOS, gizzi-code) | `[x]` | STALE (dead code) — `CodePreviewPane.tsx` (208 lines) fully written, zero callers anywhere | Resurrect-or-retire decision needed before iOS port | |
| 27 | Orchestrator Center (PARTIAL → iOS) | `[x]` | STALE (dead code) — `OrchestratorCenter.tsx` (70 lines), no callers anywhere | Resurrect-or-retire decision needed | |
| 28 | Orchestration View (PARTIAL → iOS) | `[x]` | STALE (dead code) — `OrchestrationView.tsx` (156 lines), uses real `useUnifiedStore`, no callers | Resurrect-or-retire decision needed | |
| 29 | Goal Control Center (GAP → iOS) | `[x]` | STALE (dead code) — `GoalControlCenter.tsx` backed by live `/automation/goals` API, no callers | Shovel-ready; just needs nav wiring | |
| 30 | Kanban(+DAG) Board (GAP → iOS) | `[x]` | STALE (dead code) — `KanbanBoard.tsx`+`KanbanDAG.tsx` (536+503 lines), no callers | Resurrect-or-retire decision needed | |
| 31 | Debug View (PARTIAL → iOS) | `[x]` | STALE (dead/unreachable route) — `DebugView.tsx` (458 lines) registered as `debug` route, no nav caller | Needs a nav entry point, or retire | |
| 32 | Logs View (PARTIAL → iOS) | `[x]` | STALE (dead code) — `LogsView.tsx` (246 lines), not even registered as a route | Resurrect-or-retire decision needed | |
| 33 | Run Inspector (PARTIAL → iOS) | `[x]` | STALE (dead code) — `RunInspector.tsx` (124 lines), uses real stores, no callers | Resurrect-or-retire decision needed | |
| 34 | Run Replay (GAP → iOS, gizzi-code) | `[x]` | STALE (dead/unreachable route) — `RunReplayView.tsx` registered as `run-replay` route w/ real backend calls, no nav caller | Needs a caller (e.g. "View Replay" action) before porting | |
| 35 | Tools Registry (PARTIAL → iOS) | `[x]` | STALE (dead/unreachable route) — `ToolsView.tsx` registered as `registry` route, backed by live `/tools` API, no nav caller | Cheapest orphan to resurrect; full stack works | |
| 36 | Skills Registry (GAP → iOS) | `[x]` | DEFER — `SkillsRegistryView.tsx` (293 lines) unreachable route, calls nonexistent `/api/v1/skills/registry` | Needs backend spec before UI wiring or iOS work | |
| 37 | Promotion Dashboard (GAP → iOS, gizzi-code) | `[x]` | DEFER — `PromotionDashboardView.tsx` never routed at all, calls nonexistent `/api/v1/promotion/proposals` | Needs backend + product decision on scope | |
| 38 | Automation Tasks (Code) (GAP → iOS) | `[x]` | STALE — duplicate of #3; `code-automations` route renders the same `AutomationTasksView` already shipped in PR #9 | Closed as duplicate | #9 |

## ACI/Browser (8)

| # | Item | Status | Finding | Action | PR |
|---|------|--------|---------|--------|-----|
| 39 | ACI Browser surface (GAP → gizzi-code) | `[x]` | DEFER — web+iOS fully implemented (ACITabView/ACIWebBrowserView); gizzi-code is a CLI, no GUI surface to port to | Deferred — CLI/GUI mismatch | |
| 40 | Mini-apps Store (GAP → iOS) | `[x]` | REAL — phase 1 shipped: browse-only store with community catalog + public MCP registry, search, categories, pin/unpin, Safari open (`feat/ios-mini-apps-store`) | Shipped phase 1; install/start/runtime/embedded-web deferred | `feat/ios-mini-apps-store` |
| 41 | Mini-app frame/runtime (GAP → iOS) | `[x]` | REAL — phase 1 shipped: `MiniAppRuntimeView` WKWebView host + capabilities placeholder, wired from store Open button (`feat/ios-mini-app-runtime`) | Shipped phase 1; agent harness/ACP spawn/JS bridge deferred | `feat/ios-mini-app-runtime` |
| 42 | Office Add-ins — Word (GAP → iOS, gizzi-code) | `[x]` | DEFER — one shared web component (AciAddinView.tsx, OfficeHost type) covers all 3 hosts; no iOS/gizzi-code equiv; Add-ins run inside Office hosts | Deferred; scope as 1 shared view if pursued | |
| 43 | Office Add-ins — Excel (GAP → iOS, gizzi-code) | `[x]` | DEFER — duplicate target of #42 (same AciAddinView.tsx component) | Deferred, tracked with #42 | |
| 44 | Office Add-ins — PowerPoint (GAP → iOS, gizzi-code) | `[x]` | DEFER — duplicate target of #42 (same AciAddinView.tsx component) | Deferred, tracked with #42 | |
| 45 | Office & Extensions view (GAP → iOS, gizzi-code) | `[x]` | DEFER — web AppsExtensionsView.tsx (652 lines) hub exists; no iOS/gizzi-code hub; bundles deferred #40-44 | Deferred with dependents | |
| 46 | Operator Browser (GAP → gizzi-code) | `[x]` | DEFER — web OperatorBrowserView.tsx; iOS already has documented parity (BrowserChatView.swift); gizzi-code CLI/GUI mismatch | Deferred — CLI/GUI mismatch | |

## Design/Creative (13)

| # | Item | Status | Finding | Action | PR |
|---|------|--------|---------|--------|-----|
| 47 | Design Mode — Questions tab (GAP → iOS, gizzi-code) | `[x]` | DEFER — AppMode.swift:6-8 documents Design mode deliberately skipped on iOS (opens external window on web instead) | Deferred — documented scope decision | |
| 48 | Design Mode — Mobile tab (GAP → iOS, gizzi-code) | `[x]` | DEFER — same documented iOS Design-mode exclusion (AppMode.swift:6-8) | Deferred, same as #47 | |
| 49 | Design Mode — Docs tab (GAP → iOS, gizzi-code) | `[x]` | DEFER — same documented iOS Design-mode exclusion (AppMode.swift:6-8) | Deferred, same as #47 | |
| 50 | Design Mode — Handoff tab (GAP → iOS, gizzi-code) | `[x]` | DEFER — same documented iOS Design-mode exclusion (AppMode.swift:6-8) | Deferred, same as #47 | |
| 51 | Design Mode — Graph tab (GAP → iOS, gizzi-code) | `[x]` | DEFER — same documented iOS Design-mode exclusion (AppMode.swift:6-8) | Deferred, same as #47 | |
| 52 | Design Mode — Pipeline tab (GAP → iOS, gizzi-code) | `[x]` | DEFER — same exclusion; component (ContentPipelineView.tsx) also duplicated by row #58 | Deferred, same as #47 | |
| 53 | Design Marketplace/Registry (GAP → iOS) | `[x]` | DEFER — DesignRegistryView.tsx is also the `market` tab of DesignModeView; covered by iOS Design-mode exclusion | Deferred, same as #47 | |
| 54 | Design Compare (GAP → iOS, gizzi-code) | `[x]` | STALE — ViewRegistry.tsx:387-391 `design-view-compare` route aliases to DesignRegistryView (same as marketplace); no real compare UI exists anywhere | Closed as stale — nothing to port | |
| 55 | Form Surfaces (GAP → iOS, gizzi-code) | `[x]` | REAL — phase 1 shipped: iOS `FormSurfacesView` registry + dynamic renderer for all web field types, entry from `ComposerPlusSheet` (`feat/ios-form-surfaces`) | Shipped phase 1; submit/persist deferred | `feat/ios-form-surfaces` |
| 56 | Canvas Protocol (PARTIAL → iOS) | `[x]` | REAL — phase 1 shipped: all 40+ `CanvasViewType`s + `CanvasSpec` model + spec CRUD helpers in `CanvasClient`, specs ride in existing canvas metadata (`feat/ios-canvas-protocol`) | Shipped phase 1; runtime instantiation of view types deferred | `feat/ios-canvas-protocol` |
| 57 | Design Team Workspace (GAP → iOS, gizzi-code) | `[x]` | DEFER — DesignTeamWorkspace.tsx is the `team` tab of DesignModeView; covered by iOS Design-mode exclusion | Deferred, same as #47 | |
| 58 | Content Pipeline (GAP → iOS, gizzi-code) | `[x]` | STALE — duplicate of #52; both point at the same ContentPipelineView.tsx / `pipeline` tab | Closed as stale/duplicate of #52 | |
| 59 | Live Artifact Editor (PARTIAL → upgrade) | `[x]` | DEFER — LiveArtifactEditor.tsx exists; gap is localStorage-only persistence on web (an upgrade item, not a platform-port gap) | Deferred — out of platform-parity scope | |

## Terminal/Infra (11)

| # | Item | Status | Finding | Action | PR |
|---|------|--------|---------|--------|-----|
| 60 | Monitor (GAP → iOS) | `[x]` | REAL — phase 1 shipped: iOS `MonitorView` + `MonitorClient` + `MonitorStore` wired to `/api/v1/monitor/*`, entry from Settings Infrastructure (`feat/ios-monitor`) | Shipped phase 1; backend routes don't exist yet | `feat/ios-monitor` |
| 61 | Runtime Operations (GAP → iOS, gizzi-code) | `[x]` | REAL — phase 1 shipped: iOS `RuntimeOperationsView` hub aggregating budget/replay/prewarm/execution-mode cards + `RuntimeOperationsClient` (`feat/ios-runtime-operations`) | Shipped phase 1; detail/management screens deferred to #62-64 | `feat/ios-runtime-operations` |
| 62 | Budget Dashboard (PARTIAL → iOS) | `[x]` | REAL — phase 1 shipped: iOS `BudgetDashboardView` with stat cards, quota editor (text + quick presets), pressure bars, alerts; reachable from Runtime Operations hub (`feat/ios-budget-dashboard`) | Shipped phase 1 | `feat/ios-budget-dashboard` |
| 63 | Replay Manager (GAP → iOS, gizzi-code) | `[x]` | REAL — phase 1 shipped: iOS `ReplayManagerView` with search, capture-level filter, stat cards, and per-row replay; `RuntimeOperationsStore.executeReplay` wired to `POST /runtime/replay/sessions/:run_id/execute` (`feat/ios-replay-manager`) | Shipped phase 1; gizzi-code port deferred | `feat/ios-replay-manager` |
| 64 | Prewarm Manager (GAP → iOS, gizzi-code) | `[x]` | REAL — phase 1 shipped: iOS `PrewarmManagerView` with pool-size slider, Apply/Warmup actions, health chips, pool topology, and recent activity; `RuntimeOperationsStore` wired to `POST /runtime/prewarm/pool` and `/runtime/prewarm/warmup` (`feat/ios-prewarm-manager`) | Shipped phase 1; gizzi-code port deferred | `feat/ios-prewarm-manager` |
| 65 | Nodes (GAP → iOS, gizzi-code) | `[x]` | REAL — phase 1 shipped: iOS `NodesManagerView` + `NodesStore` + `NodesClient` listing compute nodes, online/offline stats, delete, and join-token generation via `/api/v1/nodes/*` (`feat/ios-nodes-manager`) | Shipped phase 1; terminal/deploy wizard and gizzi-code port deferred | `feat/ios-nodes-manager` |
| 66 | Cloud Deploy (GAP → iOS, gizzi-code) | `[x]` | REAL — phase 1 shipped: iOS `CloudDeployManagerView` + `CloudDeployStore` + `CloudDeployClient` listing deployments, manual/automated creation form, cancel, and status polling via `/api/v1/deployments*` (`feat/ios-cloud-deploy`) | Shipped phase 1; agent-assisted wizard, WebSocket events, and gizzi-code port deferred | `feat/ios-cloud-deploy` |
| 67 | Capsule Manager (GAP → iOS) | `[x]` | STALE — web `views/CapsuleManagerView.tsx` is an explicit stub (`stub-capsule-001`, "replace when @allternit/shell-ui available"), no real feature to port | Re-flag as web-build task, not a port gap | |
| 68 | VPS & Servers panel (GAP → iOS, gizzi-code) | `[x]` | REAL — phase 1 shipped: iOS `VPSServersManagerView` + `SSHConnectionsStore` + `SSHConnectionsClient` for listing, testing, creating, connecting, disconnecting, and deleting SSH connections via `/api/v1/ssh-connections*` (`feat/ios-vps-servers`) | Shipped phase 1; agent install, provider deploy, and gizzi-code port deferred | `feat/ios-vps-servers` |
| 69 | Cloud Instances panel (GAP → iOS, gizzi-code) | `[x]` | REAL — phase 1 shipped: iOS `CloudInstancesManagerView` + `CloudInstancesStore` + `CloudInstancesClient` for the BYO-VPS wizard on the cloud API (`/api/v1/cloud/wizard/deployments*`): list sessions, manual SSH start, advance, bootstrap, cancel, delete (`feat/ios-cloud-instances`) | Shipped phase 1; automated providers, saved provider tokens, and gizzi-code port deferred | `feat/ios-cloud-instances` |
| 70 | Enterprise BYOC panel (GAP → iOS, gizzi-code) | `[x]` | REAL — phase 1 shipped: iOS `EnterpriseBYOCPanelView` + `BYOCStore` + `BYOCClient` for profile/usage summary and cloud-credential management via `/api/v1/me`, `/api/v1/usage/summary`, `/api/v1/cloud-credentials*` (`feat/ios-enterprise-byoc`) | Shipped phase 1; gizzi-code port deferred (BYOC hits unrelated) | `feat/ios-enterprise-byoc` |

## DAG suite (15)

| # | Item | Status | Finding | Action | PR |
|---|------|--------|---------|--------|-----|
| 71 | DAG Integration Page (PARTIAL → upgrade) | `[x]` | REAL — rebuilt `views/DagIntegrationPage.tsx` as a full DAG-suite index with navigation tiles for Orchestration, Knowledge, Security, Browser/Execution, and Observability views (`feat/web-dag-integration-page`) | Shipped index rebuild; individual gizzi-code ports remain under #72–#86 | `feat/web-dag-integration-page` |
| 72 | Ontology Viewer (GAP → gizzi-code) | `[x]` | REAL — gizzi-code `/ontology` command shipped: Ink TUI that fetches DAG plans, WIHs, context packs, and receipts from `/api/rails/*` and displays summary + recent DAGs + WIHs (`feat/gizzi-dag-ontology`) | Shipped phase 1 CLI port; deeper graph rendering and node-edge details deferred | `feat/gizzi-dag-ontology` |
| 73 | Directive Compiler (GAP → gizzi-code) | `[x]` | REAL — gizzi-code `/directive` command shipped: compiles natural-language directives into DAG plans via `POST /api/rails/plan` and renders the resulting plan as Markdown (`feat/gizzi-directive-compiler`) | Shipped phase 1 CLI port; interactive refinement and plan editing deferred | `feat/gizzi-directive-compiler` |
| 74 | GC Agents (GAP → gizzi-code) | `[x]` | REAL — gizzi-code `/gc` command shipped: status, `archive <wih-id>`, `verify`, and `rebuild` subcommands backed by `/api/rails/wihs`, `/vault/*`, `/gate/verify`, and `/index/rebuild` (`feat/gizzi-gc-agents`) | Shipped phase 1 CLI port | `feat/gizzi-gc-agents` |
| 75 | Receipts Viewer (GAP → gizzi-code) | `[x]` | REAL — gizzi-code `/receipts` command shipped: query receipts with `--dag/--node/--wih/--kind/--limit` filters and `show <id>` detail via `/api/rails/receipts*` (`feat/gizzi-receipts-viewer`) | Shipped phase 1 CLI port | `feat/gizzi-receipts-viewer` |
| 76 | Security Dashboard (GAP → gizzi-code) | `[x]` | REAL — gizzi-code `/security` command shipped: overview, events with filters, compliance, assessment, and ack/resolve actions via `/api/v1/security/*` (`feat/gizzi-security-dashboard`) | Shipped phase 1 CLI port | `feat/gizzi-security-dashboard` |
| 77 | Purpose Binding (GAP → gizzi-code) | `[x]` | REAL — gizzi-code `/purpose` command shipped: list purposes, bindings, violations, bind/unbind agents via `/api/v1/purposes/*` (`feat/gizzi-purpose-binding`) | Shipped phase 1 CLI port | `feat/gizzi-purpose-binding` |
| 78 | Observability Dashboard (PARTIAL → upgrade) | `[x]` | REAL — added `ObservabilityCharts.tsx` with Recharts area (ledger event volume) and stacked-bar (receipts by kind) time-series panels, integrated into `ObservabilityDashboardSurface` (`feat/web-observability-charts`) | Shipped charts upgrade; alerting deferred | `feat/web-observability-charts` |
| 79 | Multimodal Input (GAP → gizzi-code) | `[x]` | REAL — gizzi-code `/multimodal` command shipped: Ink TUI that connects to `/api/v1/multimodal/ws/multimodal` and reports connection status, message count, and last frame size (camera/audio capture is not applicable in a terminal) (`feat/gizzi-multimodal-input`) | Shipped phase 1 CLI port; full media capture deferred | `feat/gizzi-multimodal-input` |
| 80 | Evolution Layer (GAP → gizzi-code) | `[x]` | DEFER — web `views/EvolutionLayerView.tsx` (58 lines) is purely static mock, zero backend calls; nothing real to port yet | Defer until web has real backend | |
| 81 | Context Control Plane (GAP → gizzi-code) | `[x]` | DEFER — web `views/ContextControlPlaneView.tsx` (361 lines) is hardcoded mock arrays, zero API calls; nothing real to port yet | Defer until web has real backend | |
| 82 | Swarm ADE (PARTIAL → iOS) | `[x]` | REAL — iOS Swarm ADE shipped: new Swarm sidebar tab, `SwarmAgent` models, `SwarmClient` mapping agent-sessions to swarm agents, `SwarmStore`, and `SwarmADEView` with Grid / Console / Detail modes (`feat/ios-swarm-ade`) | Shipped iOS SwarmADE-lite | `feat/ios-swarm-ade` |
| 83 | H5I panel — Audit (GAP → gizzi-code) | `[x]` | REAL — gizzi-code `/h5i audit` command shipped: calls `/api/h5i/status`, `/api/h5i/init`, `/api/h5i/vibe`; reports AI footprint, riskiest files, leaked tokens, prompt injection hits, AI-written directories, and raw output (`feat/gizzi-h5i-audit`) | Shipped phase 1 CLI port | `feat/gizzi-h5i-audit` |
| 84 | H5I panel — Commit (PARTIAL → upgrade) | `[x]` | REAL — upgraded `H5iCommitPanel.tsx`: added Files to include picker, auto-load diff preview from `/api/v1/git/diff`, per-file diff display, and pass selected files to `commitWithH5i` (`feat/web-h5i-commit-upgrade`) | Shipped file-picker/diff-preview upgrade | `feat/web-h5i-commit-upgrade` |
| 85 | H5I panel — Context (GAP → gizzi-code) | `[x]` | REAL — gizzi-code `/h5i context` shipped: show trace via `/api/h5i/context/trace`, start via `/api/h5i/context/start`, finish via `/api/h5i/context/finish`; extends the same `/h5i` command as audit (`feat/gizzi-h5i-context`) | Shipped phase 1 CLI port | `feat/gizzi-h5i-context` |
| 86 | H5I panel — Diff (PARTIAL → upgrade) | `[x]` | REAL — upgraded `H5iDiffPanel.tsx`: renamed header to 'h5i Diff', added Reasoning Traces / File Diff tabs, file-diff mode calls `/api/v1/git/diff` with optional path and renders per-file diffs (`feat/web-h5i-diff-upgrade`) | Shipped file-diff mode + clearer naming | `feat/web-h5i-diff-upgrade` |
| 87 | Changeset Review (GAP → iOS) | `[x]` | PR #4 merged to main | Closed as shipped | #4 |

## Marketplace/Plugins (4)

| # | Item | Status | Finding | Action | PR |
|---|------|--------|---------|--------|-----|
| 88 | Marketplace (top-level) (PARTIAL → iOS) | `[x]` | DEFER — re-verified against live code: `views/marketplace/main/useMarketplaceManager.ts` is hardcoded mock `INITIAL_ITEMS` with no backend; nothing real to port yet | Defer until web has real marketplace backend | |
| 89 | Plugin Registry / Plugin Marketplace (PARTIAL → iOS) | `[x]` | REAL — iOS Plugin Marketplace shipped: new Plugins sidebar tab, `MarketplacePlugin` model with bundled catalog, `PluginStore` for enabled-state persistence, and `PluginMarketplaceView` with category filters/search/enable toggle (`feat/ios-plugin-marketplace`) | Shipped phase 1 iOS port | `feat/ios-plugin-marketplace` |
| 90 | Team Skills panel (GAP → iOS) | `[x]` | REAL — iOS Team Skills shipped: new sidebar tab, `WorkspaceClient`/`WorkspaceStore`, `TeamSkill` model, `TeamSkillsClient`/`TeamSkillsStore`, and `TeamSkillsView` wired to `/api/v1/team-skills` | Shipped phase 1 iOS port | `feat/ios-team-skills` |
| 91 | MiroFish simulation engine (GAP → iOS, gizzi-code) | `[x]` | DEFER — web fully built `lib/mirofish/*`, real gap on iOS/gizzi-code but large scope | Defer to later phase | |

## Products/Discovery (5)

| # | Item | Status | Finding | Action | PR |
|---|------|--------|---------|--------|-----|
| 92 | Products Discovery (GAP → iOS, gizzi-code) | `[x]` | REAL — iOS Products tab + gizzi-code `products` command shipped; bundled catalog mirrors web's ProductsDiscoveryView | Shipped phase 1 port | `feat/products-discovery` |
| 93 | A://Labs (GAP → iOS, gizzi-code) | `[x]` | REAL — iOS A://Labs tab shipped: `ALABSCourse`/`ALABSLesson` models, `LabsClient`/`LabsStore`, `LabsView` with Discovery/Tracks/Classroom tabs; gizzi-code `labs` command lists courses/lessons | Shipped phase 1 port | `feat/labs` |
| 94 | Udemy Catalog (GAP → iOS, gizzi-code) | `[x]` | REAL — backend `udemy_routes.rs` proxy shipped, iOS Udemy Catalog tab with categories/search/curate, and gizzi-code `udemy` command | Shipped phase 1 port | `feat/udemy-catalog` |
| 95 | Discovery Feed (GAP → iOS, gizzi-code) | `[x]` | STALE — not standalone; nested sub-tab inside `LabsView.tsx`, not its own route | Fold into #93 A://Labs | |
| 96 | Research tab/panel (PARTIAL → iOS) | `[x]` | REAL — iOS Research tab shipped: `ResearchNotebook`/`ResearchSource`/`ResearchChatMessage` models, `ResearchClient`/`ResearchStore`, `ResearchView` with notebook list/sources/chat, wired into `ModeBarItem.research` | Shipped phase 1 iOS port | `feat/ios-research-tab` |

## Mail/Knowledge (3)

| # | Item | Status | Finding | Action | PR |
|---|------|--------|---------|--------|-----|
| 97 | Mail Monitor (GAP → iOS) | `[x]` | STALE — `views/mail-monitor/` only holds shared helpers now; the panel was rebuilt as Agent Activity (`views/agent-activity/*`) and shipped on iOS in PR #18 | Close as superseded | #18 |
| 98 | Documents (office-file I/O) (GAP → iOS) | `[x]` | REAL — iOS Documents tab shipped: import via document picker, list, QuickLook preview, rename/share/delete; editing (docx/xlsx/pptx mutation) deferred to a later phase | Shipped phase 1 iOS port | `feat/ios-documents` |
| 99 | Knowledge (PARTIAL → iOS) | `[x]` | DEFER — no `knowledge/` dir; closest analog `views/MemoryKernelView.tsx`, needs product scoping | Defer pending scoping | |

## Onboarding/Account (10)

| # | Item | Status | Finding | Action | PR |
|---|------|--------|---------|--------|-----|
| 100 | Settings (umbrella/shell) (PARTIAL → upgrade) | `[x]` | STALE — `views/settings/SettingsView.tsx` (1510 lines) is mature, fully wired | Re-scope or close | |
| 101 | Settings > Account (PARTIAL → gizzi-code) | `[x]` | DEFER — pieces exist both sides, but no true settings sync; needs an ADR | Defer, needs architecture decision | |
| 102 | Settings > Platform (PARTIAL → iOS) | `[x]` | REAL — iOS Settings > Platform shipped: `PlatformSettingsView` with General, Appearance, Models, API Keys, Shortcuts, Permissions, Dispatch, Devices, Cloud Instances, and Diagnostics sections | Shipped phase 1 iOS parity | `feat/ios-settings-platform` |
| 103 | Settings > Products (GAP → iOS) | `[x]` | REAL — iOS Settings > Products shipped: `ProductsSettingsView` with Gizzi Code toggles, Cowork dispatch/open workspace, and Extensions toggles | Shipped phase 1 iOS parity | `feat/ios-settings-products` |
| 104 | Settings > Infrastructure (GAP → iOS, gizzi-code) | `[x]` | REAL (iOS) — iOS Settings > Infrastructure shipped: `InfrastructureSettingsView` with environment selector, mesh controls, cloud instances, BYOC/VPS/Security placeholders, and agent hub link | Shipped phase 1 iOS parity | `feat/ios-settings-infrastructure` |
| 105 | Settings > Customize (PARTIAL → upgrade) | `[x]` | STALE — Skills/Response-style/Connectors/Plugins panels all implemented and wired | Close or downgrade to cosmetic-only | |
| 106 | Device Pairing panel (GAP → iOS) | `[x]` | PR #5 merged to main | Closed as shipped | #5 |
| 107 | Organization Access panel (GAP → iOS, gizzi-code) | `[x]` | REAL — iOS `OrganizationAccessView` shipped (profile + personal-org creation) and gizzi-code `org` command shipped (`show` + `create`) | Shipped iOS + gizzi parity | `feat/ios-organization-access` |
| 108 | Compute Billing panel (GAP → iOS) | `[x]` | REAL — iOS `ComputeBillingView` shipped: plan badge, weekly usage meter, credits, and upgrade/buy-credits actions | Shipped phase 1 iOS parity | `feat/ios-compute-billing` |
| 109 | Enterprise BYOC panel (GAP → iOS, gizzi-code) | `[x]` | DUPLICATE of row 70 — same `EnterpriseBYOCPanelView` shipped in `feat/ios-enterprise-byoc`; no additional work | Closed as duplicate of #70 | `feat/ios-enterprise-byoc` |
| 110 | Model Management view (PARTIAL → upgrade) | `[x]` | REAL — iOS `ModelManagementView` shipped: default model picker + provider/engine list with auth/ready status via `/api/v1/providers*`, `/api/v1/provider/ollama/status` | Shipped phase 1 iOS parity | `feat/ios-model-management` |

## AllternitOS (1)

| # | Item | Status | Finding | Action | PR |
|---|------|--------|---------|--------|-----|
| 111 | AllternitOS (GAP → gizzi-code) | `[x]` | REAL — gizzi-code `programs` command shipped: lists built-in command programs + installed agents, shows info, and provides a `run` shim mapping AllternitOS installable programs to CLI agents/skills | Shipped minimal equivalent | `feat/ios-model-management` |

## Playground/QA (1)

| # | Item | Status | Finding | Action | PR |
|---|------|--------|---------|--------|-----|
| 112 | Playground (PARTIAL → iOS) | `[x]` | DEFER — web has full playground, zero matches iOS, but gizzi-code interactive session is a de facto substitute | Real but low value, defer indefinitely | |

## Empty stubs (1)

| # | Item | Status | Finding | Action | PR |
|---|------|--------|---------|--------|-----|
| 113 | `views/gizzi`, `components/mesh`, `lib/mesh-network` (PARTIAL → upgrade) | `[x]` | STALE (mischaracterized) — paths don't exist; git shows ~4,750 lines deleted 2026-07-02 (commit 1a1d9900a), not an empty stub | Confirm if deletion was intentional before restoring | |

## Desktop-only (2)

| # | Item | Status | Finding | Action | PR |
|---|------|--------|---------|--------|-----|
| 114 | Local runtime discovery (PARTIAL → upgrade) | `[x]` | STALE — gizzi-code already has `cli/commands/runtime.ts` equivalent; consolidation opportunity, not a new build | Evaluate shared protocol, no net-new build | |
| 115 | Local Python execution (PARTIAL → upgrade) | `[x]` | STALE — gizzi-code's general tool execution already covers Python; no dedicated desktop file needed | No build needed | |

## gizzi-code-only (5)

| # | Item | Status | Finding | Action | PR |
|---|------|--------|---------|--------|-----|
| 116 | `github ...` GitHub Actions agent bot (GAP → iOS, gizzi-code) | `[x]` | STALE (gizzi-code) — fully implemented `cli/commands/github.ts` (~1630 lines), registered in `cli/main.ts` | Close gizzi-code half; iOS parity is separate | |
| 117 | Local VM management (GAP → gizzi-code) | `[x]` | STALE — `runtime/vm/` (vfkit-based) + `cli/commands/vm.ts` fully built, used in production | Close as shipped | |
| 118 | Teleport / remote dev environments (GAP → gizzi-code) | `[x]` | STALE (gizzi-code) — extensive `remote/RemoteSessionManager.ts` + Teleport UI/hooks already exist | Close gizzi-code half; web/iOS parity is separate DEFER | |
| 119 | Slack app install (GAP → gizzi-code) | `[x]` | STALE — working `install-slack-app` command registered in two command tables | Close as shipped | |
| 120 | Theme switching (`/theme`) (PARTIAL → upgrade) | `[x]` | REAL — custom theme/palette upgrade shipped: arbitrary named themes in `~/.config/gizzi/themes.json`, `theme custom`, `theme palette`, `theme delete`; built-ins remain immutable | Shipped custom theme upgrade | `feat/gizzi-theme-upgrade` |
