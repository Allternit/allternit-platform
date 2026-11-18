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
        try await client.get(path: "agent-sessions/\(Self.escape(sessionId))/tasks")
    }

    /// Stops a background subagent for good (kills its run).
    func cancel(sessionId: String, taskId: String) async throws {
        try await client.post(path: taskPath(sessionId, taskId, "cancel"))
    }

    /// Pauses a background subagent: its current turn stops and the run
    /// settles as `interrupted`; the child session stays for Resume.
    func interrupt(sessionId: String, taskId: String, reason: String? = nil) async throws {
        try await client.post(path: taskPath(sessionId, taskId, "interrupt"), body: InterruptBody(reason: reason))
    }

    /// Starts a new run on the same child, continuing where it stopped.
    func resume(sessionId: String, taskId: String, message: String? = nil) async throws {
        try await client.post(path: taskPath(sessionId, taskId, "resume"), body: ResumeBody(message: message))
    }

    private struct InterruptBody: Encodable { let reason: String? }
    private struct ResumeBody: Encodable { let message: String? }

    private func taskPath(_ sessionId: String, _ taskId: String, _ action: String) -> String {
        "agent-sessions/\(Self.escape(sessionId))/background-tasks/\(Self.escape(taskId))/\(action)"
    }

    private static func escape(_ s: String) -> String {
        var allowed = CharacterSet.urlPathAllowed
        allowed.remove("/")
        return s.addingPercentEncoding(withAllowedCharacters: allowed) ?? s
    }
}

/// What a run tree card can ask of a background subagent.
enum SessionTaskAction: String, Sendable {
    case stop, pause, resume

    var label: String {
        switch self {
        case .stop: return "Stop"
        case .pause: return "Pause"
        case .resume: return "Resume"
        }
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
    /// Task ids with a Stop / Pause / Resume request in flight.
    @Published private(set) var busyTaskIds: Set<String> = []
    /// The last control request that failed, shown under the tree.
    @Published var actionError: String? = nil

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

    /// Stop, Pause or Resume one background subagent, then refresh so the
    /// card shows the run's new state without waiting for the next poll.
    func perform(_ action: SessionTaskAction, taskId: String) async {
        guard let sessionId, !busyTaskIds.contains(taskId) else { return }
        busyTaskIds.insert(taskId)
        defer { busyTaskIds.remove(taskId) }
        do {
            switch action {
            case .stop: try await client.cancel(sessionId: sessionId, taskId: taskId)
            case .pause: try await client.interrupt(sessionId: sessionId, taskId: taskId, reason: "Paused from the iOS app")
            case .resume: try await client.resume(sessionId: sessionId, taskId: taskId)
            }
            actionError = nil
        } catch {
            actionError = "Couldn't \(action.label.lowercased()) the agent: \(FriendlyErrorMessage.from(error.localizedDescription))"
        }
        await refresh(sessionId: sessionId)
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
