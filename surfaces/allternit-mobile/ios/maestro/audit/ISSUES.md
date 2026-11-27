# iOS Audit Findings

This file catalogs polish, functional, and accessibility issues found by the Maestro audit harness.

## Legend

- 🔴 Blocker — flow cannot proceed or view is unusable
- 🟡 Warning — visually broken, inaccessible, or confusing
- 🟢 Polish — spacing, alignment, copy, or minor visual inconsistency

## Issues

### Agent Hub

| Severity | Issue | Location | Screenshot | Fix |
|----------|-------|----------|------------|-----|
| 🔴 | Hub tabs (Sessions/Workspace/Config) were not exposed to accessibility and the Sessions tab text clashed with the Home stat card label | `AgentHubView.hubTabs` | agent_hub_home | Added `accessibilityLabel`, `accessibilityIdentifier("hubTab<name>")`, and explicit frame |
| 🔴 | Maestro's id-based tap on the Sessions tab triggered the Config button; id-based taps on the tab bar did not hit the intended buttons | `AgentHubView.hubTabs`, `03_agent_hub.yaml` | agent_hub_sessions | Replaced id-based tab taps with percentage-based point taps (Home 13%/16%, Sessions 38%/16%, Workspace 62%/16%, Config 87%/16%); added tab-content assertions |
| 🔴 | Agents load error blocked tab content switching — every tab showed "Couldn't load agents" when the agents fetch failed | `AgentHubView.content` | agent_hub_sessions, agent_hub_workspace, agent_hub_config | Restructured `content` to switch on `selectedTab` first; moved agents loading/error state into `homeContent` only |
| 🟢 | "Could not connect to the server" shown in agent hub without backend | `AgentHubView` | agent_hub_home | Replaced with `FriendlyStateView` (wifi.slash icon, friendly copy, Retry button) |
| 🟢 | Bot sessions error state was a plain text block | `AgentHubView.sessionsContent` | agent_hub_sessions | Replaced with `FriendlyStateView` |
| 🟢 | Workspace agents error state was a plain text block | `AgentHubView.workspacePrompt` | agent_hub_workspace | Replaced with `FriendlyStateView`; assertion updated to "Couldn't load workspace agents" |
| 🟢 | Templates sheet showed plain error block | `AgentHubView.AgentTemplateSheet.content` | — | Replaced with `FriendlyStateView` |
| 🟢 | Bot selection sheet showed plain error text | `AgentHubView.BotSelectionSheet` | — | Replaced with icon + friendly copy + Retry button row |
| 🟢 | Header vertical padding (6 pt) was inconsistent with other tab surfaces | `AgentHubView.headerBar` | agent_hub_home | Standardized to 10 pt |

### Sidebar / Navigation

| Severity | Issue | Location | Screenshot | Fix |
|----------|-------|----------|------------|-----|
| 🟡 | Hamburger sidebar toggle had no accessibility label across all feature headers | Multiple | — | Added `accessibilityLabel("Open sidebar")` to all hamburger buttons |
| 🟡 | Settings gear button had no accessibility label | `HistorySidebarView` | sidebar_open | Added `accessibilityLabel("Open settings")` |
| 🔴 | Settings could not be opened from the audit flow because `sidebar=true` already opened the sidebar and the flow redundantly tapped "Open sidebar", closing it | `10_settings.yaml` | settings_root | Removed redundant sidebar tap; added `assertVisible: "Settings"` |
| 🟢 | Sidebar history error/empty states were plain text | `HistorySidebarView.historyContent` | sidebar_open | Replaced with `FriendlyInlineStateView` to fit the narrow drawer |

### Login Gate

| Severity | Issue | Location | Screenshot | Fix |
|----------|-------|----------|------------|-----|
| 🟡 | App could not be opened without signing in — no guest/debug path to the workspace | `AllternitApp.gatedContent`, `LoginGateView` | login_gate | Added DEBUG-only "Continue without signing in" button that bypasses Clerk and onboarding |
| 🟢 | "Continue without signing in" button sat too close to the bottom safe area | `LoginGateView` button stack | login_gate | Increased bottom padding from 48 pt to 64 pt |

### Home / Chat

