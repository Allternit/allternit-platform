import AppIntents
import Foundation
import os

// Live Activity intents (plan I3). This file compiles into the app and the
// widget, notification and share extensions, so it names no `Core/` type.
// A `LiveActivityIntent` always runs in the app process; the app installs a
// `LiveActivityActionHandling` in `LiveActivityActions.handler` at launch
// (`BotLiveActivityManager.installAtLaunch()`, called from `AppDelegate`),
// and the intents reach the I1 resolver through it.

enum LiveActivityDecision: String, Sendable {
    case approve
    case deny
}

enum LiveActivityActionOutcome: Equatable, Sendable {
    case done
    /// The text or recipient changed since the push: review it in the app.
    case changed
    case offline
    case signedOut
    case failed
}

/// What the app does for the Live Activity intents.
protocol LiveActivityActionHandling: Sendable {
    func decide(_ decision: LiveActivityDecision, inboxId: String, runtimeId: String?,
                contentHash: String?) async -> LiveActivityActionOutcome
    /// Starts (or refreshes) the bot activity. False when Live Activities are off.
    func watch(botId: String?) async -> Bool
}

/// The registry the app fills at launch.
enum LiveActivityActions {
    static let log = Logger(subsystem: "com.allternit.mobile", category: "live-activity-intents")

    private static let slot = OSAllocatedUnfairLock<(any LiveActivityActionHandling)?>(initialState: nil)

    static var handler: (any LiveActivityActionHandling)? {
        get { slot.withLock { $0 } }
        set { slot.withLock { $0 = newValue } }
    }

    static func decide(_ decision: LiveActivityDecision, inboxId: String, runtimeId: String?,
                       contentHash: String?) async throws {
        guard let handler else {
            log.error("\(decision.rawValue, privacy: .public): no handler installed")
            throw LiveActivityIntentError.appNotReady
        }
        let outcome = await handler.decide(decision, inboxId: inboxId, runtimeId: runtimeId, contentHash: contentHash)
        log.notice("\(decision.rawValue, privacy: .public) outcome=\(String(describing: outcome), privacy: .public)")
    }
}

enum LiveActivityIntentError: Error, CustomLocalizedStringResourceConvertible {
    case appNotReady
    case activitiesOff

    var localizedStringResource: LocalizedStringResource {
        switch self {
        case .appNotReady: return "Open Allternit, then try again."
        case .activitiesOff: return "Live Activities are off for Allternit in Settings."
        }
    }
}

/// Approve button on the bot Live Activity. Shown only when the content
/// state carries an inline question (`approveInline`, `inboxId`, `contentHash`).
struct ApproveFromLiveActivityIntent: LiveActivityIntent {
    static let title: LocalizedStringResource = "Approve"
    static let isDiscoverable = false
    static var authenticationPolicy: IntentAuthenticationPolicy { .requiresAuthentication }

    @Parameter(title: "Item")
    var inboxId: String

    @Parameter(title: "Computer")
    var runtimeId: String?

    @Parameter(title: "Content hash")
    var contentHash: String?

    init() {}

    init(inboxId: String, runtimeId: String?, contentHash: String?) {
        self.inboxId = inboxId
        self.runtimeId = runtimeId
        self.contentHash = contentHash
    }

    func perform() async throws -> some IntentResult {
        try await LiveActivityActions.decide(.approve, inboxId: inboxId, runtimeId: runtimeId, contentHash: contentHash)
        return .result()
    }
}

/// Deny button on the bot Live Activity.
struct DenyFromLiveActivityIntent: LiveActivityIntent {
    static let title: LocalizedStringResource = "Deny"
    static let isDiscoverable = false
    static var authenticationPolicy: IntentAuthenticationPolicy { .requiresAuthentication }

    @Parameter(title: "Item")
    var inboxId: String

    @Parameter(title: "Computer")
    var runtimeId: String?

    @Parameter(title: "Content hash")
    var contentHash: String?

    init() {}

    init(inboxId: String, runtimeId: String?, contentHash: String?) {
        self.inboxId = inboxId
        self.runtimeId = runtimeId
        self.contentHash = contentHash
    }

    func perform() async throws -> some IntentResult {
        try await LiveActivityActions.decide(.deny, inboxId: inboxId, runtimeId: runtimeId, contentHash: contentHash)
        return .result()
    }
}

/// "Watch a bot": starts the bot Live Activity from Shortcuts, Siri or a
/// control without opening the app. Covers iOS 17.0-17.1, which have no
/// push-to-start; on 17.2+ the server can also start it. A
/// `LiveActivityIntent` may start an activity while the app is in the
/// background. `botId` is a plain id until the bot entity (plan I4a) lands.
struct WatchBotIntent: LiveActivityIntent {
    static let title: LocalizedStringResource = "Watch a bot"
    static let description = IntentDescription("Shows a bot's work on the Lock Screen and in the Dynamic Island.")

    @Parameter(title: "Bot ID")
    var botId: String?

    init() {}

    init(botId: String?) {
        self.botId = botId
    }

    func perform() async throws -> some IntentResult & ProvidesDialog {
        guard let handler = LiveActivityActions.handler else { throw LiveActivityIntentError.appNotReady }
        guard await handler.watch(botId: botId) else { throw LiveActivityIntentError.activitiesOff }
        return .result(dialog: "Watching on the Lock Screen.")
    }
}
