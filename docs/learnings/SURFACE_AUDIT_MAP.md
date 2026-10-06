# Exhaustive cross-surface feature audit — ground truth

## Why this exists

Earlier the same day, three research passes independently inventoried
every distinct feature area in three of Allternit's four surfaces. A first
synthesis pass then tried to cross-reference them, but did it as a
judgment-based skim: it picked out ~10 items that "stood out" and
explicitly excluded whole categories (Design Mode, AllternitOS, Swarm ADE,
Connectors, Model Management, Voice, Permissions, Onboarding, the
marketplace-vs-skills question) as "probably fine" **without actually
checking them**. Eoj rejected that as insufficient and asked for a real,
exhaustive, item-by-item pass — every single item classified, nothing
skipped because it "looks like admin tooling."

Read `GIZZI.md` at the repo root first — it's the authoritative framework
for how the four surfaces relate (web+desktop = one codebase;
iOS = fully separate; gizzi-code = dual-role CLI + agent engine).

## The four surfaces

- **Web/Desktop** (`surfaces/ai.allternit.com`, bundled into
  `surfaces/allternit-desktop`) — treat as one surface for this audit.
- **iOS** (`surfaces/allternit-mobile/ios`) — separate native codebase.
- **gizzi-code** (`cmd/gizzi-code`) — CLI/agent harness, dual role.

## Classification rubric (apply this, don't re-derive it)

For every single item in all three raw inventories below:

- **FULL PARITY** — an equivalent genuinely exists on every surface where
  it would plausibly apply.
- **PARTIAL** — exists on more than one surface but meaningfully thinner
  somewhere (e.g. gizzi-code has a full subsystem, web only has a settings
  toggle that references the concept without real functionality).
- **GAP** — exists on exactly one surface, and there's no good reason it
  couldn't/shouldn't exist elsewhere. State which surface(s) it's missing
  from and one sentence on what a user loses by its absence there.
- **INTENTIONALLY SURFACE-SPECIFIC** — a real, defensible reason it's
  one-surface-only. State the reason explicitly (native camera access,
  inherently visual canvas work, raw local SQL shell, CLI self-update,
  etc.) — don't just assert "probably fine," give the actual reason.

Every item gets one of these four labels. None get skipped. If you
genuinely cannot tell from the inventory description alone, say so
explicitly as its own status ("UNCLEAR — needs code-level check") rather
than guessing — that's a legitimate, honest outcome, silent omission is
not.

## RAW INVENTORY 1 — Web/Desktop (`surfaces/ai.allternit.com`)

(Verbatim from the research agent's report earlier this session — do not
re-summarize this, it is source data.)

---

# Feature Inventory — surfaces/ai.allternit.com (Web/Electron SPA)

Format: `Feature — description — size`. Grouped loosely. Desktop-only items flagged explicitly.

## Core Chat / Home
- Chat (Home) — Primary conversational AI interface, the default landing surface; session-based threads, agent-mode toggle — large (views/chat, 38 files + composer/session stores)
- Chat-legacy route — Fallback alias to the same chat view for old links — trivial
- Agent Hub — Landing gallery to browse/launch available agents (canonical agent modes) — medium
- Projects — Grouping of chats/sessions/artifacts into named projects with own recents — medium (views/ProjectView.tsx, 731 lines)
- Artifacts Library — Central browsable library of generated artifacts (docs, code, media) across sessions — medium
- Automation Tasks (Goals/Routines/Loops/Cron) — Unified UI for scheduled/recurring agent work: one-off goals, recurring routines, continuous loops, cron-style automation — large (views/automation + views/cowork/AutomationTasksView, cross-linked from several nav entries)
- Dispatch — QR-code based "hand off a task to your phone/another device" flow with permission scoping (files, keep-awake, Chrome access) — medium, marked Beta in UI (views/DispatchView.tsx, 882 lines)
- History / Archived / Recents / Search — Cross-mode session browsing utilities — small each
- Customize (Shellrail) — Toggle which nav-rail tabs are visible per mode (Home/Code/Browser) — small settings panel

## Cowork (multi-agent team workspace)
- Cowork workspace (CoworkRoot) — Full "AI teammate" workspace: launch tasks, chat/transcript, run timeline — large (47 files)
- Cowork Runs / Drafts / Tasks / Cron / Project / Documents / Tables / Files / Exports views — Sub-views of Cowork for viewing task runs, draft outputs, task lists, per-project workspace, generated docs/spreadsheets/files, and exporting results — medium (each is its own registered view)
- Cowork Insights / Activity / Goals — Analytics/activity feed and goal-tracking panels within Cowork — small-medium
- Cowork Wiki section viewer, Audit log viewer — Knowledge base viewer and permission/audit trail for cowork actions — small
- Connector Settings panel (Cowork) — Configure external service connectors used by cowork agents — small-medium
- Intelli-Schedule panel — Smart scheduling assistant for cowork tasks — small
- Harness Config panel — Configure the execution harness/sandbox for cowork agent runs — small

