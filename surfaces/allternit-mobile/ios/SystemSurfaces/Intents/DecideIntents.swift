import AppIntents
import Foundation

// Decide group of the action suite (plan step I4c): Show what needs me,
// Approve an item, Deny an item, Reply to a bot, Snooze an item.
// SystemSurfaces code: system frameworks and SystemSurfaces types only.
//
// Approve and Deny show the whole text and the recipient in the system
// confirmation, require an unlocked device, and send the item's
// `contentHash`, so the computer refuses the decision when the text changed
// after it was shown (`changed`). When the whole text is not available they
// open the item in the app instead of deciding.

// MARK: - Rules and wording (testable)

enum DecisionRules {
    /// True when the item must be read in the app instead of being decided
    /// from Siri or Shortcuts: it cannot be decided here, its whole text is
    /// missing or looks cut, or a message to send has no recipient or hash.
    static func needsAppReview(_ item: ApprovalInfo) -> Bool {
        guard item.canApprove else { return true }
        let text = item.fullText.trimmingCharacters(in: .whitespacesAndNewlines)
        if text.isEmpty { return true }
        let summary = item.summary.trimmingCharacters(in: .whitespacesAndNewlines)
        // No separate text came back (the summary stands in) and it ends cut.
        if text == summary, text.hasSuffix("…") || text.hasSuffix("...") { return true }
        if item.isPeerHeld {
            if (item.recipient ?? "").trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { return true }
            if (item.contentHash ?? "").trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { return true }
        }
        return false
    }
}

enum DecisionWording {
    static let openInAppPrompt = "Open Allternit to read the whole item before you decide."

    /// The confirmation shown before Approve or Deny: who it goes to and the
    /// whole text, never a shortened one.
    static func confirmation(_ item: ApprovalInfo, kind: DecisionKind) -> String {
        let to = item.recipient?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let text = item.fullText.trimmingCharacters(in: .whitespacesAndNewlines)
        switch kind {
        case .approve where item.isPeerHeld:
            return "Send this message from \(item.botName) to \(to)?\n\n\(text)"
        case .approve:
            return "Approve this for \(item.botName)\(to.isEmpty ? "" : " (to \(to))")?\n\n\(text)"
        case .deny:
            return "Deny this from \(item.botName)\(to.isEmpty ? "" : " (to \(to))")?\n\n\(text)"
        case .snooze:
            return "Snooze this from \(item.botName)?\n\n\(text)"
        }
    }

    static func dialog(for r: DecisionResult, item: ApprovalInfo) -> String {
        let verb: String
        switch r.kind {
        case .approve: verb = "approved"
        case .deny: verb = "denied"
        case .snooze: verb = "snoozed"
        }
        switch r.outcome {
        case .approved:
            return "Approved. \(item.botName) will go ahead."
        case .sent:
            let to = item.recipient?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            return to.isEmpty ? "Approved and sent." : "Approved and sent to \(to)."
        case .changed:
            return "The item changed since you saw it, so it was not \(verb). Open it in Allternit to read the new version."
        case .alreadyHandled:
            return "That item was already handled. Nothing was changed."
        case .offline:
            return "Your computer is offline, so nothing was \(verb). Try again when it is back."
        case .denied:
            return "Denied. \(item.botName) will not go ahead."
        case .snoozed:
            guard let until = r.snoozedUntil else { return "Snoozed." }
            return "Snoozed until \(until.formatted(date: .omitted, time: .shortened))."
        }
    }

    /// The App Group snapshot's rows as a read-only list; nil with no snapshot.
    static func snapshotList(_ snapshot: StatusSnapshot?) -> ApprovalList? {
        guard let snapshot else { return nil }
        let rows = snapshot.needsYouRows.map {
            ApprovalInfo(id: $0.id, kind: $0.kind, botId: "", botName: $0.botName, summary: $0.line,
                         fullText: $0.line, canApprove: false)
        }
        return ApprovalList(items: rows, asOf: snapshot.asOf, fromSnapshot: true)
    }

    static func listDialog(_ list: ApprovalList) -> String {
        var text: String
        switch list.items.count {
        case 0: text = "Nothing needs you right now."
        case 1: text = "1 item needs you: \(line(list.items[0]))."
        default:
            let first = list.items.prefix(3).map(line).joined(separator: "; ")
            let more = list.items.count > 3 ? ", and \(list.items.count - 3) more" : ""
            text = "\(list.items.count) items need you: \(first)\(more)."
        }
        if list.fromSnapshot {
            let when = list.asOf.map { " at \($0.formatted(date: .omitted, time: .shortened))" } ?? ""
            text += " This is from the last update\(when). Your computer did not answer, so open Allternit to act on these."
        }
        return text
    }

