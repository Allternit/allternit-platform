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