| Severity | Issue | Location | Screenshot | Fix |
|----------|-------|----------|------------|-----|
| 🟡 | "Turn On Response Notifications" one-time card covers the empty-state hero and can block screenshots/tests | `ChatView`, `ResponseNotificationsCard` | home_chat | Added `accessibilityLabel("Dismiss notifications prompt")` to the card's X button; audit flows now dismiss it optionally |

### Projects

| Severity | Issue | Location | Screenshot | Fix |
|----------|-------|----------|------------|-----|
| 🟢 | Projects list shows "Couldn't load projects" when the backend is unreachable | `ProjectsListView` | projects_list | Replaced with `FriendlyStateView` |
| 🟢 | Project detail files error was plain text | `ProjectDetailView.filesSection` | — | Replaced with icon + friendly copy + Retry button row |

### Artifacts Library

| Severity | Issue | Location | Screenshot | Fix |
|----------|-------|----------|------------|-----|
| 🟢 | Artifact detail showed plain error block | `ArtifactDetailsView.content` | — | Replaced with `FriendlyStateView` |

### Automation

| Severity | Issue | Location | Screenshot | Fix |
|----------|-------|----------|------------|-----|
| 🟢 | Cron/Routines/Loops error states were plain text blocks | `AutomationTasksListView`, `RoutinesListView`, `LoopsListView` | automation_cron, automation_routines, automation_loops | Replaced with `FriendlyStateView` |
| 🟢 | Header vertical padding (6 pt) was inconsistent with other tab surfaces | `AutomationTasksListView`, `RoutinesListView`, `LoopsListView` | automation_cron | Standardized to 10 pt |
| 🟢 | Task detail run-history error was plain text | `AutomationTaskDetailView.runsSection` | — | Replaced with icon + friendly copy + Retry button row |

### Models

| Severity | Issue | Location | Screenshot | Fix |
|----------|-------|----------|------------|-----|
| 🟢 | Models list shows "Could not connect to the server" for Default Model and Engines when backend is unreachable | `ModelsTabView`, `ModelManagementView` | models_list | Replaced inline error text with friendly copy + wifi.slash icon + Retry button |
| 🟢 | Default Model and Engines sections showed yellow inline error text + plain text Retry | `ModelManagementView` | models_list | Replaced with `FriendlyInlineStateView` for compact list-row empty/error/offline states |
| 🟢 | "No providers configured" empty state was unstyled | `ModelManagementView` | models_list | Polished with icon + friendly copy + action row |
| 🟢 | Header vertical padding (6 pt) was inconsistent with other tab surfaces | `ModelsTabView` header | models_list | Standardized to 10 pt |
| 🟢 | On-device chat showed plain error block | `OnDeviceChatView.failedState` | — | Replaced with `FriendlyStateView` |

### Code

| Severity | Issue | Location | Screenshot | Fix |
|----------|-------|----------|------------|-----|
| 🟢 | Code sessions show "Couldn't load code sessions" when backend is unreachable | `CodeModeView` | code_list | Replaced with `FriendlyStateView` |
| 🟢 | Code canvas showed plain error block | `CodeCanvasView` | — | Replaced with `FriendlyStateView` |
| 🟢 | Dev server ports sheet showed plain error block | `DevServerPortsView.content` | — | Replaced with `FriendlyStateView` |
| 🟢 | Session diff sheet showed plain error block | `SessionDiffListView.content` | — | Replaced with `FriendlyStateView` |
| 🟢 | File browser directory showed plain error block | `FileBrowserDirectoryView.content` | — | Replaced with `FriendlyStateView` |

### ACI

| Severity | Issue | Location | Screenshot | Fix |
|----------|-------|----------|------------|-----|
| 🟢 | Mini Apps store showed plain error block | `MiniAppsStoreView` | — | Replaced with `FriendlyStateView` |
| 🟢 | Mini Apps grid placeholder used `app.window` which renders blank on iOS 18 | `ACITabView.miniAppsGrid` | — | Changed icon to `square.grid.2x2` |

### Connectors

| Severity | Issue | Location | Screenshot | Fix |
|----------|-------|----------|------------|-----|
| 🟢 | Connectors error state was a plain text block | `ConnectorsListView` | — | Replaced with `FriendlyStateView` |

### Cowork

| Severity | Issue | Location | Screenshot | Fix |
|----------|-------|----------|------------|-----|
| 🟢 | Cowork tasks error state was a plain text block | `CoworkTasksListView` | — | Replaced with `FriendlyStateView` |