## Code (developer/agentic coding workspace)
- Code workspace (CodeRoot) — Full coding IDE-like surface: file editor, diff viewer, terminal-adjacent tooling, canvas preview — large (65 files)
- Code Explorer / Git / Threads(Recents) / Skills / Project views — File tree explorer, git operations UI, session history, code-specific skills list, per-project code workspace — medium each
- Code Canvas / Focus / Preview Pane — Split views for live-previewing code output and focused editing — medium
- Orchestrator Center / Orchestration View / Goal Control Center / Kanban(+DAG) Board — Higher-level multi-step coding task orchestration and visual task boards — medium-large
- Debug View / Logs View / Run Inspector / Run Replay — Debugging and execution-trace tooling for agent coding runs — medium
- Tools Registry (Registry view) — Browse/manage tools available to agents — small-medium
- Skills Registry (Memory nav item) — Browse/install reusable "skills" (labelled "Memory" in the top nav but implemented as SkillsRegistryView) — medium
- Promotion Dashboard, Usage Dashboard — Track code changes promoted to production and token/compute usage per code session — small-medium
- Automation Tasks (Code) — Code-mode specific automation task list — shares component with Cowork's

## ACI / Browser Automation
- ACI Browser surface (BrowserCapsuleEnhanced) — Embedded/automated browser capsule for web-browsing agents; "ACI" tab in nav rail — large (50 files)
- Mini-apps Store — Marketplace/registry of installable "mini-apps" (connector/runtime embeds) — medium
- Mini-app Review Console — Review/approval console for mini-app submissions (permissions diff, signing, lint checks) — medium, appears operator/admin-facing
- Mini-app frame/runtime — Sandboxed runtime surface for running an individual mini-app — medium
- Office Add-ins (Word/Excel/PowerPoint) — Companion connector views for real MS Office integration — medium, 3 registered routes
- Office & Extensions view — Combined view for Office and browser extensions — small-medium
- Browser Extensions manager — Upload/manage browser extensions — small-medium
- Operator Browser — Interface for creating/monitoring autonomous browser-automation tasks ("Operator") — medium
- Hermes — Built-in connector to Nous Research's self-improving agent / messaging gateway (external local runtime, install/start/monitor UI) — small, third-party integration wrapper
- Oh My Pi — Similar install/start/monitor wrapper for an "oh-my-pi" local runtime — small
- OpenClaw — Native UI talking directly to a local OpenClaw agent gateway (explicitly NOT routed through the backend or Gizzi) — medium

## Design / Creative
- Design Mode (DesignModeView, "Hyperdesign") — Full design workspace: canvas, layers, properties panel, handoff, mobile/video/docs tabs, skill graph, pipeline — large (35 files), tabbed sub-views registered individually (questions, mobile, video, docs, handoff, graph, pipeline)
- Design Marketplace/Registry ("design-view-market"/"design-marketplace") — Marketplace of design templates/skills — medium
- Design Compare — Compare design variants — small (shares registry view)
- Form Surfaces — Dynamic schema-driven form builder/preview used for agent-human communication — small-medium
- Canvas Protocol — Declarative "task surface" catalog with hot-reload preview (distinct from chat Canvas/Artifacts) — medium
- Allternit Canvas ("The Computer") — Split-pane artifact viewer alongside chat (documents/slides/sheets/code/media) shown side-by-side with the conversation — large, core artifact-rendering subsystem
- Design Team Workspace, Content Pipeline, HyperFrames Timeline Editor, Live Artifact Editor — Collaborative design sub-tools (team view, content pipeline builder, timeline-based frame editor, live-editable artifact) — medium each

## Terminal / Runtime / Infra Ops
- Terminal — In-app terminal view — small
- Monitor — Live dashboard of running agents (status, tokens, CPU/mem) with pause/resume/restart controls — medium
- Runtime Operations, Budget Dashboard, Replay Manager, Prewarm Manager — Runtime-ops subsystem: view agent runtime health, budget/spend dashboards, session replay, pre-warmed environment pool — medium (views/runtime, 11 files)
- Nodes — Node/cluster management view — small-medium
- Cloud Deploy — Deployment wizard (agent-assisted or manual SSH) for shipping to a cloud provider — medium
- Capsule Manager — Manage MCP "Interactive Capsules" — small-medium
- VPS & Servers panel, Cloud Instances panel, Enterprise BYOC panel — Settings-level infra management: SSH/VPS connections, provisioned cloud instances, bring-your-own-cloud enterprise credentials — small-medium each (components/settings, components/vps)

