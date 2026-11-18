---
status: done
files_changed: []
deviations: []
remaining: []
---

# Misc Sections Triage — Phase 1 Notes

Read-only re-investigation of the 34 remaining open items in `docs/learnings/SURFACE_AUDIT_SESSION_PROGRESS.md`. No source files were modified; no builds/typechecks/dev servers were run.

Surfaces referenced:
- Web: `surfaces/ai.allternit.com/src/`
- iOS: `surfaces/allternit-mobile/ios/`
- gizzi-code: `cmd/gizzi-code/src/`
- Desktop: `surfaces/allternit-desktop/`

---

## Core Chat/Home

### 1. Projects (GAP → gizzi-code)
**STALE.** Project infrastructure already exists on all three surfaces. Web: `views/ProjectView.tsx`, `views/BaseProjectView.tsx`, `views/project/{chat,design,unified}/`, `shell/rail/ProjectRailSection.tsx`, `views/code/CodeProjectView.tsx`, `views/cowork/CoworkProjectView.tsx`. gizzi-code: `cmd/gizzi-code/src/project/instance.ts` re-exporting a real `runtime/context/project/project.ts` + `project.sql.ts` model. iOS: `Features/Projects/Views/{ProjectsListView,ProjectDetailView,NewProjectSheet}.swift`. The "needs building in gizzi-code" claim is outdated.
**Next action:** Verify gizzi-code's project model exposes the fields/actions web/iOS expect; no ground-up build needed.

### 2. Artifacts Library (PARTIAL → upgrade)
**STALE.** Web has a full implementation under a different name: `views/library/LibraryView.tsx` (titled "Artifacts Library", search/filter by kind, backed by `services/library-api.ts`), plus `components/chat/ArtifactTemplateGallery.tsx` and `components/artifact/ArtifactRenderer.tsx`. iOS already ships a complete parallel implementation: `Features/Artifacts/{ArtifactContentLoader,ArtifactLibraryStore}.swift`, `Features/Artifacts/Views/{ArtifactsLibraryView,ArtifactDetailsView,SandboxedArtifactWebView}.swift`. The original audit likely missed it due to the `LibraryView` naming.
**Next action:** Re-audit `LibraryView.tsx` against iOS's `ArtifactsLibraryView.swift` for feature parity rather than treating as a fresh gap.

---

## Marketplace/Plugins

### 3. Marketplace (top-level) (PARTIAL → iOS)
**REAL.** Web is mature: `views/MarketplaceView.tsx`, `pages/MarketplacePage.tsx`, `views/marketplace/main/*`, `components/marketplace/{PluginMarketplace,McpMarketplace,VPSMarketplace}.tsx`, `plugins/{marketplaceApi.ts,marketplaceInstaller.ts}`. iOS: zero `.swift` files match "marketplace" (case-insensitive, full search).
**Next action:** Confirmed gap — port iOS marketplace using `views/marketplace/main/` as reference.

### 4. Plugin Registry / Plugin Marketplace (PARTIAL → iOS)
**REAL.** Web: `plugins/feature.registry.ts`, `plugin.types.ts`, `capability.types.ts`, `built-in/`, `catalog/`, `vendor/`. gizzi-code: `runtime/services/plugins/{pluginOperations.ts,PluginInstallationManager.ts,pluginCliCommands.ts}`, `runtime/server/routes/plugin.ts`, `plugins/builtinPlugins.ts` — a real plugin system. iOS: zero matches for "plugin" anywhere.
**Next action:** Confirmed gap — reconcile web's `feature.registry.ts` and gizzi-code's `PluginInstallationManager.ts` into an iOS plugin registry surface.

