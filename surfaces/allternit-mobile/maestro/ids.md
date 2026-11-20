# Shared Maestro test ids

One list, same strings on both platforms. Android sets `Modifier.testTag(id)` with
`testTagsAsResourceId = true` on the root composable so Maestro sees the tag as the
resource id. iOS sets `.accessibilityIdentifier(id)`. Flows select with `id: "<id>"`.
Flows run with `-e APP_ID=<bundle id>` (iOS `com.allternit.mobile`, Android
`com.allternit.app`); see `maestro/audit/run-audit.sh --platform ios|android`.

When a flow gains an `id:` selector, add its row here in the same change.

## Ids the flows use (39 total, 0 missing on iOS)

| id | flow(s) | iOS file that sets it | Android |
| --- | --- | --- | --- |
| `loginContinueEmail` | 01 | `ios/App/AllternitApp.swift:432` | `android/feature/onboarding/.../SignInScreen.kt` |
| `loginHeadline` | 01 | `ios/App/AllternitApp.swift:352` | `android/feature/onboarding/.../SignInScreen.kt` |
| `loginSkipAuth` | 01 | `ios/App/AllternitApp.swift:456` | `android/feature/onboarding/.../SignInScreen.kt` |
| `chatNewChat` | 02 | `ios/Features/Chat/Views/ChatView.swift:63` | `android/feature/shell/.../ShellScreens.kt` |
| `composerInput` | 02 | `ios/Features/Chat/Composer/ComposerView.swift:355` | `android/feature/shell/.../Composer.kt` |
| `phoneMenuButton` | 02, 04, 05, 07, 08, 09, 19 | `ios/Features/Shell/PhoneHeader.swift:192` | `android/feature/shell/.../ModeFan.kt` |
| `modeDial` | 03, 08, 09 | `ios/Features/Shell/ModeDialView.swift:433` | `android/feature/shell/.../ShellDrawer.kt` |
| `modeFanCardbots` | 03 | `ios/Features/Shell/ModeDialView.swift:370` (built as "modeFanCard\(mode.rawValue)") | `android/feature/shell/.../ModeFan.kt` |
| `drawerRowprojects` | 04 | `ios/Features/History/Views/HistorySidebarView.swift:408` (rail row id projects) | `android/feature/shell/.../ShellDrawer.kt` |
| `drawerRowartifacts-library` | 05 | `ios/Features/History/Views/HistorySidebarView.swift:408` (rail row id artifacts-library) | `android/feature/shell/.../ShellDrawer.kt` |
| `automationTabloop` | 06, 18 | `ios/Features/Automation/Views/AutomationTasksPhoneView.swift:1248` (TaskTab.loop) | Modifier.testTag (step 7+) |
| `automationTabroutine` | 06 | `ios/Features/Automation/Views/AutomationTasksPhoneView.swift:1248` (TaskTab.routine) | Modifier.testTag (step 7+) |
| `drawerRowautomation-tasks` | 06, 18 | `ios/Features/History/Views/HistorySidebarView.swift:408` (rail row id automation-tasks) | `android/feature/shell/.../ShellDrawer.kt` |
| `modelsTitle` | 07 | `ios/Features/Models/ModelsTabView.swift:77` | Modifier.testTag (step 7+) |
| `modeFanCardcode` | 08 | `ios/Features/Shell/ModeDialView.swift:370` (rawValue code) | `android/feature/shell/.../ModeFan.kt` |
| `modeFanCardaci` | 09 | `ios/Features/Shell/ModeDialView.swift:370` (rawValue aci) | `android/feature/shell/.../ModeFan.kt` |
| `drawerSettings` | 10 | `ios/Features/History/Views/HistorySidebarView.swift:839` | `android/feature/shell/.../ShellDrawer.kt` |
| `plusSheetAddFiles` | 11 | `ios/Features/Chat/Views/ComposerPlusSheet.swift:302` | `android/feature/shell/.../PlusSheet.kt` |
| `plusSheetCamera` | 11 | `ios/Features/Chat/Views/ComposerPlusSheet.swift:300` | `android/feature/shell/.../PlusSheet.kt` |
| `plusSheetClose` | 11 | `ios/Features/Chat/Views/ComposerPlusSheet.swift:259` | `android/feature/shell/.../PlusSheet.kt` |
| `plusSheetConnectors` | 11 | `ios/Features/Chat/Views/ComposerPlusSheet.swift:356` | `android/feature/shell/.../PlusSheet.kt` |
| `plusSheetGithub` | 11 | `ios/Features/Chat/Views/ComposerPlusSheet.swift:304` | `android/feature/shell/.../PlusSheet.kt` |
| `plusSheetPhotos` | 11 | `ios/Features/Chat/Views/ComposerPlusSheet.swift:273` | `android/feature/shell/.../PlusSheet.kt` |
| `modelPickerSearch` | 12 | `ios/Features/Chat/Views/ModelPickerSheet.swift:101` | `android/feature/shell/.../ModelPicker.kt` |
| `agentSheetDone` | 13 | `ios/Features/Chat/Views/AgentSelectionSheet.swift:70` | Modifier.testTag (step 7+) |
| `agentSheetTitle` | 13 | `ios/Features/Chat/Views/AgentSelectionSheet.swift:27` | Modifier.testTag (step 7+) |
| `newProjectClose` | 14 | `ios/Features/Projects/Views/NewProjectSheet.swift:42` | Modifier.testTag (step 7+) |
| `newProjectCreate` | 14 | `ios/Features/Projects/Views/NewProjectSheet.swift:59` | Modifier.testTag (step 7+) |
| `newProjectName` | 14 | `ios/Features/Projects/Views/NewProjectSheet.swift:175` | Modifier.testTag (step 7+) |
| `newProjectTitle` | 14 | `ios/Features/Projects/Views/NewProjectSheet.swift:45` | Modifier.testTag (step 7+) |
| `memoryLoadError` | 15 | `ios/Features/Settings/MemorySettingsView.swift:117` | Modifier.testTag (step 7+) |
| `platformLanguage` | 16 | `ios/Features/Settings/PlatformSettingsView.swift:50` | Modifier.testTag (step 7+) |
| `platformTimezone` | 16 | `ios/Features/Settings/PlatformSettingsView.swift:58` | Modifier.testTag (step 7+) |
| `brainSpikeList` | 17 | `ios/Features/Settings/BrainSpike/BrainSpikeView.swift:87` | Modifier.testTag (step 7+) |
| `createLoopClose` | 18 | `ios/Features/Automation/Views/CreateLoopSheet.swift:53` | Modifier.testTag (step 7+) |
| `createLoopCommand` | 18 | `ios/Features/Automation/Views/CreateLoopSheet.swift:68` | Modifier.testTag (step 7+) |
| `createLoopSubmit` | 18 | `ios/Features/Automation/Views/CreateLoopSheet.swift:149` | Modifier.testTag (step 7+) |
| `createLoopTitle` | 18 | `ios/Features/Automation/Views/CreateLoopSheet.swift:39` | Modifier.testTag (step 7+) |
| `loopsNew` | 18 | `ios/Features/Automation/Views/LoopsListView.swift:117` | Modifier.testTag (step 7+) |