## Agentic subsystem / advanced internals (DAG suite)
- DAG Integration Page — Umbrella page linking to ~15 "P4 DAG" internal subsystem views — large (18 files)
  - Policy Manager, Policy Gating — Manage/enforce agent action policies
  - Task Executor — Execute DAG-defined tasks
  - Ontology Viewer — Browse the system's knowledge ontology
  - Directive Compiler — Compile high-level directives into executable plans
  - Evaluation Harness — Run agent evaluations
  - GC Agents — Garbage-collection/lifecycle agents view
  - Receipts Viewer — View cryptographic/audit "receipts" of agent actions
  - Security Dashboard — Security posture dashboard
  - Purpose Binding — Bind agent actions to declared purposes (governance)
  - DAG WIH — "Work Item Handling" view for the DAG
  - Checkpointing — View/manage execution checkpoints
  - Observability Dashboard — Telemetry/observability dashboard for agent execution
  - IVKGE Panel, Multimodal Input, UI Forge (tambo) — Knowledge-graph-adjacent panel, multimodal input testing UI, and dynamic UI generation ("UI Forge") tools
  - Each sub-view is small-to-medium individually; collectively a large internal-ops subsystem, likely engineering/debug-facing rather than mainstream end-user
- Hooks System — Event-driven lifecycle automation manager (kernel/workspace/task/human hooks with live execution logs) — medium
- Evolution Layer — Memory/Skill/Workflow "evolution" (self-improvement) engine UI — medium
- Context Control Plane — Git-based context controller (branch/commit/state sync for agent context) — medium
- Memory Kernel — Three-layer agent memory system browser (events, entities, relationship edges) — medium
- Autonomous Code Factory — Self-improving code-generation pipeline UI — medium
- Swarm ADE — Multi-agent "swarm" development environment/dashboard — large (34 files, own README/demos)
- Runner (Agent Runner / DAK) — Lower-level agent execution runner with context-pack browser, DAG planning, lease monitor, receipt query, snapshot manager, template library panels — medium-large (14 files), looks like an internal ops console for the DAG/runner subsystem
- H5I panels (Agent Hooks / Audit / Commit / Context / Diff / MCP) — Set of review panels for agent-driven commits: audit trail, diff view, MCP tool panel, context inspection — small-medium, likely embedded into code/changeset review flow
- Changeset Review — Review UI for agent-proposed file changes (diff cards, approve/reject) — small-medium

## Marketplace / Plugins / Skills
- Marketplace (top-level) — General marketplace for discoverable content/plugins — small-medium
- Plugin Registry / Plugin Marketplace — Install/manage plugins, including a large built-in plugin catalog (Word/Excel/PowerPoint, video, image, code, slides, chrome, flow, data, assets, swarms, mirofish, research, website, claude-desktop vendor plugin) — very large (plugins dir, 480 files) — core extensibility subsystem
- Team Skills panel — Org-level shared skills management — small
- MiroFish simulation engine — Persona-driven multi-round social/world simulation product ("what would people say/do") layered on the swarm sandbox — medium, distinct product feature (15 files, own README)

## Products / Discovery / Learning
- Products Discovery — Browse/discover other Allternit products/offerings — medium-large (1596 lines)
- A://Labs — Experimental features area — medium
- Udemy Catalog — Embedded course catalog/browser (search, categories, curated courses, course detail modal) tied to Udemy — medium, notable as a non-obvious third-party integration
- Discovery Feed — General content discovery feed — small
- Research tab/panel — Research-assistant tooling — small-medium

## Mail / Knowledge
- Mail Monitor — Conversation/telemetry monitor for agent-mediated email/mail threads — small
- Documents — Document surface with office-file I/O (open/edit/save office docs), document workflows/packs — medium (22 files)
- Knowledge, Jobs — Directories exist but are effectively empty stubs — negligible/not implemented

## Onboarding & Account
- Onboarding Flow / Guided Tour — First-run setup wizard with a step-by-step product tour, including an "Infrastructure" setup step — medium
- Settings — Central settings shell with grouped sections: Account, Platform, Products, Infrastructure, Customize, About — large umbrella (settings.config.ts defines ~29 sections)
  - Account: Sign-in/Account, Organization & Access, Usage, Plans & Compute (billing), Privacy
  - Platform: General, Appearance, Models, API Keys, Shortcuts, Permissions, Dispatch, Devices, Cloud Instances, Diagnostics
  - Products: Gizzi Code settings, Cowork settings, Extensions
  - Infrastructure: Infrastructure, VPS & Servers, Enterprise BYOC, Environment, Security, Agents
  - Customize: Skills, Response Style, Connectors, Allternit Plugins
  - About
- Device Pairing panel — Approves `gizzi pair` requests from the CLI/other runtimes and lists paired devices/runtimes; cloud API-backed on web, Electron-preload-backed on desktop — small-medium, directly cross-references the gizzi-code CLI
- Organization Access panel — Manage org membership/access control — small-medium
- Compute Billing panel / Enterprise BYOC panel — Billing and bring-your-own-cloud credential management — small-medium each
- Model Management view — Manage/configure available AI models — medium

## Voice / Local Models
- Voice Service (Speech-to-Text) — Voice input/dictation service — small (4 files)
- Local Models — Router/catalog/provider-registry for running models locally (loopback provider) — medium (56 files), substantial despite being "lib"-only (no dedicated top-level view found; likely surfaced inside Settings > Models)

