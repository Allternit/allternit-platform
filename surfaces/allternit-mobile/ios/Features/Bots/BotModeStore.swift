import Foundation
import SwiftUI

/// State for Bot Mode — the native counterpart of the PWA's bot roster
/// store + thread store + group chat store (allternit-ai src/lib/bots/
/// bot-roster.store.ts, threads.ts, group-chat.store.ts).
///
/// Bots come from `AgentHubStore` (`GET /api/v1/agents`, filtered to bots
/// like the web's `getBots`); threads from `GET /api/v1/threads`; groups
/// from `GET /api/v1/bot-groups`; bot projects from `GET /api/v1/bot-projects`.
/// Pins and each bot's main-chat session id are per-device, like the web's
/// persisted roster store; the main chat id is also written to the bot
/// record (`config.canonicalThreadId`) so other devices find it.
@MainActor
final class BotModeStore: ObservableObject {
    static let shared = BotModeStore()

    @Published private(set) var threads: [BotThread] = []
    @Published private(set) var threadsError: String? = nil
    @Published private(set) var isLoadingThreads = false
    @Published private(set) var groups: [BotGroupChat] = []
    @Published private(set) var groupsError: String? = nil
    @Published private(set) var projects: [BotProjectSummary] = []
    @Published private(set) var pinnedBotIds: [String]
    @Published private(set) var canonicalChatIds: [String: String]
    /// A bot whose main chat Bots mode should open when it next appears
    /// (drawer search, Teammates); `BotsModeRootView` consumes it.
    @Published var pendingOpenBotId: String?
    /// The drawer's New pill in Bots mode: start a thread with the top bot
    /// of the rail (pinned first, then most recent).
    @Published var pendingNewThread = false
    /// The drawer's Phones list: a bot whose phone (its channels and the
    /// conversations on them) Bots mode should open next.
    @Published var pendingOpenPhoneBotId: String?
    /// Any Bot Mode screen the drawer's bots rail opens next (Threads,
    /// Groups, Computers…, the web rail's rows); `BotsModeRootView` pushes it
    /// over the Bots home.
    @Published var pendingRoute: BotRoute?
    /// Set by the drawer's Home: back to the Bots home (root of the stack).
    @Published var pendingHome = false

    private let hub = AgentHubStore.shared
    private let client = BotModeClient.shared
    private let chatClient = AgentChatClient()
    private var creatingMainChat: [String: Task<String, Error>] = [:]

    private enum Keys {
        static let pins = "allternit-bot-mode-pinned"
        static let canonical = "allternit-bot-mode-canonical-chats"
    }

    private init() {
        pinnedBotIds = UserDefaults.standard.stringArray(forKey: Keys.pins) ?? []
        canonicalChatIds = (UserDefaults.standard.dictionary(forKey: Keys.canonical) as? [String: String]) ?? [:]
    }

    // MARK: - Bots

    /// Every bot, pinned first, then by latest thread activity, then name
    /// (FabricBotModeRail sortedBots).
    var bots: [AgentRecord] {
        let all = hub.agents.filter { $0.isBot }
        let activity = Dictionary(grouping: threads, by: \.botId)
            .mapValues { $0.map(\.lastActivityAt).max() ?? "" }
        return all.sorted { a, b in
            let ap = pinnedBotIds.contains(a.id), bp = pinnedBotIds.contains(b.id)
            if ap != bp { return ap }
            let aa = activity[a.id] ?? "", ba = activity[b.id] ?? ""
            if aa != ba { return aa > ba }
            return a.botDisplayName.localizedCaseInsensitiveCompare(b.botDisplayName) == .orderedAscending
        }
    }

    func bot(_ id: String) -> AgentRecord? {
        hub.agent(withId: id)
    }

    func isPinned(_ botId: String) -> Bool { pinnedBotIds.contains(botId) }