## Chat ids (wave A, no flow uses them yet)

Same strings on both platforms where iOS sets one. The Android-only rows tag views iOS draws
without an identifier; iOS should take the same id when a flow needs it.

| id | iOS file that sets it | Android |
| --- | --- | --- |
| `chatJumpToBottom` | `ios/Features/Chat/Views/ChatView.swift:549` | `android/feature/chat/.../ChatScreen.kt` |
| `chatNewChat` | `ios/Features/Chat/Views/ChatView.swift:63` | `android/feature/shell/.../ChatPage.kt` (and the home) |
| `recentRow<sessionId>` | `ios/Features/History/Views/HistorySidebarView.swift:764` | `android/feature/shell/.../ShellDrawer.kt` (opens the chat) |
| `recentsSeeAll` | `ios/Features/History/Views/HistorySidebarView.swift:510` | `android/feature/shell/.../ShellDrawer.kt` |
| `drawerEmptyNew` | `ios/Features/History/Views/HistorySidebarView.swift:593` | `android/feature/shell/.../ShellDrawer.kt` |
| `agentSheetTitle`, `agentSheetDone` | `ios/Features/Chat/Views/AgentSelectionSheet.swift:27,70` | none: the iOS sheet has no caller, so neither app shows it |
| `chatScreen` | none | `android/feature/shell/.../ChatPage.kt` |
| `chatFeed` | none | `android/feature/chat/.../ChatScreen.kt` |
| `chatEditingBanner` | none | `android/feature/chat/.../ChatScreen.kt` |
| `chatMenuDelete` | none | `android/feature/shell/.../ChatPage.kt` |
| `composerStop` | none (label "Stop") | `android/feature/shell/.../Composer.kt` |
| `chatComputeGate`, `chatComputeGateLink`, `chatComputeGatePlans`, `chatComputeGateCredits` | none | `android/feature/chat/.../ComputeGate.kt` |
| `chatMenushareLink` | none (iOS ChatMenuPopover row "Share link") | `android/feature/shell/.../ChatPage.kt` |
| `threadShareSheet` | none | `android/feature/chat/.../ThreadShare.kt` |
| `plusSheetSketch` | none (iOS PlusRow "Sketch") | `android/feature/shell/.../PlusSheet.kt` |
| `sketchSheet`, `sketchCancel`, `sketchUndo`, `sketchAttach` | none | `android/feature/shell/.../SketchSheet.kt` |

