import AppIntents
import Foundation
import WidgetKit

@available(iOS 18.0, *)
enum PauseControl {
    static let kind = "com.allternit.mobile.control.pause"
}

/// `SetBotsPausedIntent` cannot work inside the widget extension: its service
/// path (`IntentGate.run`) needs the app's registered `IntentServices`, and an
/// extension has none (it would throw `appRequired`). This intent therefore
/// sets `openAppWhenRun`: iOS launches Allternit and runs it there, where the
/// network call works. The App Group snapshot is updated so the toggle shows the new state.
@available(iOS 18.0, *)
struct SetBotsPausedControlIntent: SetValueIntent {
    static let title: LocalizedStringResource = "Pause or resume all bots"
    static let isDiscoverable = false
    static let openAppWhenRun: Bool = true

    @Parameter(title: "Paused")
    var value: Bool

    init() {}

    func perform() async throws -> some IntentResult {
        let target = value
        let state = try await IntentGate.run { svc in try await svc.setPaused(target) }
        if var snap = AppGroupStore.snapshot {
            snap.paused = state.paused
            AppGroupStore.saveSnapshot(snap)
        }
        ControlCenter.shared.reloadControls(ofKind: PauseControl.kind)
        WidgetCenter.shared.reloadTimelines(ofKind: StatusWidgetKind.id)
        return .result()
    }
}

