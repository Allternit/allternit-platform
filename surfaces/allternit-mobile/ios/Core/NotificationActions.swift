import Foundation
import UIKit
@preconcurrency import UserNotifications
import os

// MARK: - Fixed identifiers (contract: docs api/system-surfaces.mdx)

/// Category identifiers the cloud puts in `aps.category`. Fixed: the server
/// and the app must agree on them.
enum NotificationCategoryID {
    static let approvalInline = "APPROVAL_INLINE"
    static let approvalReview = "APPROVAL_REVIEW"
    static let botReply = "BOT_REPLY"
    static let runDone = "RUN_DONE"
    static let needsFix = "NEEDS_FIX"
}

/// Button identifiers. Fixed, like the categories.
enum NotificationActionID: String, Sendable, CaseIterable {
    case approve = "APPROVE"
    case deny = "DENY"
    case reply = "REPLY"
    case review = "REVIEW"
    case open = "OPEN"
    case retry = "RETRY"
    case snooze = "SNOOZE"
}

/// The one strings table for notification buttons and follow-up lines.
enum NotificationStrings {
    static let approve = String(localized: "Approve")
    static let deny = String(localized: "Deny")
    static let reply = String(localized: "Reply")
    static let review = String(localized: "Review")
    static let open = String(localized: "Open")
    static let retry = String(localized: "Retry")
    static let snooze = String(localized: "Snooze")
    static let replyButton = String(localized: "Send")
    static let replyPlaceholder = String(localized: "Message the bot")

    static let changed = String(localized: "This changed. Review it again.")
    static let offline = String(localized: "Your computer is offline. Open Allternit to retry.")
    static let signedOut = String(localized: "Open Allternit to sign in, then try again.")
    static let failed = String(localized: "That didn't go through. Open Allternit to try again.")
    static let alreadyHandled = String(localized: "Already handled")
}

// MARK: - Categories

enum NotificationCategories {
    /// Every category, buttons in the contract's order. Buttons that act
    /// stay in the background (`.authenticationRequired`, never
    /// `.foreground`); Review and Open bring the app up.
    static func all() -> Set<UNNotificationCategory> {
        let auth: UNNotificationActionOptions = [.authenticationRequired]
        func button(_ id: NotificationActionID, _ title: String, _ options: UNNotificationActionOptions) -> UNNotificationAction {
            UNNotificationAction(identifier: id.rawValue, title: title, options: options)
        }
        func reply() -> UNTextInputNotificationAction {
            UNTextInputNotificationAction(
                identifier: NotificationActionID.reply.rawValue,
                title: NotificationStrings.reply,
                options: auth,
                textInputButtonTitle: NotificationStrings.replyButton,
                textInputPlaceholder: NotificationStrings.replyPlaceholder
            )
        }
        let approve = button(.approve, NotificationStrings.approve, auth)
        let deny = button(.deny, NotificationStrings.deny, [.authenticationRequired, .destructive])
        let review = button(.review, NotificationStrings.review, [.foreground])
        let open = button(.open, NotificationStrings.open, [.foreground])
        return [
            UNNotificationCategory(identifier: NotificationCategoryID.approvalInline,
                                   actions: [approve, deny, reply()], intentIdentifiers: []),
            UNNotificationCategory(identifier: NotificationCategoryID.approvalReview,
                                   actions: [review, deny, reply()], intentIdentifiers: []),
            UNNotificationCategory(identifier: NotificationCategoryID.botReply,
                                   actions: [reply(), open], intentIdentifiers: []),
            UNNotificationCategory(identifier: NotificationCategoryID.runDone,
                                   actions: [open], intentIdentifiers: []),
            UNNotificationCategory(identifier: NotificationCategoryID.needsFix,
                                   actions: [button(.retry, NotificationStrings.retry, auth),
                                             button(.snooze, NotificationStrings.snooze, auth)],
                                   intentIdentifiers: []),
        ]
    }

    static func register() {
        UNUserNotificationCenter.current().setNotificationCategories(all())
    }
}

// MARK: - Resolver

/// Answers an inbox item without UI: notification buttons today, Live
/// Activity buttons (I3) and App Intents (I4c) later.
///
/// - Targets the item's own computer (`runtimeId` from the push) through the
///   cloud relay, else the active one.
/// - Approve carries `contentHash`; the runtime answers 409 `changed` when
///   the item moved on. Deny, Retry and Snooze use the same inbox routes as
///   the Inbox sheet. Reply posts the owner's text to the bot's session; it
///   never sends anything to another person (that only happens on Approve).
/// - Credentials come without UI (see `BackgroundCredential`).
enum NotificationActions {
    static let log = Logger(subsystem: "com.allternit.mobile", category: "notification-actions")