    private static func line(_ item: ApprovalInfo) -> String {
        item.botName.isEmpty ? item.summary : "\(item.botName), \(item.summary)"
    }
}

/// The outcome as a Shortcuts value, so a chain can branch on it.
enum DecisionOutcomeAppEnum: String, AppEnum {
    case approved, sent, changed, alreadyHandled, offline, denied, snoozed

    static let typeDisplayRepresentation: TypeDisplayRepresentation = "Decision Result"
    static let caseDisplayRepresentations: [DecisionOutcomeAppEnum: DisplayRepresentation] = [
        .approved: "Approved",
        .sent: "Sent",
        .changed: "Changed, not approved",
        .alreadyHandled: "Already handled",
        .offline: "Computer offline",
        .denied: "Denied",
        .snoozed: "Snoozed",
    ]

    init(_ outcome: DecisionOutcome) {
        switch outcome {
        case .approved: self = .approved
        case .sent: self = .sent
        case .changed: self = .changed
        case .alreadyHandled: self = .alreadyHandled
        case .offline: self = .offline
        case .denied: self = .denied
        case .snoozed: self = .snoozed
        }
    }
}

// MARK: - Show what needs me

struct ShowWhatNeedsMeIntent: AppIntent {
    static let title: LocalizedStringResource = "Show What Needs Me"
    static let description = IntentDescription(
        "Lists the items your bots are waiting on you for. When your computer does not answer, it shows the last saved summary.",
        resultValueName: "Items")

    static var parameterSummary: some ParameterSummary {
        Summary("Show what needs me")
    }

    init() {}

    func perform() async throws -> some IntentResult & ReturnsValue<[ApprovalEntity]> & ProvidesDialog {
        try IntentGate.check(needsComputer: false)
        let list: ApprovalList
        do {
            list = try await IntentGate.run(needsComputer: false) { svc in try await svc.listApprovals() }
        } catch {
            if let failure = error as? IntentFailure, failure == .signedOut { throw failure }
            // An extension (no service) or no computer answering: the App Group snapshot.
            guard let saved = DecisionWording.snapshotList(AppGroupStore.snapshot) else { throw error }
            list = saved
        }
        return .result(value: list.items.map(ApprovalEntity.init),
                       dialog: IntentSpeech.dialog(DecisionWording.listDialog(list)))
    }
}

// MARK: - Approve / Deny (app only: they may continue in the app)

/// Approve and Deny share one flow. App only, because opening the item in
/// the app uses `ForegroundContinuableIntent`, which extensions cannot use.
@available(iOSApplicationExtension, unavailable)
protocol DecisionIntent: ForegroundContinuableIntent {}

@available(iOSApplicationExtension, unavailable)
extension DecisionIntent {
    func decide(_ kind: DecisionKind, entity: ApprovalEntity) async throws -> (DecisionResult, ApprovalInfo) {
        let id = entity.id
        do {
            try IntentGate.check(needsComputer: true)
        } catch let failure as IntentFailure where failure.opensApp {
            throw needsToContinueInForegroundError(IntentSpeech.dialog(failure.dialog))
        }

        // Decide on what the computer has now, not on what the picker cached.
        let list: ApprovalList
        do {
            list = try await IntentGate.run { svc in try await svc.listApprovals() }
        } catch IntentFailure.offline {
            return (DecisionResult(kind: kind, outcome: .offline, itemId: id), entity.info)
        }
        if list.fromSnapshot {
            return (DecisionResult(kind: kind, outcome: .offline, itemId: id), entity.info)
        }
        guard let item = list.items.first(where: { $0.id == id }) else {
            return (DecisionResult(kind: kind, outcome: .alreadyHandled, itemId: id), entity.info)
        }

        if DecisionRules.needsAppReview(item) {
            let link = IntentLinks.inboxItem(id)
            throw needsToContinueInForegroundError(IntentSpeech.dialog(DecisionWording.openInAppPrompt)) {
                if let svc = IntentServiceRegistry.current { _ = await svc.openLink(link) }
            }
        }

        let prompt = IntentSpeech.dialog(DecisionWording.confirmation(item, kind: kind))
        let action: ConfirmationActionName = (kind == .approve && item.isPeerHeld) ? .send : .continue
        if #available(iOS 18.0, *) {
            try await requestConfirmation(actionName: action, dialog: prompt)
        } else {
            try await requestConfirmation(result: .result(dialog: prompt), confirmationActionName: action)
        }