## AllternitOS ("Computer" meta-environment)
- AllternitOS — A desktop-like "Super-Agent OS" environment embedded as a first-class view, with its own kernel, windowing, and installable "programs" (code-preview, research-doc, citation-manager, asset-manager, presentation, orchestrator, data-grid) — large subsystem (80 files), effectively an OS-in-the-browser sandbox

## Playground / Verification / QA
- Playground — Model-parameter/workbench sandbox for testing prompts and model settings directly — medium
- Verification View — Visual verification tool (likely screenshot/UI diff checking for agent-produced work) — small-medium
- QA — Small internal QA utility directory — small

## Empty / not-yet-implemented stubs found
- views/gizzi (with a `tabs` subfolder), components/mesh, lib/mesh-network — directories exist but contain zero files; appear to be placeholders/removed features, not active functionality. Worth flagging for the cross-surface audit since a "gizzi" and "mesh" view are referenced elsewhere (nav labels "Gizzi Code" in Settings) but have no dedicated implementation file in this surface — likely because Gizzi/mesh functionality lives in the separate gizzi-code CLI and is only surfaced here via Device Pairing / Dispatch, not a standalone view.

## Desktop-only / platform-specific (NOT expected on iOS or gizzi-code CLI)
Files containing explicit Electron/desktop bridge checks (`window.electron`, `window.allternit` preload bridge, `isElectron`, etc.) — flagged as desktop-platform-specific, not gaps:
- `agent-workspace/discovery.ts`, `lib/page-agent/runtime-client.ts` — local runtime discovery (desktop can discover local processes; web talks to cloud instead)
- `plugins/fileSystem.ts`, `allternit-os/services/FileSystemService.ts` — native filesystem access for the plugin system / AllternitOS
- `shell/FloatingWidgets.tsx`, `shell/ControlCenter.tsx` — desktop floating widget/control-center window chrome
- `integration/computer-use-engine.ts` — desktop "computer use" automation engine (controls the actual OS)
- `allternit-os/types/electron.d.ts`, `allternit-os/kernel/KernelBridge.ts` — AllternitOS's Electron IPC bridge/type defs
- `allternit-os/programs/citation-manager/BrowserScreenshotService.ts` — native screenshot capture for citation manager program
- `allternit-os/services/PythonExecutionService.ts` — local Python execution (desktop only)
- `components/onboarding/InfrastructureStep.tsx`, `OnboardingFlow.tsx` — onboarding steps that branch on desktop vs. web bootstrap
- `capsules/browser/BrowserCapsuleEnhanced.tsx`, `useExtensionBridge.ts` — browser capsule's native Chrome-extension bridge (desktop-managed)
- `lib/platform.ts`, `lib/open-code-session-window.ts` — platform-detection helper and native window-opening for code sessions
- `views/settings/SettingsView.tsx` — branches Settings UI for desktop vs. web/self-hosted auth
- `views/aci/MiniAppRuntimeSurface.tsx`, `views/aci/open-office-web.ts` — mini-app/Office runtime surfaces that prefer native rendering on desktop, falling back to web
- Hermes/OpenClaw/Oh-My-Pi views use `window.allternit?.miniApps` (desktop preload bridge) to install/start/monitor local companion runtimes — on web these degrade to "unavailable" states

**Note from the research agent:** the largest genuine subsystems are Chat, Cowork, Code, ACI/Browser Automation, Design Mode, the Plugins/built-in catalog, AllternitOS, Swarm ADE, and the DAG/runtime-ops internal tooling cluster. Settings is a very wide (29-section) but individually shallow umbrella. `gizzi`/`mesh` as literal directories are empty stubs — the actual cross-surface touchpoint with the gizzi-code CLI is the Device Pairing panel (approves `gizzi pair`) plus the Dispatch feature, not a dedicated Gizzi view.

---

## RAW INVENTORY 2 — iOS (`surfaces/allternit-mobile/ios`)

---

## Feature Inventory — `surfaces/allternit-mobile/ios`

### Top-level navigation structure
6 destinations, NOT a persistent bottom tab bar — rows in a slide-out left sidebar (`HistorySidebarView`), switched via `AppModeStore.activeTab` (`Core/AppMode.swift`, `ModeBarItem` enum): **Chats, Projects, Artifacts Library, Agents, Code, ACI**. "Agents" (Agent Hub) is a recent addition. A separate `AppMode` enum (chat/cowork/code/browser) drives per-mode theming/accent color and session tagging — "Cowork" is not its own sidebar destination, it's a composer-level toggle live inside the Chats tab.

### Features/* directories (one per user-facing area)