    /// What the action turned out to be.
    enum Result: Equatable, Sendable {
        case done
        /// 404: someone (another device, the bot) already handled it.
        case alreadyHandled
        /// 409 `changed`: the text or recipient changed since the push.
        case changed
        /// The computer is offline, asleep past the wait, or not paired.
        case offline
        /// No credential worked (401/403, or none available).
        case signedOut
        case failed(String)
        /// The action does not apply (no inbox id, no session, empty reply).
        case invalid
    }

    /// The request an action turns into.
    enum Operation: Equatable, Sendable {
        case approve(contentHash: String?)
        case deny
        case reply(sessionId: String, text: String)
        case retry
        case snooze(minutes: Int)
    }

    /// What to do with the notification afterwards.
    enum FollowUp: Equatable, Sendable {
        /// Remove the delivered notification(s) for the item.
        case remove
        /// Remove the stale one and post a local notification with sound.
        case replace(String)
        /// Keep the original (its buttons can be tried again) and post a
        /// local notification with sound.
        case notify(String)
        case none
    }

    // MARK: Pure mapping (tested)

    static func operation(for action: NotificationActionID, contentHash: String?,
                          sessionId: String?, replyText: String?) -> Operation? {
        switch action {
        case .approve: return .approve(contentHash: contentHash?.nilIfBlank)
        case .deny: return .deny
        case .retry: return .retry
        case .snooze: return .snooze(minutes: 60)
        case .reply:
            guard let sessionId = sessionId?.nilIfBlank,
                  let text = replyText?.trimmingCharacters(in: .whitespacesAndNewlines).nilIfBlank else { return nil }
            return .reply(sessionId: sessionId, text: text)
        case .review, .open: return nil
        }
    }

    static func classify(_ error: Error) -> Result {
        switch error {
        case APIError.httpError(let status, _):
            switch status {
            case 404: return .alreadyHandled
            case 409: return .changed
            case 401, 403: return .signedOut
            case 502, 503, 504: return .offline
            default: return .failed("HTTP \(status)")
            }
        case RuntimeRelayError.noComputer, RuntimeRelayError.unreachable: return .offline
        case is NotificationActionTimeout: return .offline
        case is URLError: return .offline
        case is CancellationError: return .offline
        default: return .failed(error.localizedDescription)
        }
    }

    static func followUp(for result: Result) -> FollowUp {
        switch result {
        case .done, .alreadyHandled: return .remove
        case .changed: return .replace(NotificationStrings.changed)
        case .offline: return .notify(NotificationStrings.offline)
        case .signedOut: return .notify(NotificationStrings.signedOut)
        case .failed: return .notify(NotificationStrings.failed)
        case .invalid: return .none
        }
    }

    // MARK: Public entry

    /// Resolves one inbox item. Callable from any surface without UI.
    /// Never throws; finishes within ~25 s (the relay's wake wait is cut
    /// short so the system's background time is not exceeded).
    static func resolve(inboxId: String, action: NotificationActionID, contentHash: String?,
                        runtimeId: String?, sessionId: String?, replyText: String?) async -> Result {
        guard let operation = operation(for: action, contentHash: contentHash,
                                        sessionId: sessionId, replyText: replyText) else { return .invalid }
        if case .reply = operation {} else if inboxId.isEmpty { return .invalid }
        let started = ContinuousClock.now
        let credential = await BackgroundCredential.current()
        log.notice("action \(action.rawValue, privacy: .public) credential=\(credential.kind.rawValue, privacy: .public) runtime=\(runtimeId ?? "active", privacy: .public)")
        guard let token = credential.token else {
            log.notice("action \(action.rawValue, privacy: .public) no credential")
            return .signedOut
        }
        let result: Result
        do {
            try await withTimeout(seconds: 25) {
                try await perform(operation, inboxId: inboxId, runtimeId: runtimeId?.nilIfBlank, bearer: token)
            }
            result = .done
        } catch {
            result = classify(error)
        }
        let elapsed = ContinuousClock.now - started
        log.notice("action \(action.rawValue, privacy: .public) result=\(String(describing: result), privacy: .public) credential=\(credential.kind.rawValue, privacy: .public) elapsed_ms=\(elapsed.milliseconds, privacy: .public)")
        if case .done = result {
            // The widget's counts changed (approve, deny, snooze, retry).
            Task { @MainActor in await StatusSnapshotWriter.refreshFromCloud() }
        }
        return result
    }