        let result: DecisionResult
        do {
            result = try await IntentGate.run { svc in
                switch kind {
                case .approve: return try await svc.approve(item)
                case .deny: return try await svc.deny(item)
                case .snooze: return try await svc.snooze(item, minutes: 60)
                }
            }
        } catch IntentFailure.offline {
            result = DecisionResult(kind: kind, outcome: .offline, itemId: id)
        }
        return (result, item)
    }
}

@available(iOSApplicationExtension, unavailable)
struct ApproveItemIntent: AppIntent, DecisionIntent {
    static let title: LocalizedStringResource = "Approve an Item"
    static let description = IntentDescription(
        "Approves something a bot is waiting on, after showing you the whole text and who it goes to.",
        resultValueName: "Result")
    static let authenticationPolicy: IntentAuthenticationPolicy = .requiresAuthentication

    @Parameter(title: "Item")
    var item: ApprovalEntity

    static var parameterSummary: some ParameterSummary {
        Summary("Approve \(\.$item)")
    }

    init() {}

    func perform() async throws -> some IntentResult & ReturnsValue<DecisionOutcomeAppEnum> & ProvidesDialog {
        let (result, info) = try await decide(.approve, entity: item)
        return .result(value: DecisionOutcomeAppEnum(result.outcome),
                       dialog: IntentSpeech.dialog(DecisionWording.dialog(for: result, item: info)))
    }
}

@available(iOSApplicationExtension, unavailable)
struct DenyItemIntent: AppIntent, DecisionIntent {
    static let title: LocalizedStringResource = "Deny an Item"
    static let description = IntentDescription(
        "Denies something a bot is waiting on, after showing you the whole text and who it was for.",
        resultValueName: "Result")
    static let authenticationPolicy: IntentAuthenticationPolicy = .requiresAuthentication

    @Parameter(title: "Item")
    var item: ApprovalEntity

    static var parameterSummary: some ParameterSummary {
        Summary("Deny \(\.$item)")
    }

    init() {}

    func perform() async throws -> some IntentResult & ReturnsValue<DecisionOutcomeAppEnum> & ProvidesDialog {
        let (result, info) = try await decide(.deny, entity: item)
        return .result(value: DecisionOutcomeAppEnum(result.outcome),
                       dialog: IntentSpeech.dialog(DecisionWording.dialog(for: result, item: info)))
    }
}

// MARK: - Reply to a bot

struct ReplyToBotIntent: AppIntent {
    static let title: LocalizedStringResource = "Reply to a Bot"
    static let description = IntentDescription("Sends your reply into a bot conversation.",
                                               resultValueName: "Conversation")

    @Parameter(title: "Conversation")
    var conversation: ConversationEntity
    @Parameter(title: "Reply")
    var reply: String

    static var parameterSummary: some ParameterSummary {
        Summary("Reply \(\.$reply) in \(\.$conversation)")
    }

    init() {}

    func perform() async throws -> some IntentResult & ReturnsValue<ConversationEntity> & ProvidesDialog {
        let text = reply.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { throw $reply.needsValueError("What do you want to reply?") }
        let target = conversation.info
        let convo = try await IntentGate.run { svc in try await svc.reply(conversation: target, text: text) }
        return .result(value: ConversationEntity(convo),
                       dialog: IntentSpeech.dialog("Sent your reply in \"\(convo.title)\"."))
    }
}

// MARK: - Snooze an item

struct SnoozeItemIntent: AppIntent {
    static let title: LocalizedStringResource = "Snooze an Item"
    static let description = IntentDescription("Hides an item that needs you for a while. It comes back after the time you pick.",
                                               resultValueName: "Snoozed Until")

    @Parameter(title: "Item")
    var item: ApprovalEntity
    @Parameter(title: "Minutes", default: 60)
    var minutes: Int

    static var parameterSummary: some ParameterSummary {
        Summary("Snooze \(\.$item) for \(\.$minutes) minutes")
    }

    init() {}

    func perform() async throws -> some IntentResult & ReturnsValue<Date?> & ProvidesDialog {
        let info = item.info
        let length = min(max(minutes, 5), 7 * 24 * 60)
        let result: DecisionResult
        do {
            result = try await IntentGate.run { svc in try await svc.snooze(info, minutes: length) }
        } catch IntentFailure.offline {
            result = DecisionResult(kind: .snooze, outcome: .offline, itemId: info.id)
        }
        return .result(value: result.snoozedUntil,
                       dialog: IntentSpeech.dialog(DecisionWording.dialog(for: result, item: info)))
    }
}