- **Chat** — `Features/Chat/` (19 files, large) — The core chat/conversation surface: message feed, composer, streaming responses, model picker, agent-selection quick-switcher, attachments (camera/photos/files), usage-limit banners, permission priming, dictation, and full voice mode. This is the "home" tab and by far the biggest feature.
- **Voice Mode** (sub-feature of Chat, `Features/Chat/Views/Voice/` + `VoiceModeViewModel.swift`, 4 files, medium) — Full-screen ambient voice conversation UI (Claude iOS parity): live speech-to-text via `DictationController`, on-device TTS via `SpeechSpeaker`, voice/language/speed/interaction-style settings.
- **Dictation** (sub-feature of Chat, `DictationController.swift` + `DictationOnboardingSheet.swift`, small) — Live mic-to-text transcription for the composer using `SFSpeechRecognizer`/`AVAudioEngine`, with a one-time first-run onboarding sheet.
- **Local Response Notifications** (sub-feature of Chat, `NotificationService.swift`, small) — Opt-in local notification ("Allternit responded") posted when a stream completes while the app is backgrounded. Explicitly NOT true APNs server push (no device-token registration/push pipeline exists) — this is a native-iOS-specific capability with no real cross-surface equivalent expected.
- **Attachments** (sub-feature of Chat, `AttachmentStore.swift` + `ComposerPlusSheet.swift`, small-medium) — Camera/Photos/Files picker staging thumbnails above the composer, uploaded on send.
- **Projects** — `Features/Projects/` (3 files, medium) — Cowork "Projects" tab: list of projects with chat counts/search/filter tabs, and a detail view (name/description, instructions editor, attached files, chats-in-project list). Claude/ChatGPT Projects parity.
- **Artifacts Library** — `Features/Artifacts/` (5 files, medium) — Dedicated tab collecting every artifact seen across chat streams (client-side aggregation, no backend list endpoint), with a details viewer that sandboxes and renders artifact content (incl. an embedded web view for HTML/rich artifacts).
- **Agents (Agent Hub)** — `Features/Agents/` (5 files, medium-large) — Registry of user's configured AI agents (status/model/primary badge), agent detail view, agent creation from templates, and a per-agent workspace file editor (`.md` files) with an AI-assisted "edit with chat" revision flow.
- **Code** — `Features/Code/` (2 files + `Core/API/PtyClient.swift`, medium) — Agentic coding sessions tab (chat-driven, environment selector, session list) plus an interactive terminal (`TerminalSessionView`, via SwiftTerm) that attaches to a real pty on the standalone gizzi-code server — live shell access from the phone.
- **ACI (Allternit Computer Agent / in-app browser)** — `Features/ACI/` (6 files, medium-large) — Combined in-app web browser (URL/search bar, WebKit-based browsing) and a "computer-use" AI agent mode that takes a goal, drives a browser autonomously, and streams a live action trace/viewport. Also hosts browser-origin chat sessions.
- **Connectors** — `Features/Connectors/` (1 file, small-medium) — Full connector browser/manager (OAuth connect/disconnect, API-key prompts) against a live `/api/v1/connectors*` backend covering a 181-entry catalog; reachable from the composer's "+" sheet.
- **History / Sidebar** — `Features/History/` (1 file, but large/central — the unified left-drawer navigation shell) — Dated session history (Today/Yesterday/Previous 7 Days/Older), houses the 6-item tab list, settings entry point, and drives the drawer-drag interaction for the whole app.
- **Onboarding** — `Features/Onboarding/` (2 files, medium) — First-launch 4-page flow: welcome, work-profile persona picker (12 options), persona-ordered starter-task cards, completion. Gates the whole app before first use.
- **Settings** — `Features/Settings/` (3 files, large in scope though few files — dense hub) — Grouped settings hub with sections: Account, Usage (weekly meter), Capabilities, Agent response-style preferences, Memory (see below), Voice, Data Controls, Mesh (see below), About.
- **Memory Settings** (sub-feature of Settings, `MemorySettingsView.swift`, small) — Browse/search the backend long-term-memory documents and stats (`/api/v1/memory/*`).
- **Mesh networking** (sub-feature of Settings, `Core/Mesh/MeshClient.swift` + vendored `Frameworks/Mesh.xcframework`, small file count but a substantial embedded capability) — An embedded tsnet node that joins a private Headscale tailnet for reaching tailnet-only backend services; status/enrollment surfaced in Settings' Mesh section.
- **Authentication** — `Core/Auth/AuthManager.swift` (not under Features/, but a distinct user-facing area) — Sign in/up via Clerk iOS SDK (email + OAuth, incl. Sign in with Apple via associated domains), gating all app access (`LoginGateView`).

### Native-iOS-specific (no expected web/desktop/gizzi-code equivalent)
- **Local response notifications** (`UserNotifications` framework) — opt-in local alert on stream completion; explicitly scoped away from true push (no APNs/device-token/service-extension plumbing exists).
- **Live dictation / Speech-to-text** (`Speech` + `AVFoundation`) — native mic transcription into the composer.
- **On-device voice synthesis (TTS)** for Voice Mode (`AVSpeechSynthesisVoice`/`SpeechSpeaker`).
- **In-app Safari browsing** (`SFSafariViewController`, used for connector OAuth and support/export links) — an iOS system-provided browser sheet, distinct from the ACI in-app WebKit browser.
- **Camera/Photos library access** for attachments (`PhotosUI`, `AVFoundation`).
- **Keychain-backed session persistence** and associated-domains-based OAuth handoff (entitlements-level, Clerk requirement).

