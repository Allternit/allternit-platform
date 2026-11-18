# Shared Maestro test ids

One list, same strings on both platforms. Android sets `Modifier.testTag(id)` with
`testTagsAsResourceId = true` on the root composable so Maestro sees the tag as the
resource id. iOS sets `.accessibilityIdentifier(id)`. Flows select with `id: "<id>"`.
Flows run with `-e APP_ID=<bundle id>` (iOS `com.allternit.mobile`, Android
`com.allternit.app`); see `maestro/audit/run-audit.sh --platform ios|android`.

When a flow gains an `id:` selector, add its row here in the same change.

## Ids the flows use (5 total, 0 missing on iOS)

| id | flow(s) | iOS file that sets it today | Android: `testTag` to set |
| --- | --- | --- | --- |
| `modeDial` | 03_agent_hub, 08_code, 09_aci | `ios/Features/Shell/ModeDialView.swift:41` | `modeDial` |
| `modeDialOptionbots` | 03_agent_hub | `ios/Features/Shell/ModeDialView.swift:127` (built as `"modeDialOption\(option.rawValue)"`) | `modeDialOptionbots` |
| `modeDialOptioncode` | 08_code | same line, rawValue `code` | `modeDialOptioncode` |
| `modeDialOptionaci` | 09_aci | same line, rawValue `aci` | `modeDialOptionaci` |
| `drawerSettings` | 10_settings | `ios/Features/History/Views/HistorySidebarView.swift:826` | `drawerSettings` |

Note: the three `modeDialOption*` ids are generated from the mode's raw value, so Android
must build them the same way (`"modeDialOption" + mode.raw`), not hard-code a different case.

## Flows that select by visible text only (fragile)

These match on user-visible strings (for example "Sign In", "Message Allternit...",
"Open sidebar", "New chat"), so a copy change or a locale change breaks them on either
platform. Move each to an `id:` selector (and add the row above) as its screen is built on Android.

01_login_gate, 02_home_chat, 04_projects, 05_artifacts, 06_automation, 07_models,
11_chat_composer_plus, 12_chat_model_picker, 13_chat_agent_selection, 14_project_new,
15_settings_memory, 16_settings_platform, 17_settings_brain_spike, 18_automation_loop_new,
19_code_filter, 20_aci_browser_chat (16 of 20).

Flows 03, 08, 09, 10 use ids for navigation but still assert some text.

## iOS-only steps

The optional "Dismiss notifications prompt" tap (11 flows) is wrapped in
`runFlow: when: platform: iOS`. Each carries a `TODO(android step N)` for the Android
POST_NOTIFICATIONS dialog.
