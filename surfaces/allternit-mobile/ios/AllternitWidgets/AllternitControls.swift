import AppIntents
import SwiftUI
import WidgetKit

// Control Center / Lock Screen / Action Button controls (plan I5). iOS 18+.
//
// A control runs only intents compiled into the widget extension, so the five
// buttons use `OpenAllternitWidgetLinkIntent` from SystemSurfaces
// (`openAppWhenRun`: it runs in the app and routes the `allternit://` link).
// Run in place: none of the six. Five open the app; Pause all bots is a toggle
// whose intent also runs in the app (see `SetBotsPausedControlIntent`).

private let controlRoot = "com.allternit.mobile.control"

// MARK: Buttons that open the app

@available(iOS 18.0, *)
struct CallMainBotControl: ControlWidget {
    var body: some ControlWidgetConfiguration {
        StaticControlConfiguration(kind: "\(controlRoot).callMain") {
            ControlWidgetButton(action: OpenAllternitWidgetLinkIntent(urlString: "allternit://call/main")) {
                Label("Call main bot", systemImage: "phone.fill")
            }
        }
        .displayName("Call main bot")
        .description("Opens Allternit with your main bot.")
    }
}

@available(iOS 18.0, *)
struct AskBotControl: ControlWidget {
    var body: some ControlWidgetConfiguration {
        StaticControlConfiguration(kind: "\(controlRoot).ask") {
            ControlWidgetButton(action: OpenAllternitWidgetLinkIntent(urlString: "allternit://bot/main/chat")) {
                Label("Ask a bot", systemImage: "bubble.left.fill")
            }
        }
        .displayName("Ask a bot")
        .description("Opens a chat with your main bot.")
    }
}

@available(iOS 18.0, *)
struct HandOffTaskControl: ControlWidget {
    var body: some ControlWidgetConfiguration {
        StaticControlConfiguration(kind: "\(controlRoot).handOff") {
            // The bots launchpad composer: a new bot project handed to the team's coordinator.
            ControlWidgetButton(action: OpenAllternitWidgetLinkIntent(urlString: "allternit://handoff")) {
                Label("Hand off a task", systemImage: "arrowshape.turn.up.right.fill")
            }
        }
        .displayName("Hand off a task")
        .description("Opens the bots launchpad so you can hand a task to the team.")
    }
}

@available(iOS 18.0, *)
struct NeedsMeControl: ControlWidget {
    var body: some ControlWidgetConfiguration {
        StaticControlConfiguration(kind: "\(controlRoot).needsMe") {
            ControlWidgetButton(action: OpenAllternitWidgetLinkIntent(urlString: "allternit://inbox")) {
                Label("Show what needs me", systemImage: "tray.fill")
            }
        }
        .displayName("Show what needs me")
        .description("Opens the list of items waiting on you.")
    }
}

// MARK: Run favourite routine (configurable)

@available(iOS 18.0, *)
struct FavouriteRoutineControlIntent: ControlConfigurationIntent {
    static let title: LocalizedStringResource = "Favourite routine"
    static let isDiscoverable = false

    @Parameter(title: "Routine")
    var routine: RoutineEntity?

    init() {}
}

@available(iOS 18.0, *)
struct FavouriteRoutineProvider: AppIntentControlValueProvider {
    struct Value: Sendable { var routineID: String?; var name: String }

    func previewValue(configuration: FavouriteRoutineControlIntent) -> Value {
        Value(routineID: configuration.routine?.id, name: configuration.routine?.name ?? "Routine")
    }
    func currentValue(configuration: FavouriteRoutineControlIntent) async throws -> Value {
        previewValue(configuration: configuration)
    }
}

@available(iOS 18.0, *)
struct RunRoutineControl: ControlWidget {
    var body: some ControlWidgetConfiguration {
        AppIntentControlConfiguration(kind: "\(controlRoot).routine", provider: FavouriteRoutineProvider()) { value in
            // Without a routine the button opens the Routines list (the inbox is not it); the
            // router has `routine/<id>` only, so an unconfigured control opens the app home.
            let link = value.routineID.map { "allternit://routine/\($0)" } ?? "allternit://"
            ControlWidgetButton(action: OpenAllternitWidgetLinkIntent(urlString: link)) {
                Label(value.routineID == nil ? "Run a routine" : "Run \(value.name)", systemImage: "play.circle.fill")
            }
        }
        .displayName("Run favourite routine")
        .description("Opens one of your routines. Choose which when you add the control.")
        .promptsForUserConfiguration()
    }
}

// MARK: Pause all bots (toggle)
// `PauseControl` and `SetBotsPausedControlIntent` live in SystemSurfaces/Intents/PauseControlIntent.swift:
// an intent that runs in the app (openAppWhenRun) must be compiled into the app too.

@available(iOS 18.0, *)
struct PauseValueProvider: ControlValueProvider {
    var previewValue: Bool { false }
    func currentValue() async throws -> Bool { AppGroupStore.snapshot?.paused ?? false }
}

@available(iOS 18.0, *)
struct PauseBotsControl: ControlWidget {
    var body: some ControlWidgetConfiguration {
        StaticControlConfiguration(kind: PauseControl.kind, provider: PauseValueProvider()) { paused in
            ControlWidgetToggle("Pause all bots", isOn: paused, action: SetBotsPausedControlIntent()) { isOn in
                Label(isOn ? "Bots paused" : "Pause all bots", systemImage: isOn ? "pause.circle.fill" : "pause.circle")
            }
        }
        .displayName("Pause all bots")
        .description("Pauses or resumes routines, loops and unprompted bot messages. Opens Allternit to do it.")
    }
}
