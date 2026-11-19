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
| E | Bots | 31 | 10 |
| F | Modes | 76 | 12–14 |
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
| C | TWA → native sign-in handoff (user already signed in on m.allternit.com) | N/A: Eoj 11-19, people sign in once more after the switch |
| C | Continue with Apple retest (needs return URL on Services ID com.allternit.signin — owner) | blocked on owner |
| I | FCM device tokens: backend accepts platform android (step 6 finding c) | todo |
| J | Web / Desktop / Gizzi links to the Play app | todo |
| E | Meet your bots gaps from #2621 (vendor bots, New bot, Import, routine time editor, Week / Month) | built (wave E PR) |
| E | Bot projects open the bot project page; pad editor Your templates (wave D leftovers) | built (wave E PR) |
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
| MeetYourBotsView | Onboarding | ios/Features/Onboarding/MeetYourBotsView.swift | — | feature/shell/firstrun/MeetYourBotsScreen.kt (+ MeetFinish.kt, MeetVisuals.kt) | built (#2621; wave E adds vendor bots, New bot, Import, the routine time editor and Week / Month) | — |
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
| ArtifactDetailsView | Artifacts | ios/Features/Artifacts/Views/ArtifactDetailsView.swift | — | feature/artifacts/ArtifactSheets.kt (ArtifactDetailsScreen) | built (#2629: Preview / Code, Share, Open in browser, Copy; art_ ids open the V2 viewer) | — |
| ArtifactLiveWebView | Artifacts | ios/Features/Artifacts/Views/ArtifactLiveWebView.swift | — | feature/artifacts/ArtifactWebViews.kt (ArtifactLiveWebView) | built (#2629: app pages only; Clerk, editor and download bridges answer only the web origin's main frame via androidx.webkit; Android WebView keeps one storage per app) | — |
| ArtifactPageWebView | Artifacts | ios/Features/Artifacts/Views/ArtifactPageWebView.swift | — | feature/artifacts/ArtifactWebViews.kt (ArtifactPageWebView) | built (#2629: tapped links open outside; later main-frame navigations refused) | — |
| ArtifactStudioView | Artifacts | ios/Features/Artifacts/Views/ArtifactStudioView.swift | — | feature/artifacts/ArtifactStudio.kt | built (#2629: Chat / Edit panes; the mode contract, artifact-session note and own-tools note go out as agent-chat systemPrompt on every turn) | — |
| ArtifactsLibraryView | Artifacts | ios/Features/Artifacts/Views/ArtifactsLibraryView.swift | — | feature/artifacts/ArtifactsLibrary.kt | built (#2629: List / Grid / Deck, filters, studio tiles, card previews for every kind (paper, sheet, slide, code, dashboard, HTML / mermaid / poster snapshots, image, PDF page, video frame), New artifact sheet with Upload PDF / Image / Video, archive / delete; Allternit Office entry built in F4b (#2650)) | — |
| PDFTemplateSheet | Artifacts | ios/Features/Artifacts/Views/PDFTemplateSheet.swift | — | feature/artifacts/ArtifactSheets.kt (PDFTemplateSheet) + assets/pdf-templates.js | built (#2629: same layout JS in an offscreen WebView; Android print dialog saves the PDF) | — |
| SandboxedArtifactWebView | Artifacts | ios/Features/Artifacts/Views/SandboxedArtifactWebView.swift | — | feature/artifacts/ArtifactWebViews.kt (SandboxedArtifactWebView) | built (#2629: deny-all CSP + base-uri / form-action, every navigation and sub-request refused, no DOM storage) | — |
| V2ArtifactEditorView | Artifacts | ios/Features/Artifacts/Views/V2ArtifactEditorView.swift | — | feature/artifacts/V2ArtifactEditor.kt | built (#2629: markdown, doc blocks incl. table cells and image descriptions, tabbed docs with docAddTab / rename / delete, workbook sheets (formulas, Add row / column), simple and block decks) | — |
| V2ArtifactView | Artifacts | ios/Features/Artifacts/Views/V2ArtifactView.swift | — | feature/artifacts/V2Artifact.kt | built (#2629: render by kind, comments, share link, export; Sign PDF and the sectioned Office record built in F4b (#2650); PDF and video open in the web viewer) | — |
| section: artifacts | Artifacts | — | src/fabric-session/phone/phone-nav.ts (PhoneSection artifacts) | feature/shell/WorkspaceShell.kt (Section artifacts-library) | built (#2629) | — |
| CodeProjectDetailView | Projects | ios/Features/Projects/Views/CodeProjectDetailView.swift | — | feature/projects/ProjectDetail.kt (CodeProjectDetailScreen) | built (#2629: folder picked by browsing the computer (v1/path, v1/file/tree through the runtime relay) or typed, like iOS) | — |
| DesignProjectCanvasView | Projects | ios/Features/Projects/Views/DesignProjectCanvasView.swift | — | feature/projects/ProjectDetail.kt (DesignProjectCanvasScreen) | built (#2629: Close does not flush a pending canvas save first; the canvas saves itself) | — |
| FolderCoverView | Projects | ios/Features/Projects/Views/FolderCoverView.swift | — | feature/projects/FolderCover.kt (FolderCover) | built (#2629: symbols drawn as glyphs, not SF Symbols) | — |
| FolderLookSheet | Projects | ios/Features/Projects/Views/FolderLookSheet.swift | — | feature/projects/FolderCover.kt (FolderLookSheet) | built (#2629: colour picker with Grid, Spectrum and hex, matching the iOS ColorPicker sheet) | — |
| NewProjectSheet | Projects | ios/Features/Projects/Views/NewProjectSheet.swift | — | feature/projects/ProjectsList.kt (NewProjectSheet) | built (#2629: bot projects save to the account; the bot project page is wave E) | — |
| PadEditSheet | Projects | ios/Features/Projects/Views/PadEditSheet.swift | — | feature/projects/ProjectPad.kt (PadEditSheet) | built (#2629; wave E adds Your templates and Save as template) | — |
| ProjectDetailView | Projects | ios/Features/Projects/Views/ProjectDetailView.swift | — | feature/projects/ProjectDetail.kt (ProjectDetailScreen) | built (#2629: Overview with coordinator conversation and composer, Delegate pane, Board pane, Details with status, Memory, Add files (Photos / Files), instructions, files and Folder) | — |
| ProjectPadView | Projects | ios/Features/Projects/Views/ProjectPadView.swift | — | feature/projects/ProjectPad.kt (ProjectPadCard) | built (#2629: notes, links, checklist, quick actions, threads; Style tiles drawn like iOS ProjectDashboard Bento with the palette slots) | — |
| ProjectsListView | Projects | ios/Features/Projects/Views/ProjectsListView.swift | — | feature/projects/ProjectsList.kt (ProjectsListScreen) | built (#2629; wave E: bot projects open the bot project page in Bots mode) | — |
| ProjectsScreen | Projects | — | src/fabric-session/phone/screens/ProjectsScreen.tsx | feature/projects/ProjectsList.kt (ProjectsListScreen) | built (#2629) | — |
| section: projects | Projects | — | src/fabric-session/phone/phone-nav.ts (PhoneSection projects) | feature/shell/WorkspaceShell.kt (Section projects) | built (#2629) | — |

## Wave E. Bots

| Screen | Area | iOS path | Web path | Android | Status | Proof |
|---|---|---|---|---|---|---|
| AgentActivityDetailView | AgentActivity | ios/Features/AgentActivity/Views/AgentActivityDetailView.swift | — | feature/bots/BotsMore.kt (AgentActivityDetailScreen) | built (wave E PR) | — |
| AgentActivityListView | AgentActivity | ios/Features/AgentActivity/Views/AgentActivityListView.swift | — | feature/bots/BotsMore.kt (AgentActivityListScreen) | built (wave E PR): tag filter is a sheet, not a menu | — |
| AgentDetailView | Agents | ios/Features/Agents/AgentDetailView.swift | — | feature/bots/Agents.kt (AgentDetailView) | built (wave E PR): avatar editor sheet not ported; desktop take-over waits on wave F | — |
| AgentHubView | Agents | ios/Features/Agents/AgentHubView.swift | — | feature/bots/Agents.kt (AgentHubScreen) | built (wave E PR): opened from Bots options; response style / active bot rows not ported | — |
| AgentIdenticonView | Agents | ios/Features/Agents/AgentIdenticonView.swift | — | feature/bots/BotVisuals.kt (AgentIdenticon) | built (wave E PR) | — |
| BotBrainPickerSheet | Agents | ios/Features/Agents/BotBrainPickerSheet.swift | — | feature/bots/BotForm.kt (BotBrainPickerSheet) | built (wave E PR) | — |
| BotHomeView | Agents | ios/Features/Agents/BotHomeView.swift | — | feature/bots/Agents.kt (BotHomeScreen) | built (wave E PR): desktop preview card waits on wave F | — |
| BotWebhooksSheet | Agents | ios/Features/Agents/BotWebhooksSheet.swift | — | feature/bots/Agents.kt (BotWebhooksSheet) | built (wave E PR) | — |
| BotWorkspaceFilesSheet | Agents | ios/Features/Agents/BotWorkspaceFilesSheet.swift | — | feature/bots/Agents.kt (BotWorkspaceFilesSheet) | built (wave E PR) | — |
| MarketplaceView | Agents | ios/Features/Agents/MarketplaceView.swift | — | feature/bots/Agents.kt (MarketplaceScreen, MarketplaceListingScreen) | built (wave E PR) | — |
| WorkspaceFileEditorView | Agents | ios/Features/Agents/WorkspaceFileEditorView.swift | — | feature/bots/Agents.kt (WorkspaceFileEditorScreen) | built (wave E PR): "Edit with chat" assist (EditWithChat.kt: instruction → streamed revision in a bot session, line diff, Apply / Discard) | — |
| BotAvatarView | Bots | ios/Features/Bots/BotAvatarView.swift | — | feature/bots/BotVisuals.kt (BotAvatar, BotStatusAvatar) | built (wave E PR): sprite still frame, no sprite animation | — |
| BotChatSessionView | Bots | ios/Features/Bots/BotChatSessionView.swift | — | feature/bots/BotChat.kt (BotChatSessionScreen) | built (wave E PR): call button and Background picker not ported | — |
| BotFormSheet | Bots | ios/Features/Bots/CreateBot/BotFormSheet.swift | — | feature/bots/BotForm.kt (NewBotSheet, EditBotSheet) | built (wave E PR): Look editor (BotLook.kt, the web /editor/bot-look page signed in, avatar saved on create/edit, asset sync); voice, starter prompts and "Draft instructions" not ported | — |
| BotHomeTabsView | Bots | ios/Features/Bots/BotHomeTabsView.swift | — | feature/bots/BotPage.kt (BotHomeTabsScreen) | built (wave E PR) | — |
| BotLaunchpadView | Bots | ios/Features/Bots/BotLaunchpadView.swift | — | feature/bots/BotsRoot.kt (BotLaunchpad) | built (wave E PR): To: picker is a sheet | — |
| BotLiveStageView | Bots | ios/Features/Bots/BotLiveStageView.swift | — | feature/bots/BotChat.kt (BotLiveStageScreen) | built (wave E PR) | — |
| BotPageView | Bots | ios/Features/Bots/BotPageView.swift | — | feature/bots/BotPage.kt (BotPageView) | built (wave E PR) | — |
| BotProjectPage | Bots | ios/Features/Bots/BotProjectPage.swift | — | feature/bots/BotsMore.kt (BotProjectPage) | built (wave E PR): Delegate / Factory panes and the canvas open the Projects detail | — |
| BotThreadsView | Bots | ios/Features/Bots/BotThreadsView.swift | — | feature/bots/BotThreads.kt (BotThreadsOverview, BotThreadsPanel, BotThreadTasks) | built (wave E PR) | — |
| BotsModeRootView | Bots | ios/Features/Bots/BotsModeRootView.swift | — | feature/bots/BotsRoot.kt (BotsModeRoot) | built (wave E PR): group chats (GroupChat.kt: list, new, room stage, member turns + vendor members, add members, swipe delete); Office floor built in F4b (#2650): Bots options, Office | — |
| GroupChatViews / VendorGroupTurn | Bots | ios/Features/Bots/GroupChatViews.swift, VendorGroupTurn.swift | — | feature/bots/GroupChat.kt | built (wave E PR): room background picker not ported (same as the bot chat Background picker) | — |
| BotZipImport | Bots | ios/Features/Connect/BotZipImport.swift | — | feature/bots/BotZip.kt + ImportBotSheet | built (wave E PR) | — |
| ImpactMapView | Shared | ios/Features/Shared/ImpactMapView.swift | — | feature/bots/ImpactMap.kt | built (wave E PR): focus graph, Open, Forget, Turn off skill | — |
| BotsScreen | Bots | — | src/fabric-session/phone/screens/BotsScreen.tsx | feature/bots/BotsRoot.kt (BotsModeRoot) | built (wave E PR) | — |
| section: bots | Bots | — | src/fabric-session/phone/phone-nav.ts (PhoneSection bots) | feature/shell/WorkspaceShell.kt (Section "bots") | built (wave E PR) | — |
| DecisionsDeckView | Decisions | ios/Features/Decisions/Views/DecisionsDeckView.swift | — | feature/bots/BotsMore.kt (DecisionsScreen) | built (wave E PR) | — |
| PermissionAskView | Decisions | ios/Features/Decisions/Views/PermissionAskView.swift | — | feature/chat/ChatCards.kt (PermissionAskCard, wave A) | built (#2614, wave A chat card) | — |
| PhoneHoldReviewView | Decisions | ios/Features/Decisions/Views/PhoneHoldReviewView.swift | — | feature/shell/Inbox.kt (PhoneHoldReviewSheet, wave C; Decisions links to it) | built (#2626, wave C Review sheet) | — |
| QuestionCardView | Decisions | ios/Features/Decisions/Views/QuestionCardView.swift | — | feature/chat/ChatCards.kt (QuestionCard; reused by DecisionsScreen) | built (#2614, wave A chat card) | — |
| AgentRunTreeSheet | RunTree | ios/Features/RunTree/Views/AgentRunTreeSheet.swift | — | feature/chat/RunTree.kt (AgentRunTreeSheet, from the tool rail) | built (wave E PR) | — |
| SubagentFaceView | RunTree | ios/Features/RunTree/Views/SubagentFaceView.swift | — | feature/chat/RunTree.kt (SubagentFace) | built (wave E PR) | — |
| ImpactMapView | Shared | ios/Features/Shared/ImpactMapView.swift | — | feature/bots/BotsMore.kt (ImpactMapView) | built (wave E PR): list view, not the node graph | — |
| mode: bots | Shell | — | src/fabric-session/phone/phone-nav.ts (PhoneMode bots) | feature/shell/WorkspaceShell.kt (ShellMode.Bots → BotsModeRoot) | built (wave E PR) | — |

## Wave F. Modes

| Screen | Area | iOS path | Web path | Android | Status | Proof |
|---|---|---|---|---|---|---|
| BotDesktopView | Agents | ios/Features/Agents/Desktop/BotDesktopView.swift | src/fabric-session/phone/screens/PhoneDesktopOverlay.tsx | core/rfb/RemoteScreen.kt + feature/bots/BotDesktop.kt (BotDesktopView, BotComputerScreen) | built (#2644) | — |
| ACIAgentRunView | ACI | ios/Features/ACI/Views/ACIAgentRunView.swift | — | feature/computer/AciRun.kt (AciAgentRunView, AciRunModel) | built (#2644) | — |
| ACIComputerDetailView | ACI | ios/Features/ACI/Views/ACIComputerDetailView.swift | src/fabric-session/phone/screens/PhoneComputerViewOverlay.tsx | core/rfb/ComputerDrive.kt (ComputerDriveScreen) + feature/computer/AciComputers.kt | built (#2644): Desktop path (live screen, take control, start); its Files and Inbox requests rows wait for the Android Files drive Files rows (#2646): feature/computer/AciFiles.kt (roots, folders, Inbox requests; open/save/upload/scan and the wake sheet are not built: left) | — |
| ACIExtensionsView | ACI | ios/Features/ACI/Views/ACIExtensionsView.swift | — | feature/computer/AciExtensions.kt | built (#2646): My Extensions and Marketplace, category filter, detail sheet, Upload; kept on this phone like iOS. Left: none | — |
| ACIMcpAppsView | ACI | ios/Features/ACI/Views/ACIMcpAppsView.swift | — | feature/computer/AciMcpApps.kt + McpOfficialServers.kt | built (#2646): Installed, Discover (catalog + 94 official servers), Build, Add by URL, installed-app sheet (permission mode, tools, screen, Remove). Left: Discover landing shelves/collections and vendor logo marks | — |
| ACITabView | ACI | ios/Features/ACI/Views/ACITabView.swift | — | feature/computer/AciScreen.kt (AciRoot, landing, target picker, nudge) | built (#2644) | — |
| ACIWebBrowserView | ACI | ios/Features/ACI/Views/ACIWebBrowserView.swift | — | feature/computer/AciRun.kt (AciWebBrowser) | built (#2644) | — |
| AciScreen | ACI | — | src/fabric-session/phone/screens/AciScreen.tsx | feature/computer/AciScreen.kt (AciRoot) | built (#2644) | — |
| AppScreenWebView | ACI | ios/Features/ACI/Views/AppScreenWebView.swift | — | feature/computer/AciWeb.kt | built (#2646): app screen in the installed-app sheet, openFull, connect, Clerk token bridge. Left: not checked on a device | — |
| BrowserChatView | ACI | ios/Features/ACI/Views/BrowserChatView.swift | — | feature/shell ChatPage | built (#2646): a browser session opens in the shared chat page under ACI mode; no separate title bar (the shell header is the back control) | — |
| FilesInboxRequestsView | ACI | ios/Features/ACI/Views/Files/FilesInboxRequestsView.swift | — | feature/computer/AciFiles.kt | built (#2646): Inbox requests list, Approve, Reject over the Files drive v2 relay. :app Files client is not a dependency of a feature module, so this has its own relay calls | — |
| MiniAppRuntimeView | ACI | ios/Features/ACI/Views/MiniAppRuntimeView.swift | src/fabric-session/phone/screens/PhoneMiniAppOverlay.tsx | feature/computer/AciMiniApps.kt | built (#2646): web view, or native capabilities when the app has no page | — |
| MiniAppsStoreView | ACI | ios/Features/ACI/Views/MiniAppsStoreView.swift | — | feature/computer/AciMiniApps.kt | built (#2646): search, categories, Open, Pin; opened from MCP Apps (Mini Apps chip) and from the drawer's More menu (gap run A, shell/ShellDrawer.kt, WorkspaceShell.kt) | — |
| TeachView | ACI | ios/Features/ACI/Views/TeachView.swift | src/fabric-session/phone/screens/PhoneTeachOverlay.tsx | feature/computer/AciTeach.kt | built (#2646): the drawer Teach row and the Teach nudge open it; skills, editor, Run, Import .har + Analyze, Delete. Left: Schedule sheet, business profile, striking recorded actions | — |
| section: aci | ACI | — | src/fabric-session/phone/phone-nav.ts (PhoneSection aci) | feature/shell/WorkspaceShell.kt (mcp-apps / browser-extensions / computers rows → AciRoot) | built (#2644) | — |
| AutomationHubView | Automation | ios/Features/Automation/Views/AutomationHubView.swift | — | `android/feature/automation/src/main/java/com/allternit/feature/automation/AutomationHub.kt` | built (#2641) | — |
| AutomationScheduleGraphView | Automation | ios/Features/Automation/Views/AutomationScheduleGraphView.swift | — | `android/feature/automation/src/main/java/com/allternit/feature/automation/AutomationCalendar.kt` (AutomationGraph) | built (#2641) | — |
| AutomationSchedulerView | Automation | ios/Features/Automation/Views/AutomationSchedulerView.swift | — | `android/feature/automation/src/main/java/com/allternit/feature/automation/AutomationCalendar.kt` | built (#2641) | — |
| AutomationTaskDetailView | Automation | ios/Features/Automation/Views/AutomationTaskDetailView.swift | — | `android/feature/automation/src/main/java/com/allternit/feature/automation/AutomationHub.kt` (AutomationTaskDetailSheet) | built (#2641) | — |
| AutomationTasksListView | Automation | ios/Features/Automation/Views/AutomationTasksListView.swift | — | `android/feature/automation/src/main/java/com/allternit/feature/automation/AutomationHub.kt` (TaskCard list) | built (#2641) | — |
| AutomationTasksPhoneView | Automation | ios/Features/Automation/Views/AutomationTasksPhoneView.swift | — | `android/feature/automation/src/main/java/com/allternit/feature/automation/AutomationHub.kt` (AutomationRoot) | built (#2641) | — |
| AutomationWizardSheet | Automation | ios/Features/Automation/Views/AutomationWizardSheet.swift | — | `android/feature/automation/src/main/java/com/allternit/feature/automation/AutomationSheets.kt` | built (#2641) | — |
| CreateAutomationTaskSheet | Automation | ios/Features/Automation/Views/CreateAutomationTaskSheet.swift | — | `android/feature/automation/src/main/java/com/allternit/feature/automation/AutomationSheets.kt` | built (#2641) | — |
| CreateLoopSheet | Automation | ios/Features/Automation/Views/CreateLoopSheet.swift | — | `android/feature/automation/src/main/java/com/allternit/feature/automation/AutomationLocal.kt` | built (#2641) | — |
| CreateRoutineSheet | Automation | ios/Features/Automation/Views/CreateRoutineSheet.swift | — | `android/feature/automation/src/main/java/com/allternit/feature/automation/AutomationLocal.kt` | built (#2641) | — |
| LoopDetailView | Automation | ios/Features/Automation/Views/LoopDetailView.swift | — | `android/feature/automation/src/main/java/com/allternit/feature/automation/AutomationLocal.kt` | built (#2641) | — |
| LoopsListView | Automation | ios/Features/Automation/Views/LoopsListView.swift | — | `android/feature/automation/src/main/java/com/allternit/feature/automation/AutomationLocal.kt` | built (#2641) | — |
| RoutineDetailView | Automation | ios/Features/Automation/Views/RoutineDetailView.swift | — | `android/feature/automation/src/main/java/com/allternit/feature/automation/AutomationLocal.kt` | built (#2641) | — |
| RoutinesListView | Automation | ios/Features/Automation/Views/RoutinesListView.swift | — | `android/feature/automation/src/main/java/com/allternit/feature/automation/AutomationLocal.kt` | built (#2641) | — |
| ScheduledTaskSheet | Automation | ios/Features/Automation/Views/ScheduledTaskSheet.swift | — | `android/feature/automation/src/main/java/com/allternit/feature/automation/AutomationSheets.kt` | built (#2641) | — |
| TasksBoardScreen | Automation | ios/Features/Automation/Views/TasksBoardScreen.swift | — | `android/feature/automation/src/main/java/com/allternit/feature/automation/TasksBoard.kt` | built (#2641) | — |
| section: automation | Automation | — | src/fabric-session/phone/phone-nav.ts (PhoneSection automation) | `android/feature/automation/src/main/java/com/allternit/feature/automation/AutomationHub.kt` (drawer row automation-tasks) | built (#2641) | — |
| section: tasks | Automation | — | src/fabric-session/phone/phone-nav.ts (PhoneSection tasks) | `android/feature/automation/src/main/java/com/allternit/feature/automation/TasksBoard.kt` (Board tab of the hub) | built (#2641) | — |
| ChangesetReviewSheet | Code | ios/Features/Code/ChangesetReviewSheet.swift | — | feature/code/CodeDiffs.kt (ChangesetReviewSheet, PendingChangesBanner) | built (#2640) | — |
| CodeEditorView | Code | ios/Features/Code/CodeEditor/CodeEditorView.swift | — | feature/code/CodeFiles.kt (CodeEditorScreen: line numbers, highlight, Save through files/write) | built (#2640) | — |
| CodeModeView | Code | ios/Features/Code/CodeModeView.swift | — | feature/code/CodeHome.kt (CodeHome, CodeRouteScreen, CodeThreadTools) + feature/shell/WorkspaceShell.kt (Code card in the mode fan opens it) | built (#2640) | — |
| CodeOverviewSheet | Code | ios/Features/Code/CodeOverviewSheet.swift | — | feature/code/CodeHome.kt (CodeOverviewScreen, ThreadActionsSheet, PermissionsScreen) | built (#2640) | — |
| CodeRuntimePairingView | Code | ios/Features/Code/CodeRuntimePairingView.swift | — | feature/code/CodeHome.kt (CodeComputerScreen) | built (#2640) | — |
| CodeThreadView | Code | ios/Features/Code/CodeThreadView.swift | — | feature/shell/ChatPage.kt on a code session + feature/code/CodeDiffs.kt (PendingChangesBanner) | built (#2640) | — |
| DevServerPortsView | Code | ios/Features/Code/DevPreview/DevServerPortsView.swift | — | feature/code/CodeDevServer.kt (DevServerPortsScreen) | built (#2640) | — |
| DevServerPreviewView | Code | ios/Features/Code/DevPreview/DevServerPreviewView.swift | — | feature/code/CodeDevServer.kt (DevServerPreviewScreen: WebView, requests fetched through the relay) | built (#2640) | — |
| FileBrowserDirectoryView | Code | ios/Features/Code/FileBrowser/FileBrowserDirectoryView.swift | — | feature/code/CodeFiles.kt (FileBrowserDirectory) | built (#2640) | — |
| FileBrowserView | Code | ios/Features/Code/FileBrowser/FileBrowserView.swift | — | feature/code/CodeFiles.kt (FileBrowserScreen) | built (#2640) | — |
| FileDetailView | Code | ios/Features/Code/FileBrowser/FileDetailView.swift | — | feature/code/CodeFiles.kt (FileDetailScreen) | built (#2640) | — |
| FileDiffDetailView | Code | ios/Features/Code/DiffViewer/FileDiffDetailView.swift | — | feature/code/CodeDiffs.kt (FileDiffDetailScreen) | built (#2640) | — |
| PhoneCodeSessionsView | Code | — | src/fabric-session/phone/screens/PhoneCodeSessionsView.tsx | feature/code/CodeHome.kt (CodeOverviewScreen) | built (#2640) | — |
| PhoneCodeTerminal | Code | — | src/fabric-session/phone/screens/PhoneCodeTerminal.tsx | feature/code/CodeTerminal.kt (TerminalScreen, PtySession, PaneMirror) | built (#2640) | — |
| SessionDiffListView | Code | ios/Features/Code/DiffViewer/SessionDiffListView.swift | — | feature/code/CodeDiffs.kt (SessionDiffScreen) | built (#2640) | — |
| TerminalSessionView | Code | ios/Features/Code/TerminalSessionView.swift | — | feature/code/CodeTerminal.kt (TerminalScreen), feature/code/VtScreen.kt (native VT emulator like SwiftTerm, no WebView) | built (#2640; gap run A2 #2667: alternate screen, scroll regions, application cursor and keypad, SGR 256 and truecolour, cursor visibility, DSR/DA replies, line drawing; key bar arrows follow the app mode, PgUp and PgDn) | — |
| section: code | Code | — | src/fabric-session/phone/phone-nav.ts (PhoneSection code) | feature/shell/WorkspaceShell.kt (mode code: CodeHome under the composer) | built (#2640) | — |
| ConnectWizardView | Connect | ios/Features/Connect/ConnectWizardView.swift | — | feature/phones/Connect.kt (GiveBotPhoneSheet + ConnectWizardSheet, Email flow; phone number, Telegram and OAuth flows left) | built (#2648) | — |
| VendorConnectView | Connect | ios/Features/Connect/VendorConnectView.swift | — | feature/phones/Connect.kt (VendorConnectScreen, VendorProductScreen) | built (#2648) | — |
| CoworkSessionWorkspaceView | Cowork | ios/Features/Cowork/Views/CoworkSessionWorkspaceView.swift | — | feature/cowork/CoworkScreens.kt (CoworkSessionScreen) | built (#2648) | — |
| CoworkTaskDetailView | Cowork | ios/Features/Cowork/Views/CoworkTaskDetailView.swift | — | feature/cowork/CoworkScreens.kt (CoworkTaskDetail) | built (#2648) | — |
| CoworkTasksListView | Cowork | ios/Features/Cowork/Views/CoworkTasksListView.swift | — | feature/cowork/CoworkScreens.kt (CoworkTasksScreen) | built (#2648) | — |
| CoworkWorkspaceView | Cowork | ios/Features/Cowork/Views/CoworkWorkspaceView.swift | — | feature/cowork/CoworkScreens.kt (CoworkLaunchpad) | built (#2648) | — |
| CreateCoworkTaskSheet | Cowork | ios/Features/Cowork/Views/CreateCoworkTaskSheet.swift | — | feature/cowork/CoworkScreens.kt (CreateCoworkTaskSheet) | built (#2648) | — |
| FabricApprovalsView | Cowork | ios/Features/Cowork/Views/FabricApprovalsView.swift | — | feature/cowork/CoworkScreens.kt (FabricApprovalsScreen) | built (#2648) | — |
| BenchmarkView | Models | ios/Features/Models/BenchmarkView.swift | — | feature/models/ModelsScreens.kt (BenchmarkScreen) | built (#2648) | — |
| FreeweightsBrowseView | Models | ios/Features/Models/FreeweightsBrowseView.swift | — | feature/models/ModelsScreens.kt (catalog, faces, ModelDetailSheet) + ModelsData.kt | built (#2648) | — |
| ModelsTabView | Models | ios/Features/Models/ModelsTabView.swift | — | feature/models/ModelsScreens.kt (ModelsRoot) | built (#2648) | — |
| section: model-lab | Models | — | src/fabric-session/phone/phone-nav.ts (PhoneSection model-lab) | feature/models/ModelsScreens.kt (ModelsRoot, the Models section) | built (#2648) | — |
| LocalDocumentView | Office | ios/Features/Office/LocalDocumentView.swift | — | feature/office/LocalDocument.kt | built (#2650: ACTION_VIEW on pdf/markdown/text/Word/Excel opens it; PDF pages via PdfRenderer, text selectable, Word/Excel hand off to the system viewer, Share) | — |
| OfficeDocumentView | Office | ios/Features/Office/OfficeDocumentView.swift | — | feature/office/OfficeScreens.kt (OfficeDocumentScreen) | built (#2650: sectioned docs, slides, sheet grid and PDF pages, Interactive editor, 428 no-computer and signed-out states; opened from a v2 artifact's Open in Office) | — |
| OfficeDocumentsView | Office | ios/Features/Office/OfficeDocumentsView.swift | — | feature/office/OfficeScreens.kt (OfficeDocumentsScreen) | built (#2650: Docs/Sheets/Slides/PDFs merged newest first, pull to refresh, empty/offline states, Sign PDF; opened from the library's Allternit Office button) | — |
| OfficeEditorWebView | Office | ios/Features/Office/OfficeEditorWebView.swift | — | feature/office/OfficeEditorWebView.kt | built (#2650: same /docs|/sheets|/slides|/pdf/:id route, same fetch and XHR bearer bridge over a reply-capable message handler (allternitAuth), first navigation carries the Bearer) | — |
| OfficeFloorView | Office | ios/Features/Office/Floor/OfficeFloorView.swift | — | feature/office/OfficeFloor.kt | built (#2650: the same web facility iOS frames (3dfacility.allternit.com, ?embed=facility) in a WebView host page, fed with the roster, projects and statuses; select/answer/open push the bot chat, panel messages send into the main chat and stream back; visitor identity kept by the app. Gap run A2 #2667: computers are fed (Compute Row names), panel sends use the selected model, and Go to chat works from a document opened in the Office list. The Bots options sheet and bot routes open it; the drawer rail has no Office button on Android yet) | — |
| PDFSignView | Office | ios/Features/Office/PDFSignView.swift | — | feature/office/PdfSign.kt | built (#2650: signature pad, PdfRenderer first page, signature composed 200x80 pt bottom-right into a new one-page PDF, copy kept in app files, saved with the system picker and Save to library; opened from a PDF artifact's Sign PDF and the Office documents header) | — |
| AndroidScreenPairingView | Phones | ios/Features/Phones/AndroidScreenPairingView.swift | — | feature/phones/PhonesScreens.kt (AndroidScreenPairingScreen) | built (#2648) | — |
| AppPhonesSettingsView | Phones | ios/Features/Phones/AppPhonesSettingsView.swift | — | feature/phones/PhonesScreens.kt (AppPhonesSettingsScreen; the pairing handshake stays in :app AppPhoneLink) | built (#2648) | — |
| LinkedPhonesSection | Phones | — | src/fabric-session/phone/screens/LinkedPhonesSection.tsx | feature/computer/AciComputers.kt (AciLinkedPhonesSection, F3; now opens PhoneLiveView) | built (#2648) | — |
| PhoneLiveView | Phones | ios/Features/Phones/PhoneLiveView.swift | src/fabric-session/phone/screens/LinkedPhoneOverlay.tsx | feature/phones/PhoneLive.kt | built (#2648) | — |
| PhoneTaskCardView | Phones | ios/Features/Phones/PhoneTaskCardView.swift | — | feature/phones/PhonesScreens.kt (PhoneTaskCard, read only) | built (#2648) | — |
| PhonesSheet | Shell | ios/Features/Shell/PhonesSheet.swift | — | feature/phones/PhonesScreens.kt (PhonesSheet; the drawer Phones button opens it) | built (#2648) | — |
| mode: aci | Shell | — | src/fabric-session/phone/phone-nav.ts (PhoneMode aci) | feature/shell/WorkspaceShell.kt (ShellMode.Aci → AciRoot) | built (#2644) | — |
| mode: code | Shell | — | src/fabric-session/phone/phone-nav.ts (PhoneMode code) | feature/code/ (CodeModeRoot, wave F1) | built (#2640) | — |

## Wave G. Settings

| Screen | Area | iOS path | Web path | Android | Status | Proof |
|---|---|---|---|---|---|---|
| BrainCaptureSheet | Brain | ios/Features/Brain/BrainCaptureSheet.swift | — | feature/settings/OperatorPages.kt (BrainCapturePage) | built (#2655) | Saved on this phone; the git push to the brain remote is not on Android |
| ConnectorsListView | Connectors | ios/Features/Connectors/ConnectorsListView.swift | — | feature/settings/CustomizePages.kt (ConnectorsRoot) | built (#2655) | — |
| CustomizeScreen | Customize | ios/Features/Customize/CustomizeScreen.swift | — | feature/settings/CustomizePages.kt (CustomizeRoot) | built (#2655) | Plugin marketplace Sources and Publish pages not built |
| section: customize | Customize | — | src/fabric-session/phone/phone-nav.ts (PhoneSection customize) | feature/shell/WorkspaceShell.kt (customize section) + feature/settings/CustomizePages.kt | built (#2655) | — |
| AccountProfileView | Settings | ios/Features/Settings/AccountProfileView.swift | — | feature/settings/AccountPages.kt (ProfilePage) | built (#2653) | org switching not built |
| BrainSpikeView | Settings | ios/Features/Settings/BrainSpike/BrainSpikeView.swift | — | feature/settings/OperatorPages.kt (BrainPage) | built (#2655) | The git proof run is not on Android; shows the capture queue |
| CapabilitiesSettingsView | Settings | ios/Features/Settings/CapabilitiesSettingsView.swift | — | feature/settings/CustomizePages.kt (CapabilitiesPage) | built (#2655) | — |
| CloudComputerBackupsView | Settings | ios/Features/Settings/CloudComputerBackupsView.swift | — | feature/settings/ResourcePages.kt (CloudBackupsPage) | built (#2655) | Computer export (zip) not built |
| CloudDeployManagerView | Settings | ios/Features/Settings/CloudDeployManagerView.swift | — | feature/settings/ResourcePages.kt (CLOUD_DEPLOY) | built (#2655) | Live status polling after create is a manual Refresh |
| CloudInstancesManagerView | Settings | ios/Features/Settings/CloudInstancesManagerView.swift | — | feature/settings/ResourcePages.kt (CLOUD_INSTANCES) | built (#2655) | — |
| ConnectSheetView | Settings | ios/Features/Settings/Connections/ConnectSheetView.swift | — | feature/shell/firstrun/ConnectSheets.kt (ConnectSheetHost, reused) | built (#2653) | Keys, plan sign-in and on-computer sheets |
| ConnectionRowView | Settings | ios/Features/Settings/Connections/ConnectionRowView.swift | — | feature/shell/firstrun/FirstRunSetupScreen.kt (ConnectionRow, now internal) | built (#2653) | — |
| ConnectionsSettingsView | Settings | ios/Features/Settings/Connections/ConnectionsSettingsView.swift | — | feature/shell/firstrun/SettingsConnections.kt (ConnectionsSettingsScreen) | built (#2653) | seven groups on the setup connect kit; voice keys wait for Voice (part 2) |
| DeleteAccountView | Settings | ios/Features/Settings/DeleteAccountView.swift | — | feature/settings/AccountPages.kt (DeleteAccountPage), core/auth AllternitAuth.deleteAccount | built (#2653) | same Clerk user.delete() as iOS; never run while building |
| DeviceAccessSettingsView | Settings | ios/Features/Settings/DeviceAccessSettingsView.swift | — | feature/settings/DevicePages.kt (DeviceAccessPage) | built (#2653) | Android permissions per kind; no Shortcuts or keyboard sections |
| DevicePairingView | Settings | ios/Features/Settings/DevicePairingView.swift | — | feature/settings/DevicePages.kt (DevicePairingPage) | built (#2653) | — |
| EnterpriseBYOCPanelView | Settings | ios/Features/Settings/EnterpriseBYOCPanelView.swift | — | feature/settings/ResourcePages.kt (BYOC) | built (#2655) | Usage tab and credential test not built |
| FilesDashboardView | Settings | ios/Features/Settings/FilesDashboardView.swift | — | feature/settings/OperatorPages.kt (FilesPage) | built (#2655) | Shares, guests and kept-offline need the Files engine |
| FilesKeysView | Settings | ios/Features/Settings/FilesKeysView.swift | — | feature/settings/OperatorPages.kt (FilesKeysPage) | built (#2655) | Approve, decline and rotate need the Files crypto engine |
| InfrastructureSettingsView | Settings | ios/Features/Settings/InfrastructureSettingsView.swift | — | feature/settings/OperatorPages.kt (InfrastructurePage) | built (#2655) | — |
| McpServersSettingsView | Settings | ios/Features/Settings/McpServersSettingsView.swift | — | feature/settings/ManagePages.kt (McpPage) | built (#2653) | list, connect, disconnect; adding a server is not built |
| MeetingsSettingsView | Settings | ios/Features/Settings/MeetingsSettingsView.swift | — | feature/settings/OperatorPages.kt (MeetingsPage) | built (#2655) | Per-calendar disconnect not built |
| MemoryDriveHistoryView | Settings | ios/Features/Settings/MemoryDriveHistoryView.swift | — | feature/settings/MemoryPages.kt (MemoryHistoryPage) | built (#2653) | diff view and restore are not built |
| MemoryDriveView | Settings | ios/Features/Settings/MemoryDriveView.swift | — | feature/settings/MemoryPages.kt (MemoryDrivePage) | built (#2653) | purge and write are not built |
| MemorySettingsView | Settings | ios/Features/Settings/MemorySettingsView.swift | — | feature/settings/MemoryPages.kt (MemoryPage) | built (#2653) | memories can be forgotten; editing a memory and About you are not built |
| ModelManagementView | Settings | ios/Features/Settings/ModelManagementView.swift | — | feature/settings/ManagePages.kt (ModelsPage) | built (#2653) | provider keys are managed on Connections |
| NodesManagerView | Settings | ios/Features/Settings/NodesManagerView.swift | — | feature/settings/ResourcePages.kt (NODES) | built (#2655) | Join token screen not built |
| NotificationsSettingsView | Settings | ios/Features/Settings/NotificationsSettingsView.swift | — | feature/settings/AccountPages.kt (NotificationsPage) | built (#2653) | event choices, critical-alert texts and quiet hours; the system permission and Live Activities rows are iOS-only |
| OrganizationAccessView | Settings | ios/Features/Settings/OrganizationAccessView.swift | — | feature/settings/AccountPages.kt (OrganizationPage) | built (#2653) | — |
| PlatformSettingsView | Settings | ios/Features/Settings/PlatformSettingsView.swift | — | feature/settings/OperatorPages.kt (PlatformPage) | built (#2655) | — |
| PrewarmManagerView | Settings | ios/Features/Settings/PrewarmManagerView.swift | — | feature/settings/OperatorPages.kt (PrewarmPage) | built (#2655) | — |
| PrivacySettingsView | Settings | ios/Features/Settings/PrivacySettingsView.swift | — | feature/settings/AccountPages.kt (PrivacyPage) | built (#2653) | Face ID lock row not built (needs the biometric library) |
| ReplayManagerView | Settings | ios/Features/Settings/ReplayManagerView.swift | — | feature/settings/OperatorPages.kt (REPLAY) | built (#2655) | — |
| RuntimeOperationsView | Settings | ios/Features/Settings/RuntimeOperationsView.swift | — | feature/settings/OperatorPages.kt (RuntimeOpsPage) | built (#2655) | Execution mode switch not built |
| SafariView | Settings | ios/Features/Settings/SafariView.swift | — | feature/settings/SettingsKit.kt (openLink: Custom Tab) | built (#2653) | honours Open links in |
| SelfHostedSessionView | Settings | ios/Features/Settings/SelfHosted/SelfHostedSessionView.swift | — | feature/settings/OperatorPages.kt (SelfHostedSessionPage) | built (#2655) | Live event stream not built |
| SelfHostedSettingsView | Settings | ios/Features/Settings/SelfHosted/SelfHostedSettingsView.swift | — | feature/settings/OperatorPages.kt (SelfHostedPage) | built (#2655) | — |
| SettingsScreen | Settings | — | src/fabric-session/phone/screens/SettingsScreen.tsx | feature/settings/SettingsRoot.kt | built (#2653) | part 2 replaces the placeholders |
| SettingsView | Settings | ios/Features/Settings/SettingsView.swift | — | feature/settings/SettingsRoot.kt (SettingsRoot, RootList, AdvancedPage) | built (#2653) | Accounts card shows the person and organization; switching organizations and other remembered accounts is not built. Customize and part-2 pages show a placeholder naming the screen |
| SubagentsSettingsView | Settings | ios/Features/Settings/SubagentsSettingsView.swift | — | feature/settings/ManagePages.kt (SubagentsPage) | built (#2653) | — |
| TwinSettingsView | Settings | ios/Features/Settings/TwinSettingsView.swift | — | feature/settings/OperatorPages.kt (TwinPage) | built (#2655) | — |
| VPSServersManagerView | Settings | ios/Features/Settings/VPSServersManagerView.swift | — | feature/settings/ResourcePages.kt (VPS_SERVERS) | built (#2655) | Connection test not built |
| WebhooksSettingsView | Settings | ios/Features/Settings/WebhooksSettingsView.swift | — | feature/settings/ManagePages.kt (WebhooksPage) | built (#2653) | — |
| section: settings | Settings | — | src/fabric-session/phone/phone-nav.ts (PhoneSection settings) | feature/shell WorkspaceShell.kt (ShellPage.Settings -> SettingsRoot) | built (#2653) | drawer avatar and the first-run fix open it |

## Wave H. Money + voice

| Screen | Area | iOS path | Web path | Android | Status | Proof |
|---|---|---|---|---|---|---|
| PermissionPrimingSheet | Chat | ios/Features/Chat/Views/PermissionPrimingSheet.swift | — | android/feature/chat/.../voice/VoiceSheets.kt (PermissionPrimingSheet, rememberPrimedPermission) | built (#2663): microphone priming before voice mode; camera/photos need none on Android (TakePicturePreview and the photo picker show no system dialog); the notifications sheet is built but the empty-chat notifications card that opens it is not ported | — |
| MeetingLiveView | Calls | ios/Features/Calls/Meetings/MeetingLiveView.swift | — | android/feature/meetings/.../MeetingsScreens.kt (MeetingLiveScreen), MeetingsData.kt (MeetingLiveSession, SSE) | built (#2663) | — |
| MeetingRecapView | Calls | ios/Features/Calls/Meetings/MeetingRecapView.swift | — | android/feature/meetings/.../MeetingsScreens.kt (MeetingRecapScreen) | built (#2663) | — |
| MeetingsView | Calls | ios/Features/Calls/Meetings/MeetingsView.swift | src/fabric-session/phone/phone-nav.ts (PhoneSection meetings) | android/feature/meetings/.../MeetingsScreens.kt (MeetingsScreen, PasteLinkSheet), MeetingsData.kt (MeetingsStore, MeetingsClient) | built (#2663): drawer Meetings button with the `meetings` flag; Connect Zoom / Google calendar connect, meeting notifications and Live Activity are not ported (the empty state links to Settings › Meetings) | — |
| DictationOnboardingSheet | Chat | ios/Features/Chat/Views/DictationOnboardingSheet.swift | — | android/feature/chat/.../voice/VoiceSheets.kt (DictationOnboardingSheet) | built (#2663) | — |
| VoiceCaptionsView | Chat | ios/Features/Chat/Views/Voice/VoiceCaptionsView.swift | — | android/feature/chat/.../voice/VoiceMode.kt (VoiceCaptions) | built (#2663): bot stage not ported, so captions are not shown in voice mode yet | — |
| VoiceModeView | Chat | ios/Features/Chat/Views/Voice/VoiceModeView.swift | — | android/feature/chat/.../voice/VoiceMode.kt (VoiceModeView, VoiceModeViewModel, VoiceHost, VoiceQuickBar), VoiceEngine.kt | built (#2663): system route (SpeechRecognizer + TextToSpeech); the Sherpa on-device engine, BYOK OpenAI/ElevenLabs sessions and the bot stage are not on Android | — |
| VoiceSettingsSheet | Chat | ios/Features/Chat/Views/Voice/VoiceSettingsSheet.swift | — | android/feature/chat/.../voice/VoiceSheets.kt (VoiceSettingsSheet) | built (#2663): Live mode look section not ported | — |
| VoiceSpeakerView | Chat | ios/Features/Chat/Views/Voice/VoiceSpeakerView.swift | — | android/feature/chat/.../voice/VoiceMode.kt (VoiceSpeakerRing) | built (#2663): ring style only; blocks/ascii/glow/orb looks not ported | — |
| VoiceSummaryCardView | Chat | ios/Features/Chat/Views/Voice/VoiceSummaryCardView.swift | — | android/feature/chat/.../voice/VoiceMode.kt (VoiceSummaryCard) | built (#2663) | — |
| BudgetDashboardView | Settings | ios/Features/Settings/BudgetDashboardView.swift | — | android/feature/settings/.../ProductsPages.kt (BudgetDashboard, in RuntimeOpsPage) | built (#2657) | — |
| ComputeBillingView | Settings | ios/Features/Settings/ComputeBillingView.swift | — | android/feature/settings/.../BillingPages.kt | built (#2657) | — |
| ProductsSettingsView | Settings | ios/Features/Settings/ProductsSettingsView.swift | — | android/feature/settings/.../ProductsPages.kt (ProductsPage) | built (#2657) | — |
| VoiceKeyScannerView | Settings | ios/Features/Settings/VoiceKeyScannerView.swift | — | android/feature/settings/.../VoicePage.kt (VoicePage, VoiceKeyHandoffSheet, QrScanner, VoiceHandoffSession, VoiceKeys), core/files/.../VoiceHandoffCrypto.kt | built (#2669): CameraX + ML Kit QR, P-256 ECDH + HKDF-SHA256 + AES-256-GCM as iOS, relay poll + QR 2, vendor check, Keystore-sealed keys; Save to my Allternit account and the ElevenLabs voice picker not ported; the key drives voice in GS-D | — |
| CloudCreditsSheet | Shared | ios/Features/Shared/CloudCreditsSheet.swift | — | android/feature/billing/.../CloudCreditsSheet.kt | built (#2657) | — |

## Wave I. System

| Screen | Area | iOS path | Web path | Android | Status | Proof |
|---|---|---|---|---|---|---|
| App Intents -> app shortcuts | System surfaces | ios/SystemSurfaces/Intents | — | android/app/src/nativeapp/res/xml/shortcuts.xml, feature/system/StatusSync.kt (SystemLinks, publishShortcuts), feature/shell/MainActivity.kt (readSystemLink) | built (#2665): the four iOS shortcuts (Ask a bot, Hand off, Needs you, Call main bot) open native screens through allternit:// links; Ask <bot> shortcuts for the first two roster bots are dynamic. The iOS Siri phrases, Spotlight entities and the memory, schedule and phone intents have no Android shortcut; Call main bot opens the main bot's chat in Bots mode like iOS .callMain (gap run A, BotsLaunch.openMain). The drawer Phones button shows the total unread (PhonesUnread). A tapped push opens the exact bot thread; Approve, Deny and Reply act on the computer the push names. The model picker opens Cloud credits for a locked Cloud row with its price line | — |
| Control widgets -> Quick Settings tiles | System surfaces | ios/AllternitWidgets | — | android/feature/system/Tiles.kt, app/src/nativeapp/AndroidManifest.xml | built (#2665): six TileServices, one per iOS control (Call main bot, Ask a bot, Hand off a task, Show what needs me, Run a routine, Pause all bots); Pause toggles in place through PUT /api/v1/bots/pause. Run a routine opens Automation Tasks (a tile has no routine picker) | — |
| File Provider -> existing DocumentsProvider | System surfaces | ios/AllternitFileProvider | — | android/app/src/main/java/com/allternit/app/files/AllternitDocumentsProvider.kt | built (#2665): the provider is declared in src/main, so the nativeapp flavor ships it (authority ${applicationId}.files) next to ShareToComputerActivity and FilesSignInActivity. Its sign-in still uses the Custom Tab hand-off with the Keystore-sealed token: the native Clerk session cannot mint a Files token without a new cloud-api route, so the second sign-in stays | — |
| Focus filter | System surfaces | ios/SystemSurfaces/Intents/FocusFilter.swift | — | — | N/A: no Android equivalent (Eoj 2026-11-18) | — |
| Live Activities -> ongoing notifications | System surfaces | ios/AllternitWidgets | — | android/feature/push/LiveRunNotification.kt | built (#2665): one ongoing notification for the focus bot (name, step, progress bar, +N others, Approve / Deny when approveInline), driven by the status snapshot on every push and app foreground; the Meeting and Loop lock-screen layouts and the Dynamic Island have no Android surface | — |
| Push notifications | System surfaces | ios/AllternitNotifications | android/feature/push/AllternitPush.kt, PushNotifications.kt, AllternitMessagingService.kt, NotificationActionReceiver.kt | I1 | built (#2664) | FCM data messages; channels per iOS category with the iOS buttons; tap opens the inbox item, Decisions, Settings or Bots; private bodies (#2669, PrivateBodies.kt): fetched by event id with the Keystore-sealed read token as the iOS notification extension, MessagingStyle for communication pushes |
| Share extension -> Android share target | System surfaces | ios/AllternitShare | — | android/feature/system/ShareActivity.kt, app/src/nativeapp/AndroidManifest.xml | built (#2665): ACTION_SEND / SEND_MULTIPLE for text, links, images and files open "Share to a bot" (bot picker from the roster, note, one-time consent, upload through the chat uploads, one message). The TWA GET share target is removed for nativeapp only | — |
| Widgets -> Glance app widgets | System surfaces | ios/AllternitWidgets | — | android/feature/system/StatusWidget.kt, StatusSnapshot.kt, StatusSync.kt | built (#2665): the status widget (small 2x2: working / needs you / focus step and the five empty states; wide 4x2 adds Ask, Call, Needs you and Run routine, because Android has no flip). Lock-screen accessory families have no Android widget | — |
| iMessage extension | System surfaces | ios/AllternitMessages | — | — | N/A: no Android equivalent (Eoj 2026-11-18) | — |
| LazyScreen | Unmatched web | — | src/fabric-session/phone/screens/LazyScreen.tsx | — | N/A: web lazy-load wrapper, no screen | — |
| PhoneFilesOverlay | Unmatched web | — | src/fabric-session/phone/screens/PhoneFilesOverlay.tsx | feature/settings (Files drive screens, #2646 / #2655) | built (#2665): covered by the native Files rows (drive browser, share and Inbox): android/feature/settings Files pages (#2646 / #2655) and app/files/FilesUi.kt | — |
