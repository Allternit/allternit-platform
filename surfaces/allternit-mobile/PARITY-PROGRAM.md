# Android parity program

One program, not piecemeal (Eoj 2026-11-19: "write and document, then get this done"). Source: `surfaces/allternit-mobile/PARITY.md` (the ledger from step 2). Every iOS screen has one row here. A row counts only after it is **compared**: the real iOS screen (iPhone 17 Pro sim, Maestro) and the Android build (emulator w9 or the Moto) side by side in a compare-*.png, every difference fixed. m.allternit.com is the reference where iOS has no screen. There are no Android-only rows.

Status values: `todo` · `built (#PR)` = code in a PR, not compared · `compared` = paired shots exist (Proof column) · `N/A: reason`.

Rules: one builder at a time; each wave = one stacked PR, train-ready at the end of the wave; one report per wave with this whole table and the shots.

## Waves

| Wave | What | Rows | Plan step |
|---|---|---|---|
| A | Chat works | 19 | 8 |
| B | First run | 5 | 7c |
| C | Shell rest | 10 | 7c |
| D | Projects + artifacts | 21 | 9 |
| E | Bots | 32 | 10 |
| F | Modes | 75 | 12–14 |
| G | Settings | 44 | 15 |
| H | Money + voice | 15 | 16–17 |
| I | System | 11 | 11, 18–19 |
| J | Other surfaces + cutover | 0 | 20–24 |
| | **Total** | **232** | |

## Program items that are not one iOS screen

