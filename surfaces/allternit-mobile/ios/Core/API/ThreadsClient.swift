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

    /// Waiting on you first, then working, queued, idle, resolved.
    var groupOrder: Int {
        ["waiting", "working", "queued", "idle", "resolved"].firstIndex(of: group) ?? 3
    }

    var progressLabel: String? {
        guard progress.count == 2, progress[1] > 0 else { return nil }
        return "\(progress[0])/\(progress[1])"
    }
}
