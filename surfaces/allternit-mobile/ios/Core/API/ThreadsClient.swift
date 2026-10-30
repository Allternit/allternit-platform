import Foundation

/// Client for bot threads (`cmd/allternit-api/src/thread_routes.rs`,
/// `/api/v1/threads`) — the same thread objects Desktop's Threads panel shows
/// (spec P9.2). Goes through `APIClient.shared` like `ProjectsClient`.
final class ThreadsClient: @unchecked Sendable {
    static let shared = ThreadsClient()

    private let client: APIClient

    init(client: APIClient = .shared) {
        self.client = client
    }

    /// A bot's threads, resolved ones included (`{ threads: [...] }`).
    func listThreads(botId: String) async throws -> [BotThread] {
        let escaped = botId.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? botId
        let response: BotThreadListResponse = try await client.get(path: "threads?botId=\(escaped)&includeResolved=true")
        return response.threads.filter { !$0.incognito }
    }
}

/// One step of a thread's checklist: done, active (the one it's on) or pending.
struct ThreadTodo: Decodable, Hashable {
    let text: String
    let state: String
}

struct BotThreadListResponse: Decodable {
    let threads: [BotThread]
}

struct BotThread: Decodable, Identifiable, Hashable {
    let id: String
    let botId: String
    let title: String
    let kind: String
    let incognito: Bool
    let status: String
    let group: String
    let statusLine: String?
    let progress: [Int]
    let currentSessionId: String?
    let generation: Int
    let lastActivityAt: String
    /// "user" | "bot" | "import" … — main chat threads are `standing` +
    /// `import` + "Main chat" (web lib/bots/threads.ts isMainChatThread).
    var createdBy: String? = nil
    var projectId: String? = nil
    /// The bot's checklist for this thread (lib/bots/threads.ts ThreadTodo).
    var todo: [ThreadTodo]? = nil
    var summary: String? = nil

    var isMainChat: Bool {
        kind == "standing" && createdBy == "import" && title == "Main chat"
    }

    /// Waiting on you first, then working, queued, idle, resolved.
    var groupOrder: Int {
        ["waiting", "working", "queued", "idle", "resolved"].firstIndex(of: group) ?? 3
    }

    var progressLabel: String? {
        guard progress.count == 2, progress[1] > 0 else { return nil }
        return "\(progress[0])/\(progress[1])"
    }
}