| Wave | Item | Status |
|---|---|---|
| A | Chat test account with a computer or Allternit Cloud (allternitpbc gets 428 "No data-plane node") | asked Eoj 11-19 |
| A | No-computer state in chat: clear message + path to connect (plan chats stay first-class) | built (#2614, feature/chat/ComputeGate.kt) |
| C | -open-tab / sidebar / skip-auth debug extras for Maestro flows | built (#2626, feature/shell/MainActivity.kt readDebugLaunch) |
| C | TWA → native sign-in handoff (user already signed in on m.allternit.com) | built (#2626, Android side: feature/shell/TwaHandoff.kt); server route + m.allternit.com page needed, see #2626 "Needs" |
| C | Continue with Apple retest (needs return URL on Services ID com.allternit.signin — owner) | blocked on owner |
| I | FCM device tokens: backend accepts platform android (step 6 finding c) | todo |
| J | Web / Desktop / Gizzi links to the Play app | todo |
| J | Docs: surfaces/docs/guides/android-app.mdx "What works today" kept in step with this file | todo |
| J | Ledger audit: 0 todo, 0 built | todo |
| J | Staged Play cutover 10→50→100% (Eoj's go) | todo |

## Wave A. Chat works

| Screen | Area | iOS path | Web path | Android | Status | Proof |
|---|---|---|---|---|---|---|
| AgentSelectionSheet | Chat | ios/Features/Chat/Views/AgentSelectionSheet.swift | — | — | N/A: no caller on iOS (dead code) | — |
| AnswerCardView | Chat | ios/Features/Chat/Views/AnswerCards/AnswerCardView.swift | — | feature/chat/ChatCards.kt (AnswerCard, WritingDraftCard) | built (#2614) | — |
| ChatErrorCardView | Chat | ios/Features/Chat/Views/ChatErrorCardView.swift | — | feature/chat/ChatCards.kt (ChatErrorCard) | built (#2614) | — |
| ChatView | Chat | ios/Features/Chat/Views/ChatView.swift | — | feature/chat/ChatScreen.kt + feature/shell/ChatPage.kt | built (#2614) | — |
| ChatsScreen | Chat | — | src/fabric-session/phone/screens/ChatsScreen.tsx | feature/shell/ShellScreens.kt (AllChatsScreen) → ChatPage.kt | built (#2614) | — |
| ComposerPlusSheet | Chat | ios/Features/Chat/Views/ComposerPlusSheet.swift | — | feature/shell/PlusSheet.kt | built (#2614: photos, camera, files, GitHub upload on send) | — |
| ComposerView | Chat | ios/Features/Chat/Composer/ComposerView.swift | — | feature/shell/Composer.kt | built (#2614: send, stop, edit, no-brain pick) | — |
| ConnectSuggestCardView | Chat | ios/Features/Chat/Views/ConnectSuggestCardView.swift | — | feature/chat/ChatCards.kt (ConnectSuggestCard) | built (#2614) | — |
| FormSurfacesView | Chat | ios/Features/Chat/Views/FormSurfacesView.swift | — | — | N/A: no caller on iOS (dead code) | — |
| ModelPickerSheet | Chat | ios/Features/Chat/Views/ModelPickerSheet.swift | — | feature/shell/ModelPicker.kt | built (#2614: the pick drives the send) | — |
| RunCodeFilesView | Chat | ios/Features/Chat/Views/RunCodeFilesView.swift | — | feature/chat/ChatCards.kt (RunCodeFilesView, UploadFileCard) | built (#2614) | — |
| SettingsChangeCardView | Chat | ios/Features/Chat/Views/SettingsChangeCardView.swift | — | feature/chat/ChatCards.kt (SettingsChangeCard) | built (#2614: status + Open Settings; Apply/Undo on this phone with wave G) | — |
| SketchSheet | Chat | ios/Features/Chat/Views/SketchSheet.swift | — | feature/shell/SketchSheet.kt (from PlusSheet.kt) | built (#2614) | — |
| ThreadShareSheet | Chat | ios/Features/Chat/Views/ThreadShareSheet.swift | — | feature/chat/ThreadShare.kt (chat ⋯ menu, reply Share menu) | built (#2614) | — |
| section: chats | Chat | — | src/fabric-session/phone/phone-nav.ts (PhoneSection chats) | feature/shell/ShellScreens.kt (AllChatsScreen) | built (#2614) | — |
| HistorySidebarView | History | ios/Features/History/Views/HistorySidebarView.swift | — | feature/shell/ShellDrawer.kt | built (#2614: Recents open the chat) | — |
| ComputeGateView | Shared | ios/Features/Shared/ComputeGateView.swift | — | feature/chat/ComputeGate.kt (ComputeGateCard) | built (#2614) | — |
| ConnectComputerView | Shared | ios/Features/Shared/ConnectComputerView.swift | — | feature/chat/ComputeGate.kt (ConnectComputerSheet) | built (#2614) | — |
| mode: home | Shell | — | src/fabric-session/phone/phone-nav.ts (PhoneMode home) | feature/shell/WorkspaceShell.kt | built (#2600) | — |

## Wave B. First run

| Screen | Area | iOS path | Web path | Android | Status | Proof |
|---|---|---|---|---|---|---|
| ConnectedView | Onboarding | ios/Features/Onboarding/Setup/ConnectedView.swift | — | feature/shell/firstrun/ConnectedScreen.kt (+ SetupHandoff.kt) | built (#2621) | — |
| FirstRunSetupView | Onboarding | ios/Features/Onboarding/Setup/FirstRunSetupView.swift | — | feature/shell/firstrun/FirstRunSetupScreen.kt (+ SetupModel.kt, SetupApi.kt, ConnectSheets.kt) | built (#2621) | — |
| MeetYourBotsView | Onboarding | ios/Features/Onboarding/MeetYourBotsView.swift | — | feature/shell/firstrun/MeetYourBotsScreen.kt (+ MeetFinish.kt, MeetVisuals.kt) | built (#2621; vendor bots, New bot, Import and the routine time editor not yet) | — |
| OnboardingView | Onboarding | ios/Features/Onboarding/OnboardingView.swift | — | feature/shell/firstrun/FirstRunGate.kt (+ FirstRunStore.kt) | built (#2621) | — |
| SetupGraphView | Onboarding | ios/Features/Onboarding/Setup/SetupGraphView.swift | — | feature/shell/firstrun/SetupArt.kt (SetupGraph) | built (#2621) | — |

## Wave C. Shell rest

| Screen | Area | iOS path | Web path | Android | Status | Proof |
|---|---|---|---|---|---|---|
| ThinkingSliderView | Chat | ios/Features/Chat/Views/ThinkingSliderView.swift | — | feature/shell/ThinkingSlider.kt | built (#2626) | — |
| LoginArrivalView | Onboarding | ios/Features/Onboarding/SignInStage/LoginArrivalView.swift | — | feature/onboarding/LoginArrival.kt | built (#2596) | — |
| SignInStageView | Onboarding | ios/Features/Onboarding/SignInStage/SignInStageView.swift | — | feature/onboarding/SignInScreen.kt | built (#2596) | — |
| InboxSheet | Shell | ios/Features/Shell/InboxSheet.swift | — | feature/shell/Inbox.kt | built (#2626) | — |
| ModeDialView | Shell | ios/Features/Shell/ModeDialView.swift | — | feature/shell/ModeFan.kt | built (#2600) | — |
| NativeSessionPickerSheet | Shell | ios/Features/Shell/NativeSessionPickerSheet.swift | — | feature/shell/StartSheets.kt | built (#2626) | — |
| QuickTaskSheet | Shell | ios/Features/Shell/QuickTaskSheet.swift | — | feature/shell/StartSheets.kt | built (#2626) | — |
| SessionOverviewView | Shell | ios/Features/Shell/SessionOverviewView.swift | src/fabric-session/phone/screens/PhoneOverviewOverlay.tsx | feature/shell/SessionOverview.kt | built (#2626) | — |
| ShellSearchView | Shell | ios/Features/Shell/ShellSearchView.swift | src/fabric-session/phone/screens/PhoneSearchOverlay.tsx | feature/shell/ShellSearch.kt | built (#2626) | — |
| StageBackdropView | Shell | ios/Features/Shell/Backdrop/StageBackdropView.swift | — | feature/shell/backdrop/StageBackdrop.kt (drawn from waves D–F: iOS draws it only on bot, group, project and Code screens, and in the drawer while one of those publishes it) | built (#2626) | — |

## Wave D. Projects + artifacts

| Screen | Area | iOS path | Web path | Android | Status | Proof |
|---|---|---|---|---|---|---|
| ArtifactDetailsView | Artifacts | ios/Features/Artifacts/Views/ArtifactDetailsView.swift | — | — | todo | — |
| ArtifactLiveWebView | Artifacts | ios/Features/Artifacts/Views/ArtifactLiveWebView.swift | — | — | todo | — |
| ArtifactPageWebView | Artifacts | ios/Features/Artifacts/Views/ArtifactPageWebView.swift | — | — | todo | — |
| ArtifactStudioView | Artifacts | ios/Features/Artifacts/Views/ArtifactStudioView.swift | — | — | todo | — |
| ArtifactsLibraryView | Artifacts | ios/Features/Artifacts/Views/ArtifactsLibraryView.swift | — | — | todo | — |
| PDFTemplateSheet | Artifacts | ios/Features/Artifacts/Views/PDFTemplateSheet.swift | — | — | todo | — |
| SandboxedArtifactWebView | Artifacts | ios/Features/Artifacts/Views/SandboxedArtifactWebView.swift | — | — | todo | — |
| V2ArtifactEditorView | Artifacts | ios/Features/Artifacts/Views/V2ArtifactEditorView.swift | — | — | todo | — |
| V2ArtifactView | Artifacts | ios/Features/Artifacts/Views/V2ArtifactView.swift | — | — | todo | — |
| section: artifacts | Artifacts | — | src/fabric-session/phone/phone-nav.ts (PhoneSection artifacts) | — | todo | — |
| CodeProjectDetailView | Projects | ios/Features/Projects/Views/CodeProjectDetailView.swift | — | — | todo | — |
| DesignProjectCanvasView | Projects | ios/Features/Projects/Views/DesignProjectCanvasView.swift | — | — | todo | — |
| FolderCoverView | Projects | ios/Features/Projects/Views/FolderCoverView.swift | — | — | todo | — |
| FolderLookSheet | Projects | ios/Features/Projects/Views/FolderLookSheet.swift | — | — | todo | — |
| NewProjectSheet | Projects | ios/Features/Projects/Views/NewProjectSheet.swift | — | — | todo | — |
| PadEditSheet | Projects | ios/Features/Projects/Views/PadEditSheet.swift | — | — | todo | — |
| ProjectDetailView | Projects | ios/Features/Projects/Views/ProjectDetailView.swift | — | — | todo | — |
| ProjectPadView | Projects | ios/Features/Projects/Views/ProjectPadView.swift | — | — | todo | — |
| ProjectsListView | Projects | ios/Features/Projects/Views/ProjectsListView.swift | — | — | todo | — |
| ProjectsScreen | Projects | — | src/fabric-session/phone/screens/ProjectsScreen.tsx | — | todo | — |
| section: projects | Projects | — | src/fabric-session/phone/phone-nav.ts (PhoneSection projects) | — | todo | — |

## Wave E. Bots

| Screen | Area | iOS path | Web path | Android | Status | Proof |
|---|---|---|---|---|---|---|
| AgentActivityDetailView | AgentActivity | ios/Features/AgentActivity/Views/AgentActivityDetailView.swift | — | — | todo | — |
| AgentActivityListView | AgentActivity | ios/Features/AgentActivity/Views/AgentActivityListView.swift | — | — | todo | — |
| AgentDetailView | Agents | ios/Features/Agents/AgentDetailView.swift | — | — | todo | — |
| AgentHubView | Agents | ios/Features/Agents/AgentHubView.swift | — | — | todo | — |
| AgentIdenticonView | Agents | ios/Features/Agents/AgentIdenticonView.swift | — | — | todo | — |
| BotBrainPickerSheet | Agents | ios/Features/Agents/BotBrainPickerSheet.swift | — | — | todo | — |
| BotDesktopView | Agents | ios/Features/Agents/Desktop/BotDesktopView.swift | src/fabric-session/phone/screens/PhoneDesktopOverlay.tsx | — | todo | — |
| BotHomeView | Agents | ios/Features/Agents/BotHomeView.swift | — | — | todo | — |
| BotWebhooksSheet | Agents | ios/Features/Agents/BotWebhooksSheet.swift | — | — | todo | — |
| BotWorkspaceFilesSheet | Agents | ios/Features/Agents/BotWorkspaceFilesSheet.swift | — | — | todo | — |
| MarketplaceView | Agents | ios/Features/Agents/MarketplaceView.swift | — | — | todo | — |
| WorkspaceFileEditorView | Agents | ios/Features/Agents/WorkspaceFileEditorView.swift | — | — | todo | — |
| BotAvatarView | Bots | ios/Features/Bots/BotAvatarView.swift | — | — | todo | — |
| BotChatSessionView | Bots | ios/Features/Bots/BotChatSessionView.swift | — | — | todo | — |
| BotFormSheet | Bots | ios/Features/Bots/CreateBot/BotFormSheet.swift | — | — | todo | — |
| BotHomeTabsView | Bots | ios/Features/Bots/BotHomeTabsView.swift | — | — | todo | — |
| BotLaunchpadView | Bots | ios/Features/Bots/BotLaunchpadView.swift | — | — | todo | — |
| BotLiveStageView | Bots | ios/Features/Bots/BotLiveStageView.swift | — | — | todo | — |
| BotPageView | Bots | ios/Features/Bots/BotPageView.swift | — | — | todo | — |
| BotProjectPage | Bots | ios/Features/Bots/BotProjectPage.swift | — | — | todo | — |
| BotThreadsView | Bots | ios/Features/Bots/BotThreadsView.swift | — | — | todo | — |
| BotsModeRootView | Bots | ios/Features/Bots/BotsModeRootView.swift | — | — | todo | — |
| BotsScreen | Bots | — | src/fabric-session/phone/screens/BotsScreen.tsx | — | todo | — |
| section: bots | Bots | — | src/fabric-session/phone/phone-nav.ts (PhoneSection bots) | — | todo | — |
| DecisionsDeckView | Decisions | ios/Features/Decisions/Views/DecisionsDeckView.swift | — | — | todo | — |
| PermissionAskView | Decisions | ios/Features/Decisions/Views/PermissionAskView.swift | — | — | todo | — |
| PhoneHoldReviewView | Decisions | ios/Features/Decisions/Views/PhoneHoldReviewView.swift | — | — | todo | — |
| QuestionCardView | Decisions | ios/Features/Decisions/Views/QuestionCardView.swift | — | — | todo | — |
| AgentRunTreeSheet | RunTree | ios/Features/RunTree/Views/AgentRunTreeSheet.swift | — | — | todo | — |
| SubagentFaceView | RunTree | ios/Features/RunTree/Views/SubagentFaceView.swift | — | — | todo | — |
| ImpactMapView | Shared | ios/Features/Shared/ImpactMapView.swift | — | — | todo | — |
| mode: bots | Shell | — | src/fabric-session/phone/phone-nav.ts (PhoneMode bots) | — | todo | — |

## Wave F. Modes

| Screen | Area | iOS path | Web path | Android | Status | Proof |
|---|---|---|---|---|---|---|
| ACIAgentRunView | ACI | ios/Features/ACI/Views/ACIAgentRunView.swift | — | — | todo | — |
| ACIComputerDetailView | ACI | ios/Features/ACI/Views/ACIComputerDetailView.swift | src/fabric-session/phone/screens/PhoneComputerViewOverlay.tsx | — | todo | — |
| ACIExtensionsView | ACI | ios/Features/ACI/Views/ACIExtensionsView.swift | — | — | todo | — |
| ACIMcpAppsView | ACI | ios/Features/ACI/Views/ACIMcpAppsView.swift | — | — | todo | — |
| ACITabView | ACI | ios/Features/ACI/Views/ACITabView.swift | — | — | todo | — |
| ACIWebBrowserView | ACI | ios/Features/ACI/Views/ACIWebBrowserView.swift | — | — | todo | — |
| AciScreen | ACI | — | src/fabric-session/phone/screens/AciScreen.tsx | — | todo | — |
| AppScreenWebView | ACI | ios/Features/ACI/Views/AppScreenWebView.swift | — | — | todo | — |
| BrowserChatView | ACI | ios/Features/ACI/Views/BrowserChatView.swift | — | — | todo | — |
| FilesInboxRequestsView | ACI | ios/Features/ACI/Views/Files/FilesInboxRequestsView.swift | — | — | todo | — |
| MiniAppRuntimeView | ACI | ios/Features/ACI/Views/MiniAppRuntimeView.swift | src/fabric-session/phone/screens/PhoneMiniAppOverlay.tsx | — | todo | — |
| MiniAppsStoreView | ACI | ios/Features/ACI/Views/MiniAppsStoreView.swift | — | — | todo | — |
| TeachView | ACI | ios/Features/ACI/Views/TeachView.swift | src/fabric-session/phone/screens/PhoneTeachOverlay.tsx | — | todo | — |
| section: aci | ACI | — | src/fabric-session/phone/phone-nav.ts (PhoneSection aci) | — | todo | — |
| AutomationHubView | Automation | ios/Features/Automation/Views/AutomationHubView.swift | — | — | todo | — |
| AutomationScheduleGraphView | Automation | ios/Features/Automation/Views/AutomationScheduleGraphView.swift | — | — | todo | — |
| AutomationSchedulerView | Automation | ios/Features/Automation/Views/AutomationSchedulerView.swift | — | — | todo | — |
| AutomationTaskDetailView | Automation | ios/Features/Automation/Views/AutomationTaskDetailView.swift | — | — | todo | — |
| AutomationTasksListView | Automation | ios/Features/Automation/Views/AutomationTasksListView.swift | — | — | todo | — |
| AutomationTasksPhoneView | Automation | ios/Features/Automation/Views/AutomationTasksPhoneView.swift | — | — | todo | — |
| AutomationWizardSheet | Automation | ios/Features/Automation/Views/AutomationWizardSheet.swift | — | — | todo | — |
| CreateAutomationTaskSheet | Automation | ios/Features/Automation/Views/CreateAutomationTaskSheet.swift | — | — | todo | — |
| CreateLoopSheet | Automation | ios/Features/Automation/Views/CreateLoopSheet.swift | — | — | todo | — |
| CreateRoutineSheet | Automation | ios/Features/Automation/Views/CreateRoutineSheet.swift | — | — | todo | — |
| LoopDetailView | Automation | ios/Features/Automation/Views/LoopDetailView.swift | — | — | todo | — |
| LoopsListView | Automation | ios/Features/Automation/Views/LoopsListView.swift | — | — | todo | — |
| RoutineDetailView | Automation | ios/Features/Automation/Views/RoutineDetailView.swift | — | — | todo | — |
| RoutinesListView | Automation | ios/Features/Automation/Views/RoutinesListView.swift | — | — | todo | — |
| ScheduledTaskSheet | Automation | ios/Features/Automation/Views/ScheduledTaskSheet.swift | — | — | todo | — |
| TasksBoardScreen | Automation | ios/Features/Automation/Views/TasksBoardScreen.swift | — | — | todo | — |
| section: automation | Automation | — | src/fabric-session/phone/phone-nav.ts (PhoneSection automation) | — | todo | — |
| section: tasks | Automation | — | src/fabric-session/phone/phone-nav.ts (PhoneSection tasks) | — | todo | — |
| ChangesetReviewSheet | Code | ios/Features/Code/ChangesetReviewSheet.swift | — | — | todo | — |
| CodeEditorView | Code | ios/Features/Code/CodeEditor/CodeEditorView.swift | — | — | todo | — |
| CodeModeView | Code | ios/Features/Code/CodeModeView.swift | — | — | todo | — |
| CodeOverviewSheet | Code | ios/Features/Code/CodeOverviewSheet.swift | — | — | todo | — |
| CodeRuntimePairingView | Code | ios/Features/Code/CodeRuntimePairingView.swift | — | — | todo | — |
| CodeThreadView | Code | ios/Features/Code/CodeThreadView.swift | — | — | todo | — |
| DevServerPortsView | Code | ios/Features/Code/DevPreview/DevServerPortsView.swift | — | — | todo | — |
| DevServerPreviewView | Code | ios/Features/Code/DevPreview/DevServerPreviewView.swift | — | — | todo | — |
| FileBrowserDirectoryView | Code | ios/Features/Code/FileBrowser/FileBrowserDirectoryView.swift | — | — | todo | — |
| FileBrowserView | Code | ios/Features/Code/FileBrowser/FileBrowserView.swift | — | — | todo | — |
| FileDetailView | Code | ios/Features/Code/FileBrowser/FileDetailView.swift | — | — | todo | — |
| FileDiffDetailView | Code | ios/Features/Code/DiffViewer/FileDiffDetailView.swift | — | — | todo | — |
| PhoneCodeSessionsView | Code | — | src/fabric-session/phone/screens/PhoneCodeSessionsView.tsx | — | todo | — |
| PhoneCodeTerminal | Code | — | src/fabric-session/phone/screens/PhoneCodeTerminal.tsx | — | todo | — |
| SessionDiffListView | Code | ios/Features/Code/DiffViewer/SessionDiffListView.swift | — | — | todo | — |
| TerminalSessionView | Code | ios/Features/Code/TerminalSessionView.swift | — | — | todo | — |
| section: code | Code | — | src/fabric-session/phone/phone-nav.ts (PhoneSection code) | — | todo | — |
| ConnectWizardView | Connect | ios/Features/Connect/ConnectWizardView.swift | — | — | todo | — |
| VendorConnectView | Connect | ios/Features/Connect/VendorConnectView.swift | — | — | todo | — |
| CoworkSessionWorkspaceView | Cowork | ios/Features/Cowork/Views/CoworkSessionWorkspaceView.swift | — | — | todo | — |
| CoworkTaskDetailView | Cowork | ios/Features/Cowork/Views/CoworkTaskDetailView.swift | — | — | todo | — |
| CoworkTasksListView | Cowork | ios/Features/Cowork/Views/CoworkTasksListView.swift | — | — | todo | — |
| CoworkWorkspaceView | Cowork | ios/Features/Cowork/Views/CoworkWorkspaceView.swift | — | — | todo | — |
| CreateCoworkTaskSheet | Cowork | ios/Features/Cowork/Views/CreateCoworkTaskSheet.swift | — | — | todo | — |
| FabricApprovalsView | Cowork | ios/Features/Cowork/Views/FabricApprovalsView.swift | — | — | todo | — |
| BenchmarkView | Models | ios/Features/Models/BenchmarkView.swift | — | — | todo | — |
| FreeweightsBrowseView | Models | ios/Features/Models/FreeweightsBrowseView.swift | — | — | todo | — |
| ModelsTabView | Models | ios/Features/Models/ModelsTabView.swift | — | — | todo | — |
| section: model-lab | Models | — | src/fabric-session/phone/phone-nav.ts (PhoneSection model-lab) | — | todo | — |
| LocalDocumentView | Office | ios/Features/Office/LocalDocumentView.swift | — | — | todo | — |
| OfficeDocumentView | Office | ios/Features/Office/OfficeDocumentView.swift | — | — | todo | — |
| OfficeDocumentsView | Office | ios/Features/Office/OfficeDocumentsView.swift | — | — | todo | — |
| OfficeEditorWebView | Office | ios/Features/Office/OfficeEditorWebView.swift | — | — | todo | — |
| OfficeFloorView | Office | ios/Features/Office/Floor/OfficeFloorView.swift | — | — | todo | — |
| PDFSignView | Office | ios/Features/Office/PDFSignView.swift | — | — | todo | — |
| AndroidScreenPairingView | Phones | ios/Features/Phones/AndroidScreenPairingView.swift | — | — | todo | — |
| AppPhonesSettingsView | Phones | ios/Features/Phones/AppPhonesSettingsView.swift | — | — | todo | — |
| LinkedPhonesSection | Phones | — | src/fabric-session/phone/screens/LinkedPhonesSection.tsx | — | todo | — |
| PhoneLiveView | Phones | ios/Features/Phones/PhoneLiveView.swift | src/fabric-session/phone/screens/LinkedPhoneOverlay.tsx | — | todo | — |
| PhoneTaskCardView | Phones | ios/Features/Phones/PhoneTaskCardView.swift | — | — | todo | — |
| PhonesSheet | Shell | ios/Features/Shell/PhonesSheet.swift | — | — (opens "coming in the next update") | todo | — |
| mode: aci | Shell | — | src/fabric-session/phone/phone-nav.ts (PhoneMode aci) | — | todo | — |
| mode: code | Shell | — | src/fabric-session/phone/phone-nav.ts (PhoneMode code) | — | todo | — |

## Wave G. Settings

| Screen | Area | iOS path | Web path | Android | Status | Proof |
|---|---|---|---|---|---|---|
| BrainCaptureSheet | Brain | ios/Features/Brain/BrainCaptureSheet.swift | — | — | todo | — |
| ConnectorsListView | Connectors | ios/Features/Connectors/ConnectorsListView.swift | — | — | todo | — |
| CustomizeScreen | Customize | ios/Features/Customize/CustomizeScreen.swift | — | — | todo | — |
| section: customize | Customize | — | src/fabric-session/phone/phone-nav.ts (PhoneSection customize) | — | todo | — |
| AccountProfileView | Settings | ios/Features/Settings/AccountProfileView.swift | — | — | todo | — |
| BrainSpikeView | Settings | ios/Features/Settings/BrainSpike/BrainSpikeView.swift | — | — | todo | — |
| CapabilitiesSettingsView | Settings | ios/Features/Settings/CapabilitiesSettingsView.swift | — | — | todo | — |
| CloudComputerBackupsView | Settings | ios/Features/Settings/CloudComputerBackupsView.swift | — | — | todo | — |
| CloudDeployManagerView | Settings | ios/Features/Settings/CloudDeployManagerView.swift | — | — | todo | — |
| CloudInstancesManagerView | Settings | ios/Features/Settings/CloudInstancesManagerView.swift | — | — | todo | — |
| ConnectSheetView | Settings | ios/Features/Settings/Connections/ConnectSheetView.swift | — | — | todo | — |
| ConnectionRowView | Settings | ios/Features/Settings/Connections/ConnectionRowView.swift | — | — | todo | — |
| ConnectionsSettingsView | Settings | ios/Features/Settings/Connections/ConnectionsSettingsView.swift | — | — | todo | — |
| DeleteAccountView | Settings | ios/Features/Settings/DeleteAccountView.swift | — | — | todo | — |
| DeviceAccessSettingsView | Settings | ios/Features/Settings/DeviceAccessSettingsView.swift | — | — | todo | — |
| DevicePairingView | Settings | ios/Features/Settings/DevicePairingView.swift | — | — | todo | — |
| EnterpriseBYOCPanelView | Settings | ios/Features/Settings/EnterpriseBYOCPanelView.swift | — | — | todo | — |
| FilesDashboardView | Settings | ios/Features/Settings/FilesDashboardView.swift | — | — | todo | — |
| FilesKeysView | Settings | ios/Features/Settings/FilesKeysView.swift | — | — | todo | — |
| InfrastructureSettingsView | Settings | ios/Features/Settings/InfrastructureSettingsView.swift | — | — | todo | — |
| McpServersSettingsView | Settings | ios/Features/Settings/McpServersSettingsView.swift | — | — | todo | — |
| MeetingsSettingsView | Settings | ios/Features/Settings/MeetingsSettingsView.swift | — | — | todo | — |
| MemoryDriveHistoryView | Settings | ios/Features/Settings/MemoryDriveHistoryView.swift | — | — | todo | — |
| MemoryDriveView | Settings | ios/Features/Settings/MemoryDriveView.swift | — | — | todo | — |
| MemorySettingsView | Settings | ios/Features/Settings/MemorySettingsView.swift | — | — | todo | — |
| ModelManagementView | Settings | ios/Features/Settings/ModelManagementView.swift | — | — | todo | — |
| NodesManagerView | Settings | ios/Features/Settings/NodesManagerView.swift | — | — | todo | — |
| NotificationsSettingsView | Settings | ios/Features/Settings/NotificationsSettingsView.swift | — | — | todo | — |
| OrganizationAccessView | Settings | ios/Features/Settings/OrganizationAccessView.swift | — | — | todo | — |
| PlatformSettingsView | Settings | ios/Features/Settings/PlatformSettingsView.swift | — | — | todo | — |
| PrewarmManagerView | Settings | ios/Features/Settings/PrewarmManagerView.swift | — | — | todo | — |
| PrivacySettingsView | Settings | ios/Features/Settings/PrivacySettingsView.swift | — | — | todo | — |
| ReplayManagerView | Settings | ios/Features/Settings/ReplayManagerView.swift | — | — | todo | — |
| RuntimeOperationsView | Settings | ios/Features/Settings/RuntimeOperationsView.swift | — | — | todo | — |
| SafariView | Settings | ios/Features/Settings/SafariView.swift | — | — | todo | — |
| SelfHostedSessionView | Settings | ios/Features/Settings/SelfHosted/SelfHostedSessionView.swift | — | — | todo | — |
| SelfHostedSettingsView | Settings | ios/Features/Settings/SelfHosted/SelfHostedSettingsView.swift | — | — | todo | — |
| SettingsScreen | Settings | — | src/fabric-session/phone/screens/SettingsScreen.tsx | — | todo | — |
| SettingsView | Settings | ios/Features/Settings/SettingsView.swift | — | — | todo | — |
| SubagentsSettingsView | Settings | ios/Features/Settings/SubagentsSettingsView.swift | — | — | todo | — |
| TwinSettingsView | Settings | ios/Features/Settings/TwinSettingsView.swift | — | — | todo | — |
| VPSServersManagerView | Settings | ios/Features/Settings/VPSServersManagerView.swift | — | — | todo | — |
| WebhooksSettingsView | Settings | ios/Features/Settings/WebhooksSettingsView.swift | — | — | todo | — |
| section: settings | Settings | — | src/fabric-session/phone/phone-nav.ts (PhoneSection settings) | — | todo | — |

## Wave H. Money + voice

| Screen | Area | iOS path | Web path | Android | Status | Proof |
|---|---|---|---|---|---|---|
| PermissionPrimingSheet | Chat | ios/Features/Chat/Views/PermissionPrimingSheet.swift | — | — | todo (wave H): priming before the RECORD_AUDIO / POST_NOTIFICATIONS system dialog | — |
| MeetingLiveView | Calls | ios/Features/Calls/Meetings/MeetingLiveView.swift | — | — | todo | — |
| MeetingRecapView | Calls | ios/Features/Calls/Meetings/MeetingRecapView.swift | — | — | todo | — |
| MeetingsView | Calls | ios/Features/Calls/Meetings/MeetingsView.swift | src/fabric-session/phone/phone-nav.ts (PhoneSection meetings) | — | todo | — |
| DictationOnboardingSheet | Chat | ios/Features/Chat/Views/DictationOnboardingSheet.swift | — | — | todo | — |
| VoiceCaptionsView | Chat | ios/Features/Chat/Views/Voice/VoiceCaptionsView.swift | — | — | todo | — |
| VoiceModeView | Chat | ios/Features/Chat/Views/Voice/VoiceModeView.swift | — | — | todo | — |
| VoiceSettingsSheet | Chat | ios/Features/Chat/Views/Voice/VoiceSettingsSheet.swift | — | — | todo | — |
| VoiceSpeakerView | Chat | ios/Features/Chat/Views/Voice/VoiceSpeakerView.swift | — | — | todo | — |
| VoiceSummaryCardView | Chat | ios/Features/Chat/Views/Voice/VoiceSummaryCardView.swift | — | — | todo | — |
| BudgetDashboardView | Settings | ios/Features/Settings/BudgetDashboardView.swift | — | — | todo | — |
| ComputeBillingView | Settings | ios/Features/Settings/ComputeBillingView.swift | — | — | todo | — |
| ProductsSettingsView | Settings | ios/Features/Settings/ProductsSettingsView.swift | — | — | todo | — |
| VoiceKeyScannerView | Settings | ios/Features/Settings/VoiceKeyScannerView.swift | — | — | todo | — |
| CloudCreditsSheet | Shared | ios/Features/Shared/CloudCreditsSheet.swift | — | — | todo | — |

## Wave I. System

| Screen | Area | iOS path | Web path | Android | Status | Proof |
|---|---|---|---|---|---|---|
| App Intents -> app shortcuts | System surfaces | ios/SystemSurfaces/Intents | — | — | todo | — |
| Control widgets -> Quick Settings tiles | System surfaces | ios/AllternitWidgets | — | — | todo | — |
| File Provider -> existing DocumentsProvider | System surfaces | ios/AllternitFileProvider | — | — | todo | — |
| Focus filter | System surfaces | ios/SystemSurfaces/Intents/FocusFilter.swift | — | — | N/A: no Android equivalent (Eoj 2026-11-18) | — |
| Live Activities -> ongoing notifications | System surfaces | ios/AllternitWidgets | — | — | todo | — |
| Push notifications | System surfaces | ios/AllternitNotifications | — | — | todo | — |
| Share extension -> Android share target | System surfaces | ios/AllternitShare | — | — | todo | — |
| Widgets -> Glance app widgets | System surfaces | ios/AllternitWidgets | — | — | todo | — |
| iMessage extension | System surfaces | ios/AllternitMessages | — | — | N/A: no Android equivalent (Eoj 2026-11-18) | — |
| LazyScreen | Unmatched web | — | src/fabric-session/phone/screens/LazyScreen.tsx | — | N/A: web lazy-load wrapper, no screen | — |
| PhoneFilesOverlay | Unmatched web | — | src/fabric-session/phone/screens/PhoneFilesOverlay.tsx | — | todo | — |
