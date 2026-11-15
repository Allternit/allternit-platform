import AppIntents
import Foundation

// Talk group of the action suite (plan step I4b): Ask a bot, Call main bot,
// New chat with a bot. SystemSurfaces code: system frameworks and
// SystemSurfaces types only (this file is compiled into the extensions too).

// MARK: - Shared helpers (Talk, Delegate, Decide)

/// Builds a spoken/shown dialog from runtime text.
enum IntentSpeech {
    static func dialog(_ text: String) -> IntentDialog {
        IntentDialog(LocalizedStringResource(stringLiteral: text))
    }
}

/// The `allternit://` links the Talk/Delegate/Decide intents open
/// (parsed by `DeepLinkRouter` in the app).
enum IntentLinks {
    static let home = URL(string: "allternit://")!
    static let callMain = URL(string: "allternit://call/main")!

    static func botChat(_ botId: String) -> URL {
        URL(string: "allternit://bot/\(escape(botId))/chat") ?? home
    }

    static func inboxItem(_ id: String) -> URL {
        URL(string: "allternit://inbox/\(escape(id))") ?? home
    }

    private static func escape(_ s: String) -> String {
        s.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed.subtracting(CharacterSet(charactersIn: "/"))) ?? s
    }
}

/// Opens Allternit at one screen. Returned as `OpensIntent` by the intents
/// that end in the app; not listed in Shortcuts on its own.
struct OpenAllternitLinkIntent: AppIntent {
    static let title: LocalizedStringResource = "Open Allternit Screen"
    static let description = IntentDescription("Opens Allternit at a screen.")
    static let openAppWhenRun: Bool = true
    static let isDiscoverable: Bool = false

    @Parameter(title: "Link")
    var link: String

    init() {}
    init(_ url: URL) { self.link = url.absoluteString }

    func perform() async throws -> some IntentResult {
        // Runs in the app (openAppWhenRun). The app registers its service at
        // launch; without one the app simply opens at its last screen.
        if let url = URL(string: link), let svc = IntentServiceRegistry.current {
            _ = await svc.openLink(url)
        }
        return .result()
    }
}

// MARK: - Wording (testable)

enum TalkWording {
    static func askDialog(_ r: AskResult) -> String {
        if let answer = r.answer?.trimmingCharacters(in: .whitespacesAndNewlines), !answer.isEmpty, !r.stillWorking {
            return answer
        }
        return "\(r.botName) is still working on it. The answer will be in the chat \"\(r.conversation.title)\"."
    }
}

// MARK: - Ask a bot

/// What Ask a bot returns: the answer (empty while the bot is still working)
/// and the conversation it went to, so a shortcut can open or reply to it.
struct BotAnswerEntity: TransientAppEntity {
    static let typeDisplayRepresentation: TypeDisplayRepresentation = "Bot Answer"

    @Property(title: "Bot")
    var botName: String
    @Property(title: "Answer")
    var answer: String
    @Property(title: "Still working")
    var stillWorking: Bool
    @Property(title: "Conversation")
    var conversation: ConversationEntity

    init() {
        botName = ""
        answer = ""
        stillWorking = false
        conversation = ConversationEntity(ConversationInfo(id: "", title: ""))
    }

    init(_ r: AskResult) {
        self.init()
        botName = r.botName
        answer = r.stillWorking ? "" : (r.answer ?? "")
        stillWorking = r.stillWorking
        conversation = ConversationEntity(r.conversation)
    }

    var displayRepresentation: DisplayRepresentation {
        stillWorking
            ? DisplayRepresentation(title: "\(botName) is still working", subtitle: "\(conversation.title)")
            : DisplayRepresentation(title: "\(answer)", subtitle: "\(botName)")
    }
}

struct AskBotIntent: AppIntent {
    static let title: LocalizedStringResource = "Ask a Bot"
    static let description = IntentDescription(
        "Sends a question to one of your bots and waits up to 20 seconds for the answer. If the bot needs longer, you get the conversation to check later.",
        resultValueName: "Bot Answer")
    /// How long Ask waits for the reply before it returns "still working".
    static let wait: TimeInterval = 20

    @Parameter(title: "Bot")
    var bot: BotEntity
    @Parameter(title: "Question")
    var question: String

    static var parameterSummary: some ParameterSummary {
        Summary("Ask \(\.$bot) \(\.$question)")
    }

    init() {}

    func perform() async throws -> some IntentResult & ReturnsValue<BotAnswerEntity> & ProvidesDialog {
        let text = question.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { throw $question.needsValueError("What do you want to ask?") }
        let botId = bot.id
        let wait = Self.wait
        let r = try await IntentGate.run { svc in try await svc.ask(botId: botId, text: text, wait: wait) }
        return .result(value: BotAnswerEntity(r), dialog: IntentSpeech.dialog(TalkWording.askDialog(r)))
    }
}

// MARK: - Call main bot

struct CallMainBotIntent: AppIntent {
    static let title: LocalizedStringResource = "Call Main Bot"
    static let description = IntentDescription("Opens Allternit and starts a call with your main bot.")

    static var parameterSummary: some ParameterSummary {
        Summary("Call my main bot")
    }

    init() {}

    func perform() async throws -> some IntentResult & ProvidesDialog & OpensIntent {
        do {
            try IntentGate.check(needsComputer: false)
        } catch let failure as IntentFailure where failure.opensApp {
            return .result(opensIntent: OpenAllternitLinkIntent(IntentLinks.home),
                           dialog: IntentSpeech.dialog(failure.dialog))
        }
        return .result(opensIntent: OpenAllternitLinkIntent(IntentLinks.callMain),
                       dialog: IntentSpeech.dialog("Calling your main bot."))
    }
}

// MARK: - New chat with a bot

struct NewBotChatIntent: AppIntent {
    static let title: LocalizedStringResource = "New Chat with a Bot"
    static let description = IntentDescription("Opens Allternit in a chat with the bot you pick.",
                                               resultValueName: "Bot")

    @Parameter(title: "Bot")
    var bot: BotEntity

    static var parameterSummary: some ParameterSummary {
        Summary("New chat with \(\.$bot)")
    }

    init() {}

    func perform() async throws -> some IntentResult & ReturnsValue<BotEntity> & ProvidesDialog & OpensIntent {
        let picked = bot
        do {
            try IntentGate.check(needsComputer: false)
        } catch let failure as IntentFailure where failure.opensApp {
            return .result(value: picked, opensIntent: OpenAllternitLinkIntent(IntentLinks.home),
                           dialog: IntentSpeech.dialog(failure.dialog))
        }
        return .result(value: picked, opensIntent: OpenAllternitLinkIntent(IntentLinks.botChat(picked.id)),
                       dialog: IntentSpeech.dialog("Opening a chat with \(picked.name)."))
    }
}