    func togglePin(_ botId: String) {
        if let index = pinnedBotIds.firstIndex(of: botId) {
            pinnedBotIds.remove(at: index)
        } else {
            pinnedBotIds.append(botId)
        }
        UserDefaults.standard.set(pinnedBotIds, forKey: Keys.pins)
    }

    // MARK: - Threads

    /// Threads waiting on you, across every bot (rail badge).
    var waitingCount: Int {
        threads.filter { $0.group == "waiting" }.count
    }

    func threads(for botId: String) -> [BotThread] {
        threads.filter { $0.botId == botId }.sorted { Self.threadOrder($0, $1) }
    }

    /// Open threads of a bot, not counting its main chat (PhoneBotPage counts).
    func openThreadCount(for botId: String) -> Int {
        let main = canonicalChatIds[botId]
        return threads.filter {
            $0.botId == botId && $0.group != "resolved" && !$0.isMainChat && $0.currentSessionId != main
        }.count
    }

    func waitingCount(for botId: String) -> Int {
        threads.filter { $0.botId == botId && $0.group == "waiting" }.count
    }

    func workingCount(for botId: String) -> Int {
        threads.filter { $0.botId == botId && $0.group == "working" }.count
    }

    nonisolated static func threadOrder(_ a: BotThread, _ b: BotThread) -> Bool {
        if a.groupOrder != b.groupOrder { return a.groupOrder < b.groupOrder }
        return a.lastActivityAt > b.lastActivityAt
    }

    // MARK: - Loading

    func refreshAll() async {
        hub.fetchAgentsIfNeeded()
        await refreshThreads()
        await refreshGroups()
        await refreshProjects()
    }

    func refreshThreads() async {
        isLoadingThreads = true
        defer { isLoadingThreads = false }
        do {
            threads = try await client.listAllThreads()
            threadsError = nil
        } catch is CancellationError {
        } catch {
            threadsError = FriendlyErrorMessage.from(error.localizedDescription)
        }
    }

    func refreshGroups() async {
        do {
            groups = try await client.listGroups()
                .sorted { $0.updatedAt > $1.updatedAt }
            groupsError = nil
        } catch is CancellationError {
        } catch {
            groupsError = FriendlyErrorMessage.from(error.localizedDescription)
        }
    }

    func refreshProjects() async {
        do {
            projects = try await client.listBotProjects().filter { !$0.archived }
        } catch {
            // Older servers have no bot projects; the rail just hides them.
            projects = []
        }
    }

    // MARK: - Threads actions

    func approve(_ thread: BotThread) async throws {
        try await client.approveThread(id: thread.id)
        await refreshThreads()
    }

    func resolve(_ thread: BotThread) async throws {
        try await client.resolveThread(id: thread.id)
        await refreshThreads()
    }

    func decline(_ thread: BotThread) async throws {
        try await client.declineThread(id: thread.id)
        await refreshThreads()
    }

    // MARK: - Groups

    func group(_ id: String) -> BotGroupChat? {
        groups.first { $0.id == id }
    }

    /// Creates a room the web way: id = slug of the name, unique-suffixed.
    /// `vendorIds`: members whose bot runs on a vendor (saved as `source: "vendor"`).
    func createGroup(name: String, members: [AgentRecord], vendorIds: Set<String> = []) async throws -> BotGroupChat {
        let base = Self.slugify(name).isEmpty ? "group" : Self.slugify(name)
        var id = base
        var suffix = 2
        while groups.contains(where: { $0.id == id }) {
            id = "\(base)-\(suffix)"
            suffix += 1
        }
        let now = ISO8601DateFormatter().string(from: Date())
        let group = BotGroupChat(
            id: id,
            name: name,
            image: nil,
            members: members.map {
                GroupChatMember(botId: $0.id, displayName: $0.botDisplayName,
                                handle: Self.slugify($0.botDisplayName),
                                source: vendorIds.contains($0.id) ? "vendor" : "native", providerId: nil)
            },
            log: [],
            createdAt: now,
            updatedAt: now
        )
        try await client.saveGroup(group)
        groups.insert(group, at: 0)
        return group
    }