### Agent Activity

| Severity | Issue | Location | Screenshot | Fix |
|----------|-------|----------|------------|-----|
| 🟢 | Agent activity list showed plain error block | `AgentActivityListView.content` | — | Replaced with `FriendlyStateView` |
| 🟢 | Agent activity detail messages error was plain text | `AgentActivityDetailView.messagesSection` | — | Replaced with icon + friendly copy + Retry button row |

### Office

| Severity | Issue | Location | Screenshot | Fix |
|----------|-------|----------|------------|-----|
| 🟢 | Office document view used plain `ContentUnavailableView` error | `OfficeDocumentView.body` | — | Replaced with `FriendlyStateView` |
| 🟢 | Office documents list used plain `ContentUnavailableView` error | `OfficeDocumentsView.body` | — | Replaced with `FriendlyStateView` |

### Agent Detail

| Severity | Issue | Location | Screenshot | Fix |
|----------|-------|----------|------------|-----|
| 🟢 | Workspace files error state was a plain text block | `AgentDetailView.workspaceSections` | — | Replaced with icon + friendly copy + Retry button row |
| 🟢 | Workspace file editor showed plain error block | `WorkspaceFileEditorView.body` | — | Replaced with `FriendlyStateView` |
| 🟢 | Desktop status error was plain text | `AgentDetailView.desktopContent` | — | Replaced with icon + friendly copy + Retry button row |

### Chat / Composer Sheets

| Severity | Issue | Location | Screenshot | Fix |
|----------|-------|----------|------------|-----|
| 🟢 | Agent selection sheet showed plain error text | `AgentSelectionSheet` | — | Replaced with wifi.slash icon + friendly copy |
| 🟢 | Model picker sheet showed plain error block | `ModelPickerSheet` | — | Replaced with `FriendlyStateView` |
| 🟢 | Conversation load/refresh/send banners used raw transport copy | `ChatViewModel` | — | Error descriptions now pass through `FriendlyErrorMessage.from` |
| 🟢 | Project picker menu showed plain error text | `ComposerPlusSheet.projectRow` | — | Replaced with icon + friendly copy + Retry button row |

### Settings

| Severity | Issue | Location | Screenshot | Fix |
|----------|-------|----------|------------|-----|
| 🟡 | Settings sheet could be blocked by the notification opt-in card behind it | `10_settings.yaml` | settings_root | Flow now optionally dismisses the notification card before opening settings |
| 🟢 | Webhooks settings showed plain error text | `WebhooksSettingsView` | — | Replaced with icon + friendly copy + Retry button row |
| 🟢 | Memory settings showed plain error block | `MemorySettingsView` | — | Replaced with `FriendlyStateView` |
| 🟢 | Platform settings showed plain error text for model load failure | `PlatformSettingsView` | — | Replaced with icon + friendly copy + Retry row |
| 🟢 | Runtime Operations showed plain error block | `RuntimeOperationsView` | — | Replaced with `FriendlyStateView` |
| 🟢 | Monitor showed plain error block | `MonitorView` | — | Replaced with `FriendlyStateView` |
| 🟢 | Usage section showed plain "Usage unavailable" text | `SettingsView.usageSection` | settings_root | Replaced with `FriendlyStateView` (chart.bar icon, friendly title) |

### Design System

| Severity | Issue | Location | Screenshot | Fix |
|----------|-------|----------|------------|-----|
| 🟢 | Empty/error/offline states were inconsistent across tab surfaces | Multiple | — | Added reusable `FriendlyStateView` + `FriendlyErrorMessage` in `Core/DesignSystem/FriendlyStateView.swift` |
| 🟢 | Full-screen `FriendlyStateView` was too heavy for narrow list-row sections | Multiple | — | Added compact `FriendlyInlineStateView` for inline empty/error/offline rows |

### Audit Harness