    private static func perform(_ op: Operation, inboxId: String, runtimeId: String?, bearer: String) async throws {
        let inbox = InboxClient.shared
        switch op {
        case .approve(let hash):
            var body: [String: Any] = ["surface": "notification"]
            if let hash { body["contentHash"] = hash }
            try await inbox.act(inboxId, verb: "approve", body: body, runtimeId: runtimeId, bearer: bearer)
        case .deny:
            try await inbox.act(inboxId, verb: "approve", body: ["decision": "deny", "surface": "notification"],
                                runtimeId: runtimeId, bearer: bearer)
        case .retry:
            try await inbox.act(inboxId, verb: "retry", runtimeId: runtimeId, bearer: bearer)
        case .snooze(let minutes):
            try await inbox.act(inboxId, verb: "snooze", body: ["minutes": minutes], runtimeId: runtimeId, bearer: bearer)
        case .reply(let sessionId, let text):
            try await BotModeClient.shared.sendUserMessage(sessionId: sessionId, text: text)
        }
    }

    // MARK: Notification response (AppDelegate)

    /// Handles a button press. Runs under a background task assertion and
    /// returns on every path (the async delegate method then completes).
    static func handle(response: UNNotificationResponse) async {
        let request = response.notification.request
        let userInfo = request.content.userInfo
        let data = userInfo["data"] as? [String: Any] ?? [:]
        guard let action = NotificationActionID(rawValue: response.actionIdentifier) else { return }
        let inboxId = data["inboxId"] as? String ?? ""
        let reply = (response as? UNTextInputNotificationResponse)?.userText

        let assertion = await BackgroundAssertion.begin(name: "notification-\(action.rawValue)")
        defer { Task { await assertion.end() } }

        let result = await resolve(
            inboxId: inboxId, action: action,
            contentHash: data["contentHash"] as? String,
            runtimeId: data["runtimeId"] as? String,
            sessionId: (data["sessionId"] as? String) ?? (data["threadSessionId"] as? String),
            replyText: reply
        )
        await apply(followUp(for: result), request: request, inboxId: inboxId, userInfo: userInfo)
        if result == .done || result == .alreadyHandled {
            await MainActor.run { InboxStore.shared.removeLocally(inboxId) }
        }
    }

    private static func apply(_ followUp: FollowUp, request: UNNotificationRequest,
                              inboxId: String, userInfo: [AnyHashable: Any]) async {
        let center = UNUserNotificationCenter.current()
        switch followUp {
        case .none:
            return
        case .remove:
            center.removeDeliveredNotifications(withIdentifiers: [request.identifier])
            if !inboxId.isEmpty { await removeDelivered(inboxIds: [inboxId], tags: []) }
        case .replace(let line):
            center.removeDeliveredNotifications(withIdentifiers: [request.identifier])
            await postFollowUp(line, title: request.content.title, inboxId: inboxId, userInfo: userInfo)
        case .notify(let line):
            await postFollowUp(line, title: request.content.title, inboxId: inboxId, userInfo: userInfo)
        }
    }

    /// A local notification with sound that opens the item when tapped.
    private static func postFollowUp(_ line: String, title: String, inboxId: String, userInfo: [AnyHashable: Any]) async {
        let content = UNMutableNotificationContent()
        content.title = title.isEmpty ? "Allternit" : title
        content.body = line
        content.sound = .default
        var info: [String: Any] = [:]
        if let data = userInfo["data"] as? [String: Any] { info["data"] = data }
        if let url = userInfo["url"] as? String { info["url"] = url }
        content.userInfo = info
        let id = "followup-" + (inboxId.isEmpty ? UUID().uuidString : inboxId)
        try? await UNUserNotificationCenter.current().add(
            UNNotificationRequest(identifier: id, content: content, trigger: nil)
        )
    }

    // MARK: Revoke + reconcile

    /// A delivered notification belongs to an item when its request id (the
    /// `apns-collapse-id` when the server set one, else a system id), its
    /// thread id, its `tag`, or its `data.inboxId` matches.
    static func matches(identifier: String, threadId: String, userInfo: [AnyHashable: Any],
                        inboxIds: Set<String>, tags: Set<String>) -> Bool {
        let data = userInfo["data"] as? [String: Any]
        let inboxId = data?["inboxId"] as? String
        let tag = userInfo["tag"] as? String
        if let inboxId, inboxIds.contains(inboxId) || tags.contains(tagFor(inboxId: inboxId)) { return true }
        if tags.contains(identifier) || (!threadId.isEmpty && tags.contains(threadId)) { return true }
        if let tag, tags.contains(tag) { return true }
        if identifier.hasPrefix("followup-"), inboxIds.contains(String(identifier.dropFirst("followup-".count))) { return true }
        return false
    }

    /// `approval:ap1` → `approval-ap1`, the cloud's tag for an inbox item.
    static func tagFor(inboxId: String) -> String {
        inboxId.replacingOccurrences(of: ":", with: "-")
    }