## First-run ids (wave B, no flow uses them yet)

| id | iOS file that sets it | Android |
| --- | --- | --- |
| `conn-row-<connectionId>` | `ios/Features/Settings/Connections/ConnectionRowView.swift` | `android/feature/shell/.../firstrun/FirstRunSetupScreen.kt` (ConnectionRow) |
| `conn-key-<keyProviderId>` | `ios/Features/Settings/Connections/ConnectSheetView.swift:186` | `android/feature/shell/.../firstrun/ConnectSheets.kt` |
| `conn-disclosure`, `conn-disclosure-accept`, `conn-disclosure-decline` | `ios/Features/Settings/Connections/ConnectSheetView.swift:650-658` | `android/feature/shell/.../firstrun/ConnectSheets.kt` |
| `setup-add-computer` | `ios/Features/Onboarding/Setup/SetupKitSteps.swift` | `android/feature/shell/.../firstrun/FirstRunSetupScreen.kt` |
| `setup-splash`, `setup-checklist`, `setup-checklist-continue`, `setup-open-allternit` | none | `android/feature/shell/.../firstrun/FirstRunGate.kt`, `ConnectedScreen.kt` |
| `meet-your-bots`, `meet-build-team`, `meet-skip-for-now`, `meet-template-<templateId>`, `meet-main-name`, `meet-job-text`, `meet-start-job`, `meet-done` | none (iOS MeetYourBotsView sets no ids) | `android/feature/shell/.../firstrun/MeetYourBotsScreen.kt` |

## Shell rest ids (wave C, no flow uses them yet)

| id | iOS file that sets it | Android |
| --- | --- | --- |
| `quickTaskInput`, `quickTaskStart`, `quickTaskOpen` | `ios/Features/Shell/QuickTaskSheet.swift` | `android/feature/shell/.../StartSheets.kt` |
| `shellSearchField` | `ios/Features/Shell/ShellSearchView.swift` | `android/feature/shell/.../ShellSearch.kt` |
| `phoneOverview`, `recentsOptions`, `overviewLayout`, `overviewSelectBar` | `ios/Features/Shell/SessionOverviewView.swift` | `android/feature/shell/.../SessionOverview.kt` |
| `drawerNewPill` (hold for `drawerNew<optionId>`: `continue-cli`, `scheduled`, `quick-task`) | `ios/Features/History/Views/HistorySidebarView.swift` (drawerNewPill) | `android/feature/shell/.../ShellDrawer.kt` |
| `inboxSheet`, `inboxDone`, `inboxList`, `inboxEmpty`, `inboxRow<id>`, `inboxPrimary<id>`, `inboxDeny<id>`, `inboxSwipePrimary<id>`, `inboxSnooze<id>`, `inboxDismiss<id>`, `inboxReview`, `inboxReviewDone`, `inboxReviewApprove`, `inboxReviewDeny`, `inboxReviewRetry`, `drawerInboxCount` | none (iOS InboxSheet sets no ids) | `android/feature/shell/.../Inbox.kt`, `ShellDrawer.kt` |
| `quickTaskSheet`, `quickTaskClose`, `quickTaskStatus`, `nativeSessionPicker`, `nativeSessionCancel`, `nativeSessionSearch`, `nativeHarness<id>`, `nativeSession<harness:id>` | none | `android/feature/shell/.../StartSheets.kt` |
| `overviewLayout<list\|grid\|deck>`, `overviewSelect`, `overviewView<All\|Pinned\|Archived>`, `overviewRow<id>`, `overviewCard<id>`, `overviewRename`, `overviewSearch`, `overviewNew`, `overviewEmpty`, `overviewDeleteConfirm`, `overviewFilter`, `overviewGroupAll`, `overviewTeleport` | none | `android/feature/shell/.../SessionOverview.kt` |
| `shellSearch`, `shellSearchClose`, `shellSearchEmpty`, `searchSession<id>`, `searchProject<id>`, `searchBot<id>` | none | `android/feature/shell/.../ShellSearch.kt` |
| `thinkingSlider`, `thinkingLevel<levelId>` | none | `android/feature/shell/.../ThinkingSlider.kt` |