Checked for and explicitly **absent**: no widgets (WidgetKit), no Siri Shortcuts/App Intents, no share extension, no biometric auth (Face ID/Touch ID/LocalAuthentication), no true APNs push. The Xcode project has exactly one target (the app itself) — no extension targets at all.

### Possibly incomplete / stub / orphaned
- **Unused accent color asset-catalog entries**: `Assets.xcassets/AccentCowork.colorset` and `AccentBrowser.colorset` (and also `AccentCode.colorset`) are present in the asset catalog but never referenced by name in code — `Core/DesignSystem/Color+Theme.swift` defines the actual `accentCowork`/`accentCode`/`accentBrowser` colors as hardcoded hex literals instead of reading from the catalog. Only `AccentChat` is wired through the catalog.
- **Connectors feature** self-documents its own incompleteness: the view's header comment states "No attachments, no tool-access toggles: those have no real backend behind them... and aren't built here," and it renders honest "not mapped yet" states for most of its 181-entry catalog — i.e., a real but partially-backed feature.
- **ProjectDetailView** explicitly notes the backend has no files routes yet and doesn't persist project id on session create against "the old live backend," so its files list and chats section render empty/error states until a newer backend is deployed.
- **History directory** contains only one file (`HistorySidebarView.swift`) — not incomplete, just a thin wrapper name for what is actually the app's central navigation shell (tab list + session history + settings entry).

---

## RAW INVENTORY 3 — gizzi-code (`cmd/gizzi-code`)

---

## Full gizzi-code inventory

### 1. Top-level CLI commands (registered in `src/cli/main.ts`, cross-checked as actually wired)