    static func removeDelivered(inboxIds: Set<String>, tags: Set<String>) async {
        guard !inboxIds.isEmpty || !tags.isEmpty else { return }
        let center = UNUserNotificationCenter.current()
        let delivered = await center.deliveredNotifications()
        let ids = delivered.compactMap { note -> String? in
            let r = note.request
            return matches(identifier: r.identifier, threadId: r.content.threadIdentifier,
                           userInfo: r.content.userInfo, inboxIds: inboxIds, tags: tags) ? r.identifier : nil
        }
        if !ids.isEmpty {
            center.removeDeliveredNotifications(withIdentifiers: ids)
            log.notice("removed \(ids.count, privacy: .public) delivered notification(s)")
        }
    }

    /// Background push `{"data":{"event":"notification.revoke","tags":[…]}}`.
    /// Returns false when the payload is not a revoke.
    static func revokeTargets(userInfo: [AnyHashable: Any]) -> (inboxIds: Set<String>, tags: Set<String>)? {
        guard let data = userInfo["data"] as? [String: Any],
              data["event"] as? String == "notification.revoke" else { return nil }
        return (Set(data["inboxIds"] as? [String] ?? []), Set(data["tags"] as? [String] ?? []))
    }

    /// On every foreground: drop delivered notifications whose inbox item is
    /// gone. Only runs when the inbox list loaded (an offline computer
    /// removes nothing).
    @MainActor
    static func reconcileDelivered() async {
        let store = InboxStore.shared
        await store.refresh()
        guard store.hasLoaded, store.isAvailable, store.error == nil else { return }
        let live = Set(store.items.map(\.id))
        let delivered = await UNUserNotificationCenter.current().deliveredNotifications()
        var gone: Set<String> = []
        for note in delivered {
            let data = note.request.content.userInfo["data"] as? [String: Any]
            if let id = data?["inboxId"] as? String, !id.isEmpty, !live.contains(id) { gone.insert(id) }
        }
        await removeDelivered(inboxIds: gone, tags: [])
    }
}

// MARK: - Credentials without UI

/// The Bearer a background action uses, chosen without showing anything.
///
/// The phone may have launched the app in the background (after the unlock
/// that `.authenticationRequired` asks for). Clerk restores its session from
/// its own keychain asynchronously, so it is given up to 2 s to appear. When
/// it has not loaded, the runtime device token (`allternit_runtime_…`) is
/// used. Cloud-api's relay accepts that token only once the relay route
/// takes device credentials; until then it answers 401, which shows as
/// "Open Allternit to sign in" (see the os_log line with `credential=`).
enum BackgroundCredential {
    enum Kind: String, Sendable { case clerk, device, none }

    struct Value: Sendable {
        let token: String?
        let kind: Kind
    }

    @MainActor
    static func current(clerkWait: Duration = .seconds(2)) async -> Value {
        let deadline = ContinuousClock.now + clerkWait
        repeat {
            // getToken() returns nil at once while Clerk has no session.
            if let token = try? await AuthManager.shared.getToken(), !token.isEmpty {
                return Value(token: token, kind: .clerk)
            }
            try? await Task.sleep(for: .milliseconds(200))
            AuthManager.shared.refreshAuthState()
        } while ContinuousClock.now < deadline
        if let device = RuntimePairing.shared.deviceToken() { return Value(token: device, kind: .device) }
        return Value(token: nil, kind: .none)
    }
}

// MARK: - Helpers

struct NotificationActionTimeout: Error {}

/// Runs `body`, throwing `NotificationActionTimeout` after `seconds`.
func withTimeout(seconds: Double, _ body: @escaping @Sendable () async throws -> Void) async throws {
    try await withThrowingTaskGroup(of: Void.self) { group in
        group.addTask { try await body() }
        group.addTask {
            try await Task.sleep(for: .seconds(seconds))
            throw NotificationActionTimeout()
        }
        defer { group.cancelAll() }
        try await group.next()
    }
}

/// A `beginBackgroundTask` assertion so the process is not suspended while
/// an action's request is in flight.
@MainActor
final class BackgroundAssertion {
    private var id: UIBackgroundTaskIdentifier = .invalid

    static func begin(name: String) -> BackgroundAssertion {
        let a = BackgroundAssertion()
        a.id = UIApplication.shared.beginBackgroundTask(withName: name) { [weak a] in a?.end() }
        return a
    }

    func end() {
        guard id != .invalid else { return }
        UIApplication.shared.endBackgroundTask(id)
        id = .invalid
    }
}

private extension String {
    var nilIfBlank: String? {
        trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? nil : self
    }
}

private extension Duration {
    var milliseconds: Int64 {
        let c = components
        return c.seconds * 1000 + c.attoseconds / 1_000_000_000_000_000
    }
}