Launch arguments: the iOS flows pass `skip-auth`, `sidebar` and `-open-tab <id>` as launch arguments.
Android debug builds read the same keys as intent extras (Maestro's `launchApp: arguments:`), with
leading dashes stripped and string or boolean values: `skip-auth`, `open-tab`, `sidebar`,
`shell-mode`, `open-inbox`, `open-quick-task`, `open-continue-cli`. Release builds ignore them.

Notes:

- Ids ending in a dynamic suffix (`modeFanCard<mode>`, `drawerRow<railId>`, `automationTab<tab>`)
  are built from a raw value; Android must build them the same way (`"modeFanCard" + mode.raw`),
  not hard-code a different case. Rail ids keep the web's rail-tab ids (`artifacts-library`, `automation-tasks`).
- `modeDialOption{bots,code,aci}` no longer exist: the calm pass replaced the dial options with the mode fan
  (`modeFanCard<mode>`); flows 03/08/09 now use those.
- Empty-string identifiers (`plusSheetClose` on the Back button, `newProjectName` on the description field)
  mean "no id" on that variant.

## Flow status

All 20 flows select by `id:` except: 20_aci_browser_chat (no selectors, screenshots only), the iOS-only
"Dismiss notifications prompt" taps (a system alert, no app view to tag), and a few text asserts where the
text is the thing under test (03: error copy and "Response style"; 10: "Settings").
Stale selectors fixed on the way: 01 (gate has no "Sign In"/"Create Account" now), 07 (Models left the
drawer rail, opened with `-open-tab models`), 12 (no Done button on the picker), 19 (status-filter sheet is gone;
asserts the Code header menu button).

## iOS-only steps

The optional "Dismiss notifications prompt" tap (11 flows) is wrapped in
`runFlow: when: platform: iOS`. Each carries a `TODO(android step N)` for the Android
POST_NOTIFICATIONS dialog.

## Projects and artifacts ids (wave D, no flow uses them yet)

| id | iOS file that sets it | Android |
| --- | --- | --- |
| `newProjectClose`, `newProjectTitle`, `newProjectCreate`, `newProjectName` | `ios/Features/Projects/Views/NewProjectSheet.swift` | `android/feature/projects/.../ProjectsList.kt` |
| `newProjectSheet`, `newProjectMode<chat\|code\|design\|bot>`, `newProjectDescription`, `newProjectSubmit` | none | `android/feature/projects/.../ProjectsList.kt` |
| `projectsList`, `projectsSearch`, `projectsSearchField`, `projectsFilter`, `projectsFilterSheet`, `projectsCounts`, `projectsNew`, `projectsEmpty`, `projectsError`, `projectsExplainer`, `projectsExplainerOk`, `projectCard<mode:id>`, `projectMore<mode:id>`, `projectActions`, `projectAction<Open\|Pin\|Edit\|Archive\|Delete>`, `projectDeleteConfirm`, `projectsShow<mode\|all>`, `projectsSort<Recent\|Name\|Created>`, `projectsArchived<true\|false>`, `overviewLayout<list\|grid\|deck>` | none (iOS ProjectsListView sets no ids) | `android/feature/projects/.../ProjectsList.kt` |
| `projectDetail`, `projectBack`, `projectTitle`, `projectPin`, `projectCustomize`, `projectTab<Overview\|Board\|Details>`, `projectStartThread`, `projectNewChat`, `projectThread<id>`, `projectInstructions`, `projectInstructionsSave`, `projectLook<Settings\|EditPad\|EditDetails\|Archive\|Delete>`, `projectDetailsSheet`, `projectDetailsClose`, `projectDetailsSave`, `projectDetailsName`, `projectDetailsDescription` | none | `android/feature/projects/.../ProjectDetail.kt`, `ProjectsList.kt` |
| `folderLookSheet`, `folderLookCancel`, `folderLookSave`, `folderPreset<id>`, `folderColor<hex>`, `folderPaper<hex>`, `folderTab<hex>`, `folderArt<art>`, `folderSymbol<name>`, `folderCoverText`, `folderChooseImage`, `folderFont<font>`, `folderPlace<place>`, `folderPattern<pattern>`, `folderFrame<frame>`, `folderEdge<edge>`, `folderSheets<n>`, `folderTabLabel`, `folderShowMode<true\|false>` | none | `android/feature/projects/.../FolderCover.kt` |
| `projectPadCard`, `projectPadView<pad\|list>`, `projectPadEdit`, `padTile<tileId>`, `padNotes`, `padLinkName`, `padLinkUrl`, `padChecklistAdd`, `padEditSheet`, `padEditDone`, `padEditTile<tileId>`, `padSize<tileId><S\|M\|L\|W>`, `padAdd<tileId>`, `padColors<id>`, `padTemplate<id>` | none | `android/feature/projects/.../ProjectPad.kt` |
| `codeProjectDetail`, `codeProjectFolder`, `codeProjectNewSession`, `codeProjectSession<id>`, `codeProjectFolderSheet`, `codeProjectFolderPath`, `codeProjectFolderSave`, `designProjectCanvas`, `designCanvasClose` | none | `android/feature/projects/.../ProjectDetail.kt` |
| `artifactsLibrary`, `artifactsSearch`, `artifactsSearchField`, `artifactsFilter`, `artifactsFilterSheet`, `artifactsTile<kind>`, `artifactsConnectTools`, `artifactsClearFilters`, `artifactsEmpty`, `artifactsError`, `artifactsNewRow`, `artifactCard<id>`, `artifactMore<id>`, `artifactActions`, `artifactAction<Open\|Archive\|Delete>`, `artifactDeleteConfirm`, `artifactsShow<Saved\|Archive>`, `artifactsOwner<all\|mine\|shared>`, `artifactsKind<kind\|all>` | none (iOS ArtifactsLibraryView sets no ids) | `android/feature/artifacts/.../ArtifactsLibrary.kt` |
| `v2ArtifactView`, `v2ArtifactBack`, `v2ArtifactComments`, `v2ArtifactMore`, `v2ArtifactMoreSheet`, `v2ArtifactShare`, `v2ArtifactGoToChat`, `v2ArtifactOpenBrowser`, `v2ArtifactViewComments`, `v2ArtifactEdit`, `v2ArtifactExport`, `v2ArtifactArchive`, `v2ArtifactDelete`, `v2ArtifactDeleteConfirm`, `v2ArtifactError`, `v2ArtifactExportSheet`, `v2ArtifactSaveFile`, `v2ArtifactPdfTemplate`, `v2ArtifactCopy`, `v2Slides`, `v2SheetGrid` | none | `android/feature/artifacts/.../V2Artifact.kt` |
| `docAddTab` | `ios/Features/Artifacts/Views/V2ArtifactEditorView.swift:234` | `android/feature/artifacts/.../V2ArtifactEditor.kt` |
| `v2ArtifactEditor`, `v2EditorCancel`, `v2EditorSave`, `v2EditorMarkdown`, `v2EditorBlock<path>`, `v2EditorCell<r:c>`, `v2EditorAddRow`, `v2EditorAddColumn`, `v2EditorSlide<Title\|Bullets\|Notes><n>`, `v2EditorSlideText<slide.block>`, `docTab<id>`, `docTabName` | none | `android/feature/artifacts/.../V2ArtifactEditor.kt` |
| `artifactStudio`, `artifactStudioClose`, `artifactStudio<Chat\|Edit>`, `artifactStudioDescribe` | none | `android/feature/artifacts/.../ArtifactStudio.kt` |
| `artifactDetails`, `artifactDetailsBack`, `artifactDetailsShare`, `artifactDetailsMore`, `artifactDetailsPreview`, `artifactDetailsCode`, `artifactDetailsOpenBrowser`, `artifactDetailsCopy`, `artifactComments`, `artifactCommentsClose`, `artifactCommentInput`, `artifactCommentSend`, `pdfTemplateSheet`, `pdfTemplateClose`, `pdfTemplateMake`, `pdfTemplate<id>`, `pdfTheme<id>`, `pdfVariant<id>`, `pdfPaper<letter\|a4>` | none | `android/feature/artifacts/.../ArtifactSheets.kt` |
| `artifactSandboxWeb`, `artifactPageWeb`, `artifactLiveWeb`, `artifactLiveError` | none | `android/feature/artifacts/.../ArtifactWebViews.kt` |
| `chatArtifactCard<id>` | none | `android/feature/chat/.../MessageRow.kt` |
| `artifactsNew`, `newArtifactSheet`, `newArtifact<kind>`, `artifactsUpload<Pdf\|Image\|Video>`, `newArtifactConnectors` | none (iOS uses the "New artifact" menu label) | `android/feature/artifacts/.../ArtifactUpload.kt`, `ArtifactsLibrary.kt` |
| `colorPickerSheet`, `colorPickerCancel`, `colorPickerDone`, `colorPicker<Grid\|Spectrum>`, `colorGrid<hex>`, `colorSpectrum`, `colorHue`, `colorHex`, `folderColor`, `folderPaper`, `folderTab`, `padColorMain`, `padColorAccent` | none | `android/feature/projects/.../ColorPicker.kt` |
| `projectTabDelegate`, `projectBoard`, `projectDelegate`, `delegateAuto`, `delegateNewTask`, `delegateAssignSheet`, `delegateAssign<botId>`, `projectConversation`, `projectComposerInput`, `projectComposerSend`, `projectServerThread<id>`, `projectStatusCard`, `projectStatusSheet`, `projectViewMemory`, `projectMemorySheet`, `projectAddFiles`, `projectAddInstructions`, `projectAddFilesSheet`, `projectAddPhotos`, `projectAddFilesFiles`, `projectChooseFolder`, `projectFolderPicker`, `projectFolderTyped`, `projectFolderUse`, `projectFolderRow<name>` | none | `android/feature/projects/.../ProjectDetail.kt`, `ProjectCoordinator.kt` |

## Bots ids (wave E)

iOS Bots mode sets few ids (`botsOptionsThreads`, `botsOptionsOffice`, `botsOptionsPause`, `botsOptionsConnections`, `hubTab<tab>`); Android keeps those strings and adds its own. No flow uses them yet.

| id | iOS file that sets it | Android |
| --- | --- | --- |
| `botsOptionsThreads`, `botsOptionsPause`, `botsOptionsConnections` | `ios/Features/Bots/BotsModeRootView.swift` | `android/feature/bots/.../BotsRoot.kt` |
| `hubTab<Agents\|Sessions\|Workspace>` | `ios/Features/Agents/AgentHubView.swift` | `android/feature/bots/.../Agents.kt` |
| `botsModeRoot`, `botsSearch`, `botsSearchField`, `botsOptions`, `botsOptionsSheet`, `botsOptionsHub`, `botsOptionsDecisions`, `botsOptionsActivity`, `botsLayout<List\|Grid\|Deck>`, `botsPausedBanner`, `botsRoster<bot:id\|group:id>`, `botsCard<key>`, `botsNewBot`, `botsNewBotCard`, `botsLaunchHeadline`, `botsLaunchTo`, `botsLaunchToClear`, `botsLaunchHint`, `botsToSheet`, `botsToTeam`, `botsTo<botId>`, `botsToVendor`, `botsThreadsSheet`, `botsTemplateError`, `botsBack`, `botsLoading`, `botsNotFound` | none | `android/feature/bots/.../BotsRoot.kt` |
| `botChatScreen`, `botChip`, `botChipFace`, `botThreadMenu`, `botThreadMenuSheet`, `botMenu<Computer\|Stage\|Threads\|Show\|Approve\|Decline\|Resolve\|Pin\|Edit\|Settings\|Delete>`, `botDeck<Thread\|Threads\|Tasks\|Bot>`, `botChatStage`, `botStageNeed<id>`, `botStageApprove<id>`, `botStageDecline<id>`, `botDeleteConfirm`, `botActionError`, `botLiveStage`, `botStageComputer`, `botStageTray`, `botStageFiles`, `botStagePhotos`, `botStageLink`, `botStageLinkAdd`, `botStageNote`, `botStage<Plan\|Do\|Ask>` | none | `android/feature/bots/.../BotChat.kt` |
| `botPage`, `botPageScreen`, `botPage<Edit\|Workspace\|Model\|Threads\|Tasks\|Routines\|Artifacts\|Memory\|Computer\|Channels>`, `botHomeTabs`, `botHomeTab<Threads\|Artifacts\|Routines\|Memory\|Config>`, `botHomeMainChat`, `botArtifacts`, `botArtifact<id>`, `botRoutines`, `botRoutine<id>`, `botMemory`, `botMemorySearch`, `botMemoryDraft`, `botMemoryAdd`, `botMemoryForget<id>`, `botChannels`, `botChannel<accountId>`, `botChannelEmail`, `botComputerScreen`, `botGroupScreen` | none | `android/feature/bots/.../BotPage.kt` |
| `botThreadsOverview`, `botThreadsWaiting`, `botThreadsSearch`, `botThreadsSearchField`, `botThreadsLayout`, `botThreadsLayoutSheet`, `botThreadsLayout<List\|Grid\|Deck>`, `botThread<id>`, `botThreadRow<id>`, `botThreadsGroup<group>`, `botThreadReopen<id>`, `botTasks`, `botTaskFold<id>` | none | `android/feature/bots/.../BotThreads.kt` |
| `routineScheduleEditor`, `routineScheduleCancel`, `routineScheduleSave`, `routineScheduleLabel`, `routineScheduleTime`, `routineRepeat<kind>`, `routineEvery<min>`, `routineUntil<hour>`, `routineDay<0-6>`, `routineDom<1-28>` | none | `android/feature/bots/.../RoutineSchedule.kt` |
| `newBotSheet`, `editBotSheet`, `botFormCancel`, `botFormSave`, `botFormName`, `botFormAccent<hex>`, `botFormTagline`, `botFormWelcome`, `botFormPrompt`, `botFormMore`, `botFormCategory<id>`, `botFormDescription`, `botFormTool<id>`, `botFormDesktop`, `botFormDesktop<preset>`, `botFormBrain<mode>`, `botFormHarness<id>`, `botFormNativeSession`, `botFormUhp`, `botFormKnowledge<id>`, `botFormModel`, `botFormProvider<id>`, `botFormHarnessMode<id>`, `botFormAdvanced`, `botFormIterLess`, `botFormIterMore`, `botFormTemperature`, `botFormVendor`, `botFormImport`, `botFormTemplates`, `botModelPicker`, `botModel<id>`, `botModelSave`, `agentTemplateSheet`, `agentTemplate<id>`, `agentTemplateConfirm`, `agentTemplateCreate`, `vendorConnectSheet`, `vendor<product>`, `vendorProductSheet`, `vendorConnect`, `vendorField<key>`, `vendorMakeMain`, `importBotSheet`, `importSource<file\|url>`, `importChooseFile`, `importLink`, `importReadLink`, `importBotRun` | none | `android/feature/bots/.../BotForm.kt` |
| `agentHub`, `hubDiscover`, `hubCreate`, `hubSearch`, `hubNewFromTemplate`, `hubMarketplace`, `hubAgent<id>`, `hubDelete<id>`, `hubDeleteConfirm`, `hubSession<id>`, `hubWorkspace<id>`, `agentDetail`, `agentDetailScreen`, `agent<EditLook\|SystemPrompt\|Greeting\|Model\|WorkspaceFiles\|Behavior\|Webhooks\|Publish>`, `agentPromptSheet`, `agentGreetingSheet`, `agentGreetingSave`, `agentPrompt<n>`, `agentBehaviorSheet`, `botHome`, `botHome<Chat\|RunTask\|RunFirst\|TasksRetry\|Artifacts\|Automation\|Connections\|Computer\|Settings>`, `botRunTaskSheet`, `botRunTaskPrompt`, `botRunTaskRun`, `marketplace`, `marketplaceSearch`, `marketplaceListing<id>`, `marketplaceDetail`, `marketplaceInstall`, `marketplaceRate`, `marketplaceRateSheet`, `marketplaceStars<n>`, `marketplaceReview`, `marketplaceRateSave`, `publishAgentSheet`, `publish<Title\|Category\|Tags\|Description>`, `publishAgentSave`, `botWorkspaceFiles`, `botWorkspaceNew`, `botWorkspaceFile<path>`, `botWorkspaceCreate<path>`, `botNewWorkspaceFile`, `botNewWorkspaceCreate`, `botNewFileCat<label>`, `botNewFileName`, `workspaceFileEditor`, `workspaceFileSave`, `workspaceFileText`, `workspaceFileConflict`, `workspaceFileReload`, `workspaceFileKeepMine`, `botWebhooksSheet` | none | `android/feature/bots/.../Agents.kt` |
| `agentActivityList`, `agentActivityFilter`, `agentActivityFilterSheet`, `agentActivityFilter<id>`, `agentActivitySearch`, `agentActivity<threadId>`, `agentActivityDetail`, `agentActivityReview`, `agentActivity<Approve\|Reject>`, `agentActivityReply`, `agentActivitySend`, `decisionsScreen`, `decisionsPrev`, `decisionsNext`, `decisionsEmpty`, `decisionsRetry`, `decisionsInbox`, `approval<id>`, `approval<Approve\|Reject><id>`, `impactMap`, `impactKind<kind>`, `impactItem<id>`, `impactForget<id>`, `impactForgetConfirm`, `botProjectPage`, `botProjectActions`, `botProjectTitle`, `botProjectBot<id>`, `botProjectAddBot`, `botProjectActionsSheet`, `botProject<Edit\|Settings\|Favorite\|Archive\|Delete>`, `botProjectEditSheet`, `botProjectEditSave`, `botProjectEditTitle`, `botProjectEditDetails`, `botProjectDeleteConfirm`, `botProjectThread` | none | `android/feature/bots/.../BotsMore.kt` |
| `toolRailAgents`, `agentRunTree`, `agentRunTreeDone`, `agentRunTreeTotals`, `runNode<id>` | none | `android/feature/chat/.../ChatCards.kt`, `RunTree.kt` |
| `padYourTemplatesLoading`, `padYourTemplatesRetry`, `padSaveTemplate`, `padTemplateName` | none | `android/feature/projects/.../ProjectPad.kt` |
| `meet-routine-<id>`, `meet-routines-<Day\|Week\|Month>` | none | `android/feature/shell/firstrun/MeetYourBotsScreen.kt` |
| `codeHome`, `codeFolderName`, `codeHomeNoComputer`, `codeHomeConnect`, `codeToolTerminal`, `codeToolFiles`, `codeToolChanges`, `codeToolComputer`, `codeEmpty`, `codeSeeAll`, `codeSession-<id>`, `codeSessionMore-<id>`, `codeRoute`, `codeNoComputer`, `codeOverviewBack`, `codeOverviewSearch`, `codeOverviewList`, `codeFilter-<all\|Status>`, `codeThreadSheet`, `codeThreadName`, `codeThread<Rename\|Reviewed\|Archive>`, `codePermission-<DEFAULT\|AUTO\|PLAN\|SAFE>`, `codePermissionsBack`, `codeComputerBack`, `codeComputerList`, `codeComputerAuto`, `codeComputer-<id>`, `codeComputerConnect`, `codeThreadTools`, `codeToolsSheet`, `codeTool-<changes\|permissions\|files\|terminal\|overview\|computer>`, `codePendingBanner`, `codeChangesetSheet`, `codeChangeset<Cancel\|Reject\|Always\|Approve\|Error>`, `codeChangesBack`, `codeChangesList`, `codeChangesRefresh`, `codeChangeRow-<file>`, `codeFileDiffBack`, `codeDiffView`, `codeFilesBack`, `codeFileList`, `codeFileRow-<name>`, `codeFileBack`, `codeFileModified`, `codeFileEdit`, `codeFileText`, `codeEditorBack`, `codeEditorSave`, `codeEditorField`, `codeEditorError`, `codePortsBack`, `codePortsList`, `codePortsRefresh`, `codePortsError`, `codePortRow-<port>`, `codePreviewClose`, `codePreviewRefresh`, `codePreviewWeb`, `codePreviewError`, `codeTerminal`, `codeTerminalBack`, `codeTerminalTab-<n>`, `codeTerminalBot-<slug>`, `codeTerminalNew`, `codeTerminalPorts`, `codeTerminalClose`, `codeTerminalStart`, `codeTerminalOffline`, `codeTerminalConnect`, `codeTerminalRetry`, `codeTerminalReconnect`, `codeTerminalBotRetry`, `codeTerminalScreen`, `codeTerminalInput`, `codeTerminalKey-<label>` | none (iOS has no ids for Code yet) | `android/feature/code/.../*.kt` |