    func saveGroup(_ group: BotGroupChat) async {
        if let index = groups.firstIndex(where: { $0.id == group.id }) {
            groups[index] = group
        }
        try? await client.saveGroup(group)
    }

    func deleteGroup(_ id: String) async throws {
        try await client.deleteGroup(id: id)
        groups.removeAll { $0.id == id }
    }

    static func slugify(_ text: String) -> String {
        let lowered = text.lowercased().trimmingCharacters(in: .whitespaces)
        var out = ""
        var lastDash = false
        for ch in lowered {
            if ch.isASCII && (ch.isLetter || ch.isNumber) {
                out.append(ch)
                lastDash = false
            } else if !lastDash {
                out.append("-")
                lastDash = true
            }
        }
        return out.trimmingCharacters(in: CharacterSet(charactersIn: "-"))
    }

    // MARK: - Main chat

    /// The bot's main chat session, recovered before anything creates one
    /// (web bot-canonical-chat.service.ts `openBotCanonicalChat`):
    /// local pin → server pin (`config.canonicalThreadId`) → newest
    /// "Main chat" thread → create (with `botCanonicalFor`).
    func mainChatSessionId(for bot: AgentRecord) async throws -> String {
        if let local = canonicalChatIds[bot.id] { return local }
        if case .string(let pin)? = bot.config?["canonicalThreadId"], !pin.isEmpty {
            remember(pin, for: bot.id)
            return pin
        }
        let botThreads = (try? await ThreadsClient.shared.listThreads(botId: bot.id)) ?? threads(for: bot.id)
        if let main = botThreads
            .filter({ $0.isMainChat && $0.currentSessionId != nil })
            .max(by: { $0.lastActivityAt < $1.lastActivityAt }),
           let sessionId = main.currentSessionId {
            remember(sessionId, for: bot.id)
            return sessionId
        }
        if let running = creatingMainChat[bot.id] {
            return try await running.value
        }
        let greeting = bot.botGreeting
        let task = Task { [client] () throws -> String in
            let session = try await client.createBotSession(
                botId: bot.id, botName: bot.botDisplayName, name: "Bot Chat",
                metadata: ["botCanonicalFor": bot.id]
            )
            // The bot speaks first. Best effort: a runtime that can't store
            // assistant notes still opens the chat.
            if session.id.hasPrefix("ses") {
                try? await client.storeGreeting(sessionId: session.id, text: greeting)
            }
            return session.id
        }
        creatingMainChat[bot.id] = task
        defer { creatingMainChat[bot.id] = nil }
        let sessionId = try await task.value
        remember(sessionId, for: bot.id)
        writeServerPin(sessionId, for: bot)
        return sessionId
    }

    /// Starts a fresh thread chat with the bot (the "New thread" action).
    func newThreadSession(for bot: AgentRecord, title: String = "New thread") async throws -> String {
        let session = try await client.createBotSession(
            botId: bot.id, botName: bot.botDisplayName, name: title,
            metadata: ["botThreadOf": bot.id]
        )
        return session.id
    }

    private func remember(_ sessionId: String, for botId: String) {
        canonicalChatIds[botId] = sessionId
        UserDefaults.standard.set(canonicalChatIds, forKey: Keys.canonical)
    }

    /// Fire-and-forget pin on the bot record (web bot-thread-pin.ts). The
    /// agent PUT replaces `config` whole, so the full merged object is sent.
    private func writeServerPin(_ sessionId: String, for bot: AgentRecord) {
        var config = bot.config ?? [:]
        config["canonicalThreadId"] = .string(sessionId)
        let next = config
        Task {
            try? await AgentClient().updateAgent(id: bot.id, config: next)
        }
    }

    /// Drops a pin that no longer loads (deleted session) so the next open
    /// recovers or recreates it.
    func forgetMainChat(for botId: String) {
        canonicalChatIds[botId] = nil
        UserDefaults.standard.set(canonicalChatIds, forKey: Keys.canonical)
    }
}