| Severity | Issue | Location | Screenshot | Fix |
|----------|-------|----------|------------|-----|
| 🔴 | `run-audit.sh` tried to execute `config.yaml` as a Maestro flow, causing a false failure | `run-audit.sh` | — | Changed glob from `*.yaml` to `[0-9]*.yaml` |
| 🔴 | Debug simulator builds intermittently crashed on launch with "Library not loaded: @rpath/Allternit.debug.dylib" / unsigned debug dylib | `Allternit.xcodeproj` debug build settings | login_gate, home_chat, agent_hub, projects, artifacts, automation | Set `ENABLE_DEBUG_DYLIB=NO` in `project.yml` debug config and regenerated project |
| 🔴 | `03_agent_hub.yaml` toggled the already-open sidebar closed before tapping "agent \| bot hub" | `03_agent_hub.yaml` | agent_hub_home | Removed redundant "Open sidebar" tap; tabs now tapped by percentage point |
| 🟢 | Workspace tab assertion expected old plain-text error copy | `03_agent_hub.yaml` | agent_hub_workspace | Updated assertion to "Couldn't load workspace agents" to match `FriendlyStateView` |
| 🟢 | Login gate flow did not assert the skip-auth affordance | `01_login_gate.yaml` | login_gate | Added `assertVisible: "Continue without signing in"` |
| 🔴 | Maestro launch arguments were inconsistently parsed: `skip-auth` worked in some flows but dropped the app back to the login gate in others because `CommandLine.arguments.contains` only matched raw keys | `Core/LaunchArguments.swift`, `AllternitApp.swift` | login_gate (flows 11–19) | Created `launchArgumentEnabled(_:)` normalizer that accepts `key`, `-key`, `--key`, `key=true`, `-key=true`, and `--key=true`; replaced every `CommandLine.arguments.contains` check in the codebase |

### Skip-auth / no-Clerk launch

| Severity | Issue | Location | Screenshot | Fix |
|----------|-------|----------|------------|-----|
| 🔴 | App crashed on launch with `-skip-auth` when `CLERK_PUBLISHABLE_KEY` was unset because `Clerk.shared` was accessed before `Clerk.configure` | `AllternitApp.body`, `AuthManager` | login_gate | Only inject `Clerk.shared` into the environment when the SDK is configured; guarded `AuthManager.displayName`, `avatarInitial`, `primaryEmail`, and `firstName` to avoid touching `Clerk.shared` when unconfigured |
| 🟡 | "Continue without signing in" was not offered when Clerk was unconfigured | `AllternitApp.body` | login_gate | DEBUG fallback `LoginGateView` now receives the skip-auth callback even when `isClerkConfigured == false` |

### Remaining state-view polish (second pass)

| Severity | Issue | Location | Screenshot | Fix |
|----------|-------|----------|------------|-----|
| 🟢 | Office document / documents / local document used `ContentUnavailableView` | `OfficeDocumentView`, `OfficeDocumentsView`, `LocalDocumentView` | — | Replaced with `FriendlyStateView` |
| 🟢 | Marketplace list/detail error and empty states used plain text | `MarketplaceView`, `MarketplaceListingDetailView` | — | Replaced with `FriendlyStateView` / `FriendlyInlineStateView` |
| 🟢 | Runtime Operations budget empty state was plain text | `RuntimeOperationsView.budgetCardContent` | — | Replaced with `FriendlyInlineStateView` |
| 🟢 | Connectors empty state was a custom icon+text block | `ConnectorsListView.content` | — | Replaced with `FriendlyStateView` |
| 🟢 | Cowork sessions error/empty were plain text | `CoworkWorkspaceView.recentSessionsSection` | — | Replaced with `FriendlyInlineStateView` |
| 🟢 | Agent Activity list empty state was custom icon+text | `AgentActivityListView.emptyState` | — | Replaced with `FriendlyStateView` |
| 🟢 | Agent Activity detail messages empty was plain text | `AgentActivityDetailView.messagesSection` | — | Replaced with `FriendlyInlineStateView` |
| 🟢 | Intelli-Schedule error/empty states were plain text | `IntelliSchedulePanel.content` | — | Replaced with `FriendlyStateView` |
| 🟢 | Memory settings empty state was plain text | `MemorySettingsView` | — | Replaced with `FriendlyStateView` |
| 🟢 | Webhooks settings empty/delivery/bot states were custom/plain | `WebhooksSettingsView` | — | Replaced with `FriendlyStateView` / `FriendlyInlineStateView` |
| 🟢 | History sidebar filter-empty state was plain text | `HistorySidebarView` | — | Replaced with `FriendlyInlineStateView` |
| 🟢 | Project detail files/chats empty states were plain text | `ProjectDetailView` | — | Replaced with `FriendlyInlineStateView` |
| 🟢 | Automation task detail / loop detail empty states were plain text | `AutomationTaskDetailView`, `LoopDetailView` | — | Replaced with `FriendlyInlineStateView` |
| 🟢 | Code runtimes error/empty were plain text | `CodeModeView.runtimesSection` | — | Replaced with `FriendlyInlineStateView` |
| 🟢 | Artifact detail empty state was plain text | `ArtifactDetailsView` | — | Replaced with `FriendlyStateView` |
| 🟢 | Additional Settings manager, Code, Agent, Automation, Project/Artifact/ACI, and Chat/Composer views still had custom/plain error/empty/offline/no-results states | ~35 additional Swift files | — | Converted to `FriendlyStateView` / `FriendlyInlineStateView` across the remaining surfaces |