- `gizzi` (default/TUI) — Launches the full interactive terminal agent session (Gizzi's main chat/agent loop) — large — unclear (it's the CLI's own front door, not a discrete feature to mirror)
- `attach <url>` — Attaches this terminal to an already-running gizzi-code server, optionally resuming a session — small — no (terminal/dev workflow)
- `run [message..]` — Non-interactive one-shot invocation: send a prompt/command and get output, scriptable — large — unclear (headless automation entrypoint, CI-oriented)
- `generate` — Emits OpenAPI spec/code samples for the SDK client — small — no (SDK/dev tooling)
- `debug ...` — Developer diagnostics: file/ripgrep/LSP introspection, agent config dump, skill list, global paths — medium — no (explicitly debug-only)
- `acp` — Starts an ACP (Agent Client Protocol) server for IDE integrations — small — no (IDE-integration protocol, dev-only)
- `mcp ...` — Manage MCP (Model Context Protocol) servers: list, OAuth auth, status — large — yes (integration management is a plausible settings screen)
- `connect` — Connect to and manage LLM providers, incl. list/login — large — yes (provider auth is a natural settings feature)
- `skills` — Manage skills/agents: create a new agent, list/create skills — large — yes (skill authoring/browsing plausible in GUI)
- `upgrade [target]` — Self-updates the gizzi CLI binary to latest/specific version — small — no (CLI self-updater)
- `uninstall` — Removes gizzi and related files, optionally keeping config/session data — medium — no (CLI install lifecycle)
- `serve` — Starts the gizzi-code backend server, optionally exposed via a cloudflared tunnel — medium — unclear (infra the other surfaces depend on, not itself end-user facing)
- `pair` — Pairs this CLI/device with an Allternit platform account for remote control — small — yes (this is literally the mobile-pairing flow)
- `web` — Starts the server and opens the browser-based web interface — small — yes (browser session view is a surface feature)
- `models [provider]` — Lists available models, optional cost/metadata detail — medium — yes (model picker plausible in GUI)
- `stats` — Shows token usage and cost statistics — large — yes (usage/cost dashboard, clearly GUI-shaped)
- `status` — Shows current session status (JSON or text) — small — unclear (diagnostic, though "session status" concept exists elsewhere)
- `export [sessionID]` — Exports session data as JSON — medium — yes (data export/download is a common GUI feature)
- `import <file>` — Imports session data from a JSON file or share URL — medium — yes (importing a shared session plausible in GUI)
- `github ...` — Install/run a GitHub Actions-based agent bot that responds to issue/PR mentions in a repo — very large (1600+ lines) — yes (integration setup is settings-shaped)
- `pr <number>` — Fetches and checks out a GitHub PR branch, then launches gizzi on it — small — no (git/terminal workflow)
- `session ...` — Manage sessions: list, delete, etc. — medium — yes (session list/delete is natural for GUI, likely partly mirrored already)
- `db [query]` — Opens an interactive sqlite shell / runs a query against the local gizzi DB; also shows DB path and runs migrations — medium — no (raw local DB access, dev-only)
- `cron ...` — Manage scheduled jobs: create/list/run a background cron daemon — large — yes (task scheduling is very plausible for GUI)
- `plugin ...` — Install/remove/list npm-based agent plugin packages — medium — yes (plugin management plausible in GUI)
- `init` — Initializes gizzi in the current project (scaffolds `.gizzi` workspace: identity/memory files) — medium — yes (project onboarding could have a GUI equivalent)
- `doctor` — Checks system health and configuration — medium — no (CLI/environment diagnostics)
- `verification ...` — Verifies code changes using a semi-formal reasoning pass; verify/history/show subcommands — large — unclear (could back a "review" GUI surface)
- `agent-hub` — Browse and create agents from a categorized library of specialist templates — large — yes (agent template gallery is a natural GUI feature)
- `ac ...` — Agent-communication commands: send messages to agents/roles/channels for multi-agent coordination — large — unclear
- `cowork ...` — Manage cowork runtime: autonomous agent "runs," schedules, and human-in-the-loop approvals — very large (2700+ lines) — yes (approvals/runs UI is clearly GUI-shaped)
- `cowork-team ...` — Team board / Kanban-style task assignment across agents — medium — yes
- `agent ...` — Manage agent mode; select/list active agents — small — yes
- `provider ...` — Manage LLM providers: list/add/remove/test — large — yes (API-key/provider settings plausible in GUI)
- `runtime ...` — Manage local agent-runtime discovery (register/list/status of local runtimes for cross-surface bridging) — small — no (low-level plumbing for other surfaces to use)
- `allternit [path]` — Detects, downloads, and launches the Allternit Desktop app from the CLI — medium — n/a (this launches the platform app itself)
- `brain [action]` — Brain integration: persistent knowledge/memory, remember/recall actions — medium — yes (memory is a very plausible GUI feature, "what does Gizzi remember about me")
- shell completion (`--completion`, wired via yargs, not a commands/ file) — Generates a shell completion script — small — no (inherently CLI-only)

Note: several files under `src/cli/commands/` are **not** imported by `main.ts` and are dead/orphaned as top-level commands (though some represent real capabilities reachable another way — see §3): `marketplace.ts`, `voice.ts`, `vault.ts`, `swarm.ts`, `vm.ts`, `bridge/`, `share/`, `mobile/`, `hooks/`, `agents/`, `teleport/`, `allternit-vms.ts`, `allternit-capsules.ts`, `allternit-sessions.ts`, `allternit-plugins.ts`, `createMovedToPluginCommand.ts`.

### 2. Builtin plugin/skill bundles (`src/runtime/plugins/builtin/`)

Each is a domain plugin bundling several named skills plus its own `.mcp.json` (MCP server wiring):

- data-sql — SQL/database skills: writing queries, schema design, migration review, query optimization, data quality checks, report generation — medium — yes
- data-visualization — Chart/dashboard skills: chart-type selection, dashboard design, accessibility in viz, metric definitions — medium — yes
- engineering-code — Code-review skills: code review, security review, performance review, refactor guidance, test strategy, dependency risk — medium — yes
- engineering-incident — Incident-response skills: incident response, postmortems, runbook execution, escalation drafting, timeline summarization — medium — yes
- finance-analysis — Finance skills: budget analysis, expense review, financial modeling, forecast review, unit economics — medium — yes
- hr-recruiting — HR/recruiting skills: interview planning, feedback synthesis, job descriptions, offer letters, inclusive language — medium — yes
- legal-contracts — General contract-review skills: contract review, indemnification, liability caps, governing law, termination clauses, plain-language summaries — medium — yes
- legal-nda — NDA-specific legal skills: triage, mutual/unilateral review, red-flag detection, duration checks, negotiation — medium — yes
- marketing-content — Marketing content skills: blog outlines, campaign briefs, messaging frameworks, SEO metadata, social copy — medium — yes
- operations-runbooks — Ops skills: runbook writing, checklist review, procedure execution, handoff drafting, vendor assessment — medium — yes
- product-management — PM skills: PRD writing, user stories, prioritization, competitive analysis, release notes — medium — yes
- search-knowledge — Knowledge-management skills: enterprise search, knowledge synthesis, FAQ generation, doc triage, expert finding, onboarding content — medium — yes
- security-compliance — Security/compliance skills: threat modeling, compliance checks, policy writing, audit response, security review — medium — yes

### 3. Major runtime subsystems / capabilities (beyond a single top-level command)

- MCP client/tooling — Connect to and call external MCP servers as tools (resource listing/reading, connection manager, OAuth) — large — yes
- Cron daemon (`runtime/automation/cron`) — Full cron subsystem: expression parsing, DB-persisted jobs, background daemon, workers, executors, metrics — large — yes
- Scheduler engine (`IntelliScheduleEngine`) — Separate "intelligent" task scheduling layer on top of raw cron — medium — yes
- Goal engine (`goal-engine.ts` + `/goal` command) — Durable, open-ended "goals" with milestones/validation the agent pursues autonomously until done — medium — yes
- Loop engine (`loop-engine.ts`) — Repeatedly re-runs a shell command with logged iterations (e.g. build/test loops) — small — no
- Routine engine (`routine-engine.ts` + `/routines`) — Named, multi-step saved workflows that can be executed as a sequence — medium — yes
- Hooks system (`runtime/hooks`, dispatcher + command/HTTP hook types, `/hooks` viewer) — User-configurable lifecycle hooks that fire a shell command or webhook on tool-call/session events — medium — yes
- Subagent/agent-template library (`agent-hub`, `/agents`) — Browse and instantiate pre-built specialist agent configs by category; manage custom agent configs — large — yes
- Permissions system (`permission/`, `tools/guard/permission`, `/permissions`) — Fine-grained allow/deny rules for which tools an agent may run unattended, plus interactive approval prompts — large — yes
- Plugin marketplace (`marketplace.ts`, `officialMarketplace*`, add/browse/manage-marketplace TUI dialogs) — Discover, search, and install plugins from one or more marketplace sources — large — yes
- Session sharing (`ShareNext`, export/import share URLs) — Publish a session to a shareable URL (opncd.ai) and re-import a shared session elsewhere — medium — yes
- HTML artifact publish (`html-artifact publish/status`, in-flight on branch `ao/html-artifacts`, not yet merged to this branch) — Deterministically renders structured input into a self-contained HTML "canvas" artifact and publishes/redeploys it via a stable key to the Allternit backend, viewable in the iOS Artifacts Library — large — yes (this is explicitly the newest gizzi-code→platform bridge feature)
- Remote/bridge session pairing (`RemoteSessionManager`, `SessionsWebSocket`, `remotePermissionBridge`, `pair`, `/bridge`, `/mobile`, session QR) — Pairs a terminal session with the Allternit mobile/web app for remote viewing and control over a websocket, with QR-code pairing and permission bridging — large — yes
- Voice mode (`voiceModeEnabled.ts`, `/voice`; a fuller speak/transcribe/Whisper command also exists in source as `voice.ts` but isn't wired into the CLI) — Toggleable voice interaction in the TUI — medium — yes
- Teleport / remote dev environments (`utils/teleport`, `environment-runner`, `/remote-env`, stash/resume UI) — Runs or resumes an agent session inside a remote/cloud dev environment or VM instead of locally, with stash/resume and repo-mismatch handling — large — yes
- Local VM management (`vm.ts` "Manage local VMs via vfkit"; broader `allternit-vms/capsules/sessions/plugins` commands exist but are unwired) — Spin up/manage local sandboxed VMs for execution — medium — unclear
- Cowork runtime (overarching subsystem behind `cowork`/`cowork-team`) — Autonomous multi-step agent "runs" with scheduling, approvals, and a team Kanban board for assigning work across agents — large — yes
- Swarm mode (`swarm.ts`, unwired top-level file; reachable via `/swarm` TUI command) — Run several agents in parallel on subtasks with a visual task tracker — medium — yes
- Knowledge vault (`vault.ts`, unwired top-level file) — Searchable personal knowledge base organized into folders (Daily/People/Projects/Meetings/Topics), can sync from Gmail/Calendar/Fireflies — large — yes
- Cost tracking (`cost-tracker.ts`, backs `stats`) — Tracks token usage and dollar cost per session/tool across a run — medium — yes
- Slack app install (`/install-slack-app`) — Installs the Gizzi/Claude Slack app into a workspace — small — yes
- Onboarding/workspace bootstrap (`.gizzi` scaffold: IDENTITY.md/SOUL.md/USER.md/MEMORY.md/AGENTS.md, `init`, `/onboarding`) — First-run setup that scaffolds the agent's local "personality" and memory files for a project or globally — medium — yes
- Keybinding customization (`/keybindings`) — Remap terminal keyboard shortcuts — small — no (terminal-specific)
- Theme switching (`/theme`) — Change the CLI's color theme — small — no (terminal-specific)
- Output/response style (`outputStyles`, `/output-style`) — Controls the verbosity/format of agent responses — small — yes (iOS already has "response-style settings" — good parity check)
- Debug/diagnostic tooling (`debug/*`, heapdump, ant-trace, debug-tool-call, break-cache, mock-limits/reset-limits) — Low-level developer diagnostics — medium — no (explicitly developer-only)

### 4. Quick CLI/dev-only vs. plausibly-wants-GUI flags (summary, from the research agent)

**Inherently CLI/dev-only:** shell completion, `debug`, `db`, `doctor`, `upgrade`, `uninstall`, `attach`, `pr`, `runtime` (discovery plumbing), keybindings, theme, loop engine, `acp`, `generate` (SDK codegen).

**Plausibly want a platform UI:** provider/model connection & settings, MCP server management, skills/agent-template browsing & creation, plugin marketplace, stats/usage/cost, session export/import/share/list, GitHub integration setup, cron/scheduling, permissions rules, cowork runs/approvals/team board, brain memory, vault knowledge base, swarm mode, voice mode, response-style settings, remote/bridge pairing (mobile), teleport remote environments, HTML artifact publish, Slack app install, onboarding/workspace init, agent-hub templates.

**Unclear/borderline:** `run` (headless automation/CI), `serve` (infra other surfaces sit on top of), `verification` (could back a review UI), `ac` (inter-agent messaging, may only matter internally to cowork-team), `status` (diagnostic-flavored).

---
