import SwiftUI

/// One thing a bot waits on the person for, in the order the floor shows it:
/// a permission (approval), a question, a suggested thread, or a thread that
/// waits on an answer the app has not loaded as a question card yet.
enum OfficeNeed: Identifiable {
    case permission(AgentPermissionAsk)
    case question(AgentQuestionRequest)
    case suggestion(BotThread)
    case waiting(BotThread)

    var id: String {
        switch self {
        case .permission(let ask): return "permission:\(ask.id)"
        case .question(let request): return "question:\(request.id)"
        case .suggestion(let thread): return "suggestion:\(thread.id)"
        case .waiting(let thread): return "waiting:\(thread.id)"
        }
    }

    /// The chat the need belongs to, when known.
    var sessionId: String? {
        switch self {
        case .permission(let ask): return ask.sessionID.isEmpty ? nil : ask.sessionID
        case .question(let request): return request.sessionID.isEmpty ? nil : request.sessionID
        case .suggestion(let thread), .waiting(let thread): return thread.currentSessionId
        }
    }
}

/// Reads a bot's needs from the app's existing stores: `AgentAskStore` (the
/// permissions and questions the chat's approval cards answer) and
/// `BotModeStore` threads (proposed = suggestion, waiting = question).
@MainActor
enum OfficeNeeds {
    /// The bot's live chats, waiting and working first; at most 6 so a poll stays cheap.
    static func sessions(for botId: String) -> [String] {
        let threads = BotModeStore.shared.threads(for: botId).filter { $0.archived != true && $0.group != "resolved" }
        let ordered = threads.sorted { rank($0.group) < rank($1.group) }
        var seen = Set<String>()
        return ordered.compactMap(\.currentSessionId).filter { seen.insert($0).inserted }.prefix(6).map { $0 }
    }

    private static func rank(_ group: String) -> Int {
        switch group {
        case "waiting": return 0
        case "working": return 1
        default: return 2
        }
    }

    /// Every open need for the bot, approvals first. `skipping` holds need ids put off with Not now.
    static func all(for botId: String, skipping: Set<String> = []) -> [OfficeNeed] {
        let asks = AgentAskStore.shared
        let store = BotModeStore.shared
        let sessions = sessions(for: botId)
        let threads = store.threads(for: botId).filter { $0.archived != true }
        var out: [OfficeNeed] = []
        out += sessions.flatMap { asks.permissions(for: $0) }.map(OfficeNeed.permission)
        let questions = sessions.flatMap { asks.questions(for: $0) }
        out += questions.map(OfficeNeed.question)
        out += threads.filter { $0.status == "proposed" }.map(OfficeNeed.suggestion)
        // A waiting thread whose question already shows as a card is not listed twice.
        let asked = Set(questions.map(\.sessionID))
        out += threads.filter { $0.group == "waiting" && !asked.contains($0.currentSessionId ?? "") }.map(OfficeNeed.waiting)
        return out.filter { !skipping.contains($0.id) }
    }

    static func first(for botId: String, skipping: Set<String> = []) -> OfficeNeed? {
        all(for: botId, skipping: skipping).first
    }

    /// Re-reads what the bot's chats wait on (the same calls the chat's cards use).
    static func refresh(botId: String) async {
        for session in sessions(for: botId) {
            await AgentAskStore.shared.refresh(sessionId: session)
        }
    }
}

/// The approval card docked at the top of a bot's thread sheet on the floor:
/// what the bot waits on, with Approve / Not now (Answer for questions). It
/// acts only through the app's existing calls: `AgentAskStore.reply` / `answer`
/// / `skip` (the chat's PermissionAskView and QuestionCardView) and
/// `BotModeStore.approve` / `decline` (threads approve/decline). The frame
/// never approves.
struct OfficeApprovalCard: View {
    let bot: AgentRecord
    let need: OfficeNeed
    /// After an approval or answer reached the bot: a short confirmation for the floor's done bubble.
    var onDone: (String) -> Void
    /// Not now on a need the app can't decline on the server (a waiting thread): hide it here.
    var onSkip: () -> Void
    /// Answer a waiting thread in its own chat (its session id, nil when it has none yet).
    var onAnswerInThread: (BotThread) -> Void