## External Reference

- [openmausbot.com](https://www.openmausbot.com) was checked for iOS app parity. Their iOS app is currently listed as **"Not yet available" / "Coming soon"**, so there is no published iOS build or design to reference at this time.

## Full-view polish pass (second pass)

A second, file-by-file audit of every SwiftUI view in `Features/` and `Core/` found and fixed the remaining inconsistencies below.

| Severity | Issue | Location | Fix |
|----------|-------|----------|-----|
| 🟡 | Missing `accessibilityLabel` on icon-only header/close/toolbar buttons across Chat, Code, Automation, Cowork, Projects, ACI, Settings, and History | ~45 buttons | Added explicit labels (`Open sidebar`, `Close`, `New *`, `Refresh *`, `Copy content`, `Pin/Unpin app`, etc.) |
| 🟡 | Custom inline error/empty states still built from raw `HStack`/`VStack` + `Image` + `Text` | `AgentSelectionSheet`, `AutomationTaskDetailView`, `CoworkWorkspaceView`, `CoworkTasksListView`, `MemorySettingsView`, `WebhooksSettingsView`, `OnDeviceChatView`, `ComposerPlusSheet` | Replaced with `FriendlyInlineStateView` or `FriendlyStateView` preserving retry actions |
| 🟢 | Hardcoded `.red` / `.green` / `.yellow` / `.orange` used for status, error, success, and warning states | `ChatView`, `CodeModeView`, `DiffRenderer`, `DevServer*`, `ChangesetReviewSheet`, `MarketplaceView`, `Cowork*`, `IntelliSchedulePanel`, `SettingsView`, `Infrastructure*`, `Platform*`, `ModelManagement*`, `ComputeBillingView` | Replaced with `Theme.statusError`, `Theme.statusSuccess`, `Theme.statusWarning` |
| 🟢 | `HistorySidebarView` body became too complex for the Swift type checker after accessibility modifiers | `HistorySidebarView.swift` | Split `body` into `tabList` and `footer` subviews |
| 🔴 | Maestro audit blocked by iOS "Allow Live Activities from Allternit?" system permission dialog | `run-audit.sh` | Script now grants `live-activities` permission and disables simulator auto-lock before running flows |

## Audit Run Log

- Latest run: 2026-08-20 06:25 UTC — **20/20 flows passed**
- Simulator: iPhone 16 (iOS 18.3.1) — 2CC27A61-C301-41C2-9B9E-76BF4DF3C84B
- Branch: session/ios-bot-parity
- Flows: `01_login_gate`, `02_home_chat`, `03_agent_hub`, `04_projects`, `05_artifacts`, `06_automation`, `07_models`, `08_code`, `09_aci`, `10_settings`, `11_chat_composer_plus`, `12_chat_model_picker`, `13_chat_agent_selection`, `14_project_new`, `15_settings_memory`, `16_settings_platform`, `17_settings_brain_spike`, `18_automation_loop_new`, `19_code_filter`, `20_aci_browser_chat`
- Notes: Extended sub-view coverage is now live. Fixed launch-argument normalization so `skip-auth` and deep-link args are reliable regardless of Maestro's serialization format.
- Screenshots: `surfaces/allternit-mobile/ios/maestro/audit/screenshots`

Previous runs:

- 2026-08-19 20:52 UTC — **10/10 flows passed**
- 2026-08-19 20:12 UTC — **10/10 flows passed**
- 2026-08-19 19:30 UTC — **10/10 flows passed**