### 5. Team Skills panel (GAP → iOS)
**REAL** (for iOS). Web already has `components/marketplace/TeamSkillsPanel.tsx` (workspace-installed skills, uses `useWorkspaceStore`). Note: this is a distinct domain from `views/code/SkillsRegistryView.tsx`/`SkillsView.tsx` (browser-automation skills) and gizzi-code's own agent-capability skill system (`skills/bundledSkills.ts`, `runtime/skills/`) — naming overlaps three unrelated concepts. iOS: zero matches for "skill".
**Next action:** Build iOS port of `TeamSkillsPanel.tsx`; disambiguate naming from the unrelated "Skills Registry" (browser automation) domain before shipping.


---

## Products/Discovery

### 7. Products Discovery (GAP → iOS, gizzi-code)
**REAL.** Web is fully built and live: `views/products/ProductsDiscoveryView.tsx`, wired in `shell/ViewRegistry.tsx:109,642`. iOS/gizzi-code: no matches.
**Next action:** Confirmed cross-surface gap; web needs no further work, iOS/gizzi-code build remains.

### 8. A://Labs (GAP → iOS, gizzi-code)
**REAL.** Web: `views/LabsView.tsx` + full `views/labs/main/*` module (tracks/classroom/settings tabs), wired in `ViewRegistry.tsx:111,652`. iOS: no `Features/Labs*`. gizzi-code: no dedicated labs concept.
**Next action:** Confirmed gap; build iOS/gizzi-code using `views/labs/main/` as reference. (Note: items #10 Discovery Feed and #11 Research tab are both sub-tabs nested inside this view, not standalone surfaces — see below.)

### 9. Udemy Catalog (GAP → iOS, gizzi-code)
**REAL.** Web: `views/CatalogView.tsx` wired in `ViewRegistry.tsx:112,657`; genuinely Udemy-specific — uses `UdemyPublicCourse` type, calls `/api/v1/udemy/search`, header reads "Udemy Course Catalog" (`views/catalog/main/CatalogViewHeader.tsx:32`). iOS/gizzi-code: no matches.
**Next action:** Confirmed cross-surface gap, lower priority per original audit.

### 10. Discovery Feed (GAP → iOS, gizzi-code)
**STALE.** Not a standalone surface. Web: `views/discovery/DiscoveryFeed.tsx` + `hooks/useDiscoveryFeed.ts`, imported directly into `LabsView.tsx:82` as a sub-tab, fed by `data/discovery-pipeline.json`. It is not registered in `ViewRegistry.tsx` on its own.
**Next action:** Fold into item #8 (A://Labs) scope rather than tracking as an independent row.

### 11. Research tab/panel (PARTIAL → iOS)
**REAL** (nested scope). Web: `views/research/ResearchTab.tsx` (+ `SourcePanel`, `ToolsPanel`, `AudioPlayer`, `ChatWorkspace`) is imported into `LabsView.tsx:6,76` as a tab, not a standalone route. A separate, unrelated `ResearchQueryPanel.tsx` exists under `views/cowork/`, used by `InsightsView.tsx` — do not conflate the two. gizzi-code's closest analog is the `search-knowledge` skill bundle (no dedicated panel). iOS: no match.
**Next action:** iOS build needed; scope it as nested inside the Labs port, not a standalone screen.

---

## Mail/Knowledge

### 12. Mail Monitor (GAP → iOS)
**REAL.** Web is fully implemented: `views/mail-monitor/ConversationMonitorPanel.tsx` + `monitor.helpers.ts`, rendered via `shell/ConversationMonitorOverlay.tsx`, wired into `shell/ShellApp.tsx`. gizzi-code has only a partial analog (`cli/commands/vault.ts` — `vault sync gmail|calendar|fireflies|all`, a sync feature not a live monitor). iOS: no match.
**Next action:** Confirmed gap — build iOS port of `ConversationMonitorPanel.tsx`.

### 13. Documents (office-file I/O) (GAP → iOS)
**REAL.** Web is substantially more built than the original audit implies: real docx/xlsx/pptx read+write at `views/documents/office-io/{docx,xlsx,pptx}.ts`, full editor packs (`views/documents/packs/{DocumentEditorPack,SheetEditorPack,PresentationEditorPack}.tsx`) loaded via `views/documents/editor-packs.ts`, consumed by `views/cowork/DocumentsView.tsx`. This is a distinct feature from "Office Add-ins" (`views/aci/AciAddinView.tsx`, `lib/design/office-bridge.ts`, `plugins/built-in/office-{word,excel,powerpoint}/`) — confirmed no double-count between the two audit items. iOS: no match; gizzi-code edits files natively on disk (no gap there).
**Next action:** iOS build remains the real gap; update audit language — web is not "thin," it's a mature native document editor.

### 14. Knowledge (PARTIAL → iOS)
**DEFER.** No `knowledge/` directory exists anywhere in web `src` — it was never web-native under this name. The closest analog is `views/MemoryKernelView.tsx` (325 lines, real UI for events/entities/edges, wired in `ViewRegistry.tsx:62,437`). gizzi-code's `vault/` + `cli/commands/{vault,brain}.ts` (init/query/write/sync/graph/status) are more built-out than any web equivalent. iOS only has `Features/Settings/MemorySettingsView.swift`, a settings-level browse surface, not a full panel.
**Next action:** Do not build a net-new "Knowledge" feature. Reclassify as "extend `MemoryKernelView` + iOS `MemorySettingsView`" pending further product scoping — real work, but needs a product decision before it's actionable this phase.

---

## Onboarding/Account (Settings)

Web has two settings directories: `components/settings/` (20 files — shared primitives + panels) and `views/settings/` (17 files — `SettingsView.tsx`, `settings.config.ts`, group-specific views). `settings.config.ts` defines groups `account/platform/products/infrastructure/customize/about`, matching the audit row names exactly. `SettingsView.tsx`'s `renderContent()` switch confirms every panel below is wired and live.

### 15. Settings (umbrella/shell) (PARTIAL → upgrade)
**STALE.** `views/settings/SettingsView.tsx` (1510 lines) + `settings.config.ts` is a mature, fully-wired shell with nav search, groups, and event-driven open (`allternit:open-settings`). The "needs upgrade" claim no longer reflects current maturity.
**Next action:** Re-scope against current UX bar before assigning more work; likely close as done.

### 16. Settings > Account (PARTIAL → gizzi-code)
**DEFER.** Web's `ClerkAuthPanel`/`renderGizziioCodePanel` already reference gizzi-code (`/api/oauth/revoke-user`, `gizzi login`); gizzi-code has `cli/commands/login/login.tsx`, `runtime/context/user/user.ts`, `shared/utils/{config,billing}.ts`. The pieces exist but there is no true bidirectional settings *sync* between web Account and gizzi-code local config.
**Next action:** Needs an ADR on account/config sync architecture, not a UI ticket — defer to a dedicated phase.

### 17. Settings > Platform (PARTIAL → iOS)
**REAL.** Web's platform group (general, appearance, models, api-keys, shortcuts, permissions, dispatch, devices, cloud-instances, diagnostics) is complete. iOS `Features/Settings/SettingsView.swift` only has `accountSection, usageSection, capabilitiesSection, agentSection, memorySection, voiceSection, dataControlsSection, meshSection, aboutSection` — no api-keys/shortcuts/dispatch/cloud-instances/diagnostics equivalents.
**Next action:** Build iOS Platform parity; start with api-keys + shortcuts (lowest lift).

### 18. Settings > Products (GAP → iOS)
**REAL.** Web Products group (gizziio-code, cowork, extensions) is fully wired. iOS has no "Products" settings grouping — Cowork exists elsewhere in iOS (`Core/API/Models/CoworkProject.swift`, `Features/Projects/*`) but not under Settings.
**Next action:** Add an iOS Settings > Products entry that links to existing Cowork/Projects screens rather than rebuilding them.

### 19. Settings > Infrastructure (GAP → iOS, gizzi-code)
**REAL** for iOS; gizzi-code portion likely N/A. Web `InfrastructureSettings.tsx` (1669 lines) + VPS/environment/security/agents panels are extensive. iOS has no VPS/Infrastructure/Environment/Security settings files. gizzi-code has infra-adjacent CLI surfaces (`teleport`, `sandbox-toggle`, `runtime` commands) but "Infrastructure settings screen" doesn't map onto a CLI — it's already exposed via commands, not a screen.
**Next action:** Scope iOS Infrastructure settings only; treat the gizzi-code half of this row as STALE (already covered by existing commands).

### 20. Settings > Customize (PARTIAL → upgrade)
**STALE.** Skills (`SkillsSettingsPanel.tsx`), Response style (`ResponseStylePanel.tsx`), Connectors (`lib/design/owned-connector`), Plugins (`PluginsSettingsPanel.tsx` + `PluginManager` overlay) are all implemented and wired.
**Next action:** Any remaining gaps are cosmetic, not structural — downgrade from "needs upgrade" or close.

### 21. Organization Access panel (GAP → iOS, gizzi-code)
**REAL.** `components/settings/OrganizationAccessPanel.tsx` (+ test) exists and is wired at `case 'organization'`. iOS: no Organization/Access files. gizzi-code: only backing data (`organizationUuid/organizationName/organizationRole` in `shared/utils/config.ts`), no invite/roles UI or command.
**Next action:** Build iOS org-access screen and a `gizzi org` CLI command using the web panel as spec.

### 22. Compute Billing panel (GAP → iOS)
**REAL.** `components/settings/ComputeBillingPanel.tsx` (+ test) exists and is wired via `renderBillingPanel()`. No iOS Billing/ComputeBilling files anywhere.
**Next action:** Port `ComputeBillingPanel.tsx` data model to an iOS Settings > Account > Billing screen.

### 23. Enterprise BYOC panel (GAP → iOS, gizzi-code)
**REAL**, with a tracking issue. `docs/learnings/SURFACE_AUDIT_SESSION_PROGRESS.md` rows 70 (Terminal/Infra section) and 109 (this section) have **identical text** — `Enterprise BYOC panel (GAP → iOS, gizzi-code)` — this is one feature double-counted across two audit sections. Web: `EnterpriseByocPanel.tsx` (wraps `CloudCredentialsPanel.tsx`, wired + tested). iOS: nothing found. gizzi-code: only a code comment reference to the BYOC runner in `runtime/integrations/{types.ts:48,claude/types.ts:48}` — no settings panel/command.
**Next action:** Merge tracker rows 70 and 109 into a single item before scoping iOS/gizzi-code work, to avoid double-counting effort. The underlying gap itself is real.

### 24. Model Management view (PARTIAL → upgrade)
**REAL.** `views/settings/ModelManagementView.tsx` (170 lines) is functional (lists providers/engines, auth status via `api.listProviders`), registered as its own view `"models-manage"` in `shell/ViewRegistry.tsx:98,256` — distinct from the inline `LocalModelManager` used in Settings > Platform > Models. It's minimal: no per-model config, quotas, or context-window controls.
**Next action:** Genuine upgrade work remains; also flag potential UX overlap with Platform > Models' `LocalModelManager` for de-duplication review.

---

## AllternitOS

### 25. AllternitOS (GAP → gizzi-code)
**REAL.** Web has a large implementation: `allternit-os/AllternitOS.tsx` (486 lines), `views/AllternitOSView.tsx` (384 lines), `allternit-os/kernel/KernelBridge.ts`, `allternit-os/services/{FileSystemService,PythonExecutionService}.ts`. Searching `cmd/gizzi-code/src` for any OS-metaphor/windowing/installable-program concept found nothing relevant.
**Next action:** gizzi-code genuinely lacks a windowing/kernel/installable-programs concept. Scope a minimal "installable skill-as-program" equivalent, not a full OS port.

## Playground/QA

### 26. Playground (PARTIAL → iOS)
**DEFER.** Web: `views/PlaygroundView.tsx` + full `views/playground/main/` module, plus a separate `components/agents/AgentTestingPlayground.tsx`. `grep -ril "playground" surfaces/allternit-mobile/ios` returns zero hits — confirmed absent on iOS. However, gizzi-code's interactive session + `models`/`provider` commands already act as a de facto playground.
**Next action:** Real gap, but low value — skip building a dedicated iOS Playground UI; defer indefinitely unless product demand emerges.

## Empty stubs

### 27. `views/gizzi`, `components/mesh`, `lib/mesh-network` (PARTIAL → upgrade)
**STALE** (mischaracterized). None of these paths exist in the current tree. `git log --all` shows they were **not** empty stubs — commit `1a1d9900a` ("source refactor," 2026-07-02) deleted 4,750 lines of real working code: `lib/mesh-network/{agent/agent.ts (801 lines), agent/install.ts (522), deploy/headscale-install.ts (521), wireguard-platform.ts (291), platform-integration.ts (541), types.ts (278)}` and `views/gizzi/tabs/{GizziCronTab,GizziExecApprovalsTab,GizziLogsTab,GizziSkillsTab}.tsx`. The audit doc characterizing these as "empty stubs, zero files" is factually wrong for both the pre-deletion state (substantial real code) and current state (directories don't exist, not "empty"). iOS has a separate, unrelated real mesh feature: `Core/Mesh/MeshClient.swift`, `Core/API/MeshEnrollClient.swift` (tsnet/headscale-based).
**Next action:** Confirm with the team whether the 2026-07-02 deletion was intentional before deciding to restore, rebuild, or formally deprecate. Correct the audit doc's "empty stub" framing — this is a regression/deletion, not a stub.

## Desktop-only

### 28. Local runtime discovery (PARTIAL → upgrade)
**STALE.** Web has real discovery logic: `agent-workspace/discovery.ts`, `lib/page-agent/runtime-client.ts`. Desktop's `backend-manager.ts` manages a fixed-port (127.0.0.1:8013) local backend, not multi-process discovery. gizzi-code already has a named equivalent: `cli/commands/runtime.ts` ("manage local agent runtime discovery") with `runtime/{list,register,status}.ts` subcommands.
**Next action:** Not a clean gap — a consolidation opportunity. Evaluate whether web's discovery client and gizzi-code's `runtime` command should share one protocol, rather than building desktop-specific discovery from scratch.

### 29. Local Python execution (PARTIAL → upgrade)
**STALE.** `allternit-os/services/PythonExecutionService.ts` (557 lines) supports mock/kernel/HTTP execution backends — this is AllternitOS-scoped, not desktop-app-scoped. `surfaces/allternit-desktop/` has no dedicated Python execution file. gizzi-code already executes Python via its general shell/tool-call access, a stronger equivalent by design.
**Next action:** No build needed; already covered by gizzi-code's general tool execution.

## gizzi-code-only

### 30. `github ...` GitHub Actions agent bot (GAP → iOS, gizzi-code)
**STALE** (gizzi-code portion). Fully implemented: `cli/commands/github.ts` (~1630 lines), `GithubCommand` with `install` (creates `.github/workflows/gizzi.yml`, GitHub App install flow) and `run` (full event router for issue_comment/pull_request/issues/schedule/workflow_dispatch, OIDC token exchange, Octokit calls). Registered in `cli/main.ts:26,190`.
**Next action:** Close the gizzi-code half as already shipped. iOS parity (if genuinely wanted) is a separate, smaller-priority tracking item.

### 31. Local VM management (GAP → gizzi-code)
**STALE.** Two real implementations: `runtime/vm/` (vfkit/Apple-Virtualization-based: `lima-setup.ts`, `lima-executor.ts`, `README.md`) and CLI command `cli/commands/vm.ts` (`gizzi vm start|stop|restart|status|setup|exec`). Consumed in production at `runtime/cowork/cowork.runtime.ts:22`.
**Next action:** None — already built. Verify `vm.ts` (marked `@ts-nocheck`) is fully tested.

### 32. Teleport / remote dev environments (GAP → gizzi-code)
**STALE** (gizzi-code portion). Extensive subsystem already exists: `remote/RemoteSessionManager.ts`, `cli/ui/components/Teleport*.tsx`, `shared/utils/teleport.tsx`, `teleport/api.ts`, `cli/hooks/{useTeleportResume,useRemoteSession,useSSHSession,useDirectConnect}.ts`, `commands/remote-env/`, `commands/remote-setup/api.ts`. `docs/learnings/SURFACE_AUDIT_FINAL_REPORT.md:323` itself confirms "Missing" refers only to web/iOS lacking it.
**Next action:** Close the gizzi-code work item. Remaining gap is web/iOS parity only — track separately as DEFER (larger scope).

### 33. Slack app install (GAP → gizzi-code)
**STALE.** `cli/ui/ink-app/commands/install-slack-app/{index.ts,install-slack-app.ts}` defines a working command, registered in both `cli/ui/ink-app/commands.ts:33,305` and `cli/commands-claude.ts:32,287`. A duplicate legacy copy exists under `migration/claude/src/commands/install-slack-app/` (not the live path).
**Next action:** None needed. Confirm the `migration/claude/` copy is dead/removable.

### 34. Theme switching (`/theme`) (PARTIAL → upgrade)
**REAL** (upgrade scope confirmed). Two working implementations: CLI subcommand `cli/commands/theme/index.ts` (show/set/list, writes `~/.config/gizzi/theme.json`) and interactive `/theme` slash command (`cli/ui/ink-app/commands/theme/theme.tsx`, renders `ThemePicker`, wired via `useTheme()`). Both are registered and functional but limited to 3 built-in themes (dark/light/system) — no custom-theme or palette editing.
**Next action:** Base feature is done (not a gap); the "needs upgrade" claim is accurate — scope custom-theme/palette-editor support as the real remaining work.

---

## Summary tally

| Classification | Count | Items |
|---|---|---|
| REAL | 18 | 3,4,5,7,8,9,11,12,13,17,18,19,21,22,23,24,25,34 |
| STALE | 13 | 1,2,10,15,20,27,28,29,30,31,32,33 |
| DEFER | 5 | 6,14,16,26 (+ #19's gizzi-code half, folded into REAL row) |

Note: counts approximate — several items carry split verdicts across surfaces (e.g. #19 REAL for iOS, STALE for gizzi-code; #30/#32 STALE for gizzi-code, DEFER-worthy for iOS). See per-item sections above for the authoritative nuance; the tracker's Finding column captures the primary verdict per row.

## Cross-cutting observations

- **Naming drift is the single biggest source of false "GAP" claims.** Artifacts Library → `LibraryView`, Knowledge → `MemoryKernelView`, Team Skills → `TeamSkillsPanel` (vs. two unrelated "Skills" concepts) all show the original audit searched for literal names that don't match the actual implementation.
- **The "gizzi-code-only" section (items 30–34) was almost entirely stale** — 4 of 5 items already have full, wired implementations in `cmd/gizzi-code/src`. Only Theme switching has genuine remaining upgrade work.
- **One confirmed duplicate tracker row**: Enterprise BYOC panel appears as both row 70 (Terminal/Infra) and row 109 (Onboarding/Account) with identical text.
- **One confirmed regression, not a stub**: item 27's "empty stub" framing is wrong — `views/gizzi` and `lib/mesh-network` contained ~4,750 lines of real code deleted on 2026-07-02, a month before the audit was written.