    @State private var busy: String?
    @State private var error: String?
    @State private var answering = false

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 6) {
                Image(systemName: icon)
                    .font(.caption)
                    .accessibilityHidden(true)
                Text(kicker)
                    .font(.caption.weight(.semibold))
            }
            .foregroundColor(Color("TextSecondary"))

            Text(headline)
                .font(.subheadline.weight(.semibold))
                .foregroundColor(Color("TextPrimary"))
                .fixedSize(horizontal: false, vertical: true)
            if let detail {
                Text(detail)
                    .font(.footnote)
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(3)
            }

            if answering, case .question(let request) = need {
                // The chat's own question card, answered in place.
                QuestionCardView(request: request, agentName: bot.botDisplayName) {
                    onDone("Got your answer")
                }
            } else {
                buttons
            }

            if let error {
                Text(error)
                    .font(.caption)
                    .foregroundColor(Theme.statusError)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Color("BgPanel")))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
        .accessibilityElement(children: .contain)
        .accessibilityLabel("\(bot.botDisplayName) needs you: \(headline)")
    }

    // MARK: - Copy

    private var icon: String {
        switch need {
        case .permission: return "hand.raised.fill"
        case .question, .waiting: return "questionmark.bubble.fill"
        case .suggestion: return "lightbulb.fill"
        }
    }

    private var kicker: String {
        switch need {
        case .permission: return "\(bot.botDisplayName) is waiting for approval"
        case .question, .waiting: return "\(bot.botDisplayName) has a question"
        case .suggestion: return "\(bot.botDisplayName) suggests"
        }
    }

    private var headline: String {
        switch need {
        case .permission(let ask): return ask.actionLabel
        case .question(let request): return request.questions.first?.title ?? "A question for you"
        case .suggestion(let thread): return thread.displayTitle
        case .waiting(let thread): return thread.statusLine ?? thread.displayTitle
        }
    }

    private var detail: String? {
        switch need {
        case .permission(let ask): return ask.detail
        case .question(let request): return request.questions.count > 1 ? "\(request.questions.count) questions" : nil
        case .suggestion(let thread): return thread.statusLine
        case .waiting(let thread): return thread.statusLine == nil ? nil : thread.displayTitle
        }
    }

    // MARK: - Actions

    @ViewBuilder
    private var buttons: some View {
        HStack(spacing: 6) {
            switch need {
            case .permission(let ask):
                button("Not now", id: "no", kind: .quiet) {
                    try await AgentAskStore.shared.reply(ask, reply: "reject")
                    return nil
                }
                button("Approve", id: "yes", kind: .primary) {
                    try await AgentAskStore.shared.reply(ask, reply: "once")
                    return "Approved, back to work"
                }
            case .question(let request):
                button("Not now", id: "no", kind: .quiet) {
                    try await AgentAskStore.shared.skip(request)
                    return nil
                }
                Button("Answer") { answering = true }
                    .buttonStyle(QuestionCardButtonStyle(kind: .primary))
                    .disabled(busy != nil)
                    .accessibilityLabel("Answer \(bot.botDisplayName)")
            case .suggestion(let thread):
                button("Not now", id: "no", kind: .quiet) {
                    try await BotModeStore.shared.decline(thread)
                    return nil
                }
                button("Approve", id: "yes", kind: .primary) {
                    try await BotModeStore.shared.approve(thread)
                    return "On it"
                }
            case .waiting(let thread):
                Button("Not now") { onSkip() }
                    .buttonStyle(QuestionCardButtonStyle(kind: .quiet))
                Button("Answer") { onAnswerInThread(thread) }
                    .buttonStyle(QuestionCardButtonStyle(kind: .primary))
                    .accessibilityLabel("Answer \(bot.botDisplayName) in the thread")
            }
        }
    }

    /// `run` returns the done-bubble text (nil: no bubble, e.g. Not now).
    private func button(_ title: String, id: String, kind: QuestionCardButtonStyle.Kind,
                        run: @escaping @MainActor () async throws -> String?) -> some View {
        Button {
            busy = id
            error = nil
            Task {
                do {
                    let done = try await run()
                    Haptics.success()
                    busy = nil
                    if let done { onDone(done) } else { onSkip() }
                } catch {
                    busy = nil
                    self.error = "Couldn't send that. \(FriendlyErrorMessage.from(error.localizedDescription))"
                    Haptics.error()
                }
            }
        } label: {
            if busy == id {
                ProgressView().controlSize(.small)
            } else {
                Text(title)
            }
        }
        .buttonStyle(QuestionCardButtonStyle(kind: kind))
        .disabled(busy != nil)
        .accessibilityLabel("\(title): \(headline)")
    }
}
