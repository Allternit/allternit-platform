import AppIntents
import Foundation

/// The last step of an Allternit shortcut: hands the shortcut's result back
/// to the bot run that started it (`ShortcutBridge`), without bringing the
/// app forward. Main app target only (Core/), so it runs in the process that
/// holds the pending run.
///
/// Allternit's own shortcuts pass the `request_id` from their JSON input.
/// Any other shortcut may leave Request empty: the result then goes to the
/// one run in flight, and is dropped if none or several are waiting. It can
/// only answer a run the paired computer started; it never starts anything.
struct ReturnShortcutResultIntent: AppIntent {
    static let title: LocalizedStringResource = "Return Result to Allternit"
    static let description = IntentDescription(
        "Send this shortcut's result back to the Allternit bot that ran it. Add it as the shortcut's last step.")
    static let openAppWhenRun: Bool = false

    @Parameter(title: "Result")
    var result: String?

    @Parameter(title: "Request", description: "The request_id from the shortcut's input. Leave empty if there is none.")
    var requestId: String?

    init() {}

    @MainActor
    func perform() async throws -> some IntentResult {
        ShortcutBridge.shared.deliver(result: result, requestId: requestId)
        return .result()
    }
}
