import Foundation

/// `GET /api/v1/agent-sessions/:id/tasks` → bare `SessionTask[]` (gizzi
/// session tasks: background shells and subagents, with nested children on
/// newer runtimes). Relayed by cloud-api routes/agent_sessions.rs.
final class SessionTasksClient: @unchecked Sendable {
    static let shared = SessionTasksClient()

    private let client: APIClient

    init(client: APIClient = .shared) {
        self.client = client
    }

    func listTasks(sessionId: String) async throws -> [SessionTask] {
        let id = sessionId.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? sessionId
        return try await client.get(path: "agent-sessions/\(id)/tasks")
    }
}

/// One session's task list, kept live while a run tree is on screen —
/// polls every 2 s while any task runs, every 15 s otherwise (the web
/// session-tasks.store active/idle cadence).
@MainActor
final class SessionTasksStore: ObservableObject {
    @Published private(set) var tasks: [SessionTask] = []
    @Published private(set) var loadError: String? = nil
    @Published private(set) var hasLoaded = false

    private let sessionId: String?
    private let client: SessionTasksClient
    private var pollTask: Task<Void, Never>? = nil

    init(sessionId: String?, client: SessionTasksClient = .shared) {
        self.sessionId = sessionId
        self.client = client
    }

    func start() {
        guard pollTask == nil, let sessionId else {
            hasLoaded = true
            return
        }
        pollTask = Task { [weak self] in
            while !Task.isCancelled {
                guard let self else { return }
                await self.refresh(sessionId: sessionId)
                let active = self.tasks.contains { $0.isLive || $0.children.contains(where: \.isLive) }
                try? await Task.sleep(nanoseconds: active ? 2_000_000_000 : 15_000_000_000)
            }
        }
    }

    func stop() {
        pollTask?.cancel()
        pollTask = nil
    }

    private func refresh(sessionId: String) async {
        do {
            tasks = try await client.listTasks(sessionId: sessionId)
            loadError = nil
        } catch {
            // Older runtimes have no tasks route; the tree still draws from
            // the reply's own steps, so this is a quiet note, not a wall.
            if tasks.isEmpty {
                loadError = FriendlyErrorMessage.from(error.localizedDescription)
            }
        }
        hasLoaded = true
    }
}
