import Foundation

/// Composer "+" sheet tool options, sent as `metadata.tools` on every
/// agent-chat request (ToolOptionsStore is the persisted source).
struct ToolOptions: Encodable, Sendable {
    let webSearch: Bool
    let research: Bool
    /// "auto" | "on_demand" | "always" (ToolAccess raw values).
    let toolAccess: String
    /// Optional response style override: "formal" | "creative" | "technical".
    let style: String?
}

/// One composer-staged attachment in the agent-chat body. `url` is the
/// `POST /api/v1/uploads` result; `dataBase64` inlines small payloads
/// without an upload round-trip. The agent-chat bridge forwards each as
/// a gizzi `{"type":"file","url":...}` part.
struct AttachmentRef: Encodable, Sendable {
    let url: String?
    let dataBase64: String?
    let mediaType: String
    let name: String?
}

/// Errors specific to the chat path.
enum AgentChatClientError: Error, LocalizedError {
    /// No gizzi-code/Fabric node is currently reachable.
    case noReachableNode

    var errorDescription: String? {
        switch self {
        case .noReachableNode:
            return "No reachable harness. Make sure Allternit Desktop is running or a runtime backend is paired."
        }
    }
}

/// Client for the agent-chat protocol.
///
/// Sessions and messages are managed by the platform gateway
/// (`POST /api/v1/agent-sessions`, `GET /api/v1/agent-sessions/:id/messages`,
/// etc.). A turn streams from `POST /api/agent-chat` on the resolved
/// computer, as on the web. If no computer resolves, the stream fails fast
/// with `AgentChatClientError.noReachableNode`.
final class AgentChatClient: ObservableObject, @unchecked Sendable {
    private let client: APIClient

    init(client: APIClient = .shared) {
        self.client = client
    }

    /// Response of `POST /api/v1/uploads` (upload_routes.rs).
    struct UploadResponse: Decodable {
        let uploadId: String
        let url: String
    }

    private struct UploadRequest: Encodable {
        let name: String
        let mediaType: String
        let dataBase64: String
    }

    // MARK: - Sessions (REST, /api/v1/agent-sessions)

    /// Creates a session; returns the `ses_*` session record.
    ///
    /// `originSurface` is the current app mode ("chat" | "cowork"),
    /// `sessionMode` is "agent" when the composer's agent pill is on
    /// (otherwise "regular"). `agentModeId` is the selected bottom-deck
    /// tile, carried as `metadata.agentModeId` like the web
    /// (mode-session-store.ts:897-905). `projectId` is the selected cowork
    /// project, carried as `metadata.projectId` — the backend stamps it onto
    /// the gizzi session so list responses can group chats by project
    /// (agent_session_routes.rs create_session). `ephemeral` marks an
    /// incognito chat (Phase 6), carried as `metadata.ephemeral`. `persona`
    /// is the onboarding work-profile answer (Phase 10), carried as
    /// `metadata.persona`.
    func createSession(name: String,
                       originSurface: String,
                       sessionMode: String,
                       agentId: String? = nil,
                       agentName: String? = nil,
                       agentModeId: String? = nil,
                       projectId: String? = nil,
                       ephemeral: Bool = false,
                       persona: String? = nil,
                       permissionMode: String? = nil) async throws -> AgentSession {
        var metadata: [String: String] = [:]
        if let agentModeId { metadata["agentModeId"] = agentModeId }
        // The composer's Auto · Ask · Read-only pill (web permission-mode.ts),
        // read by the agent-chat bridge as `permissionMode`.
        if let permissionMode { metadata["permissionMode"] = permissionMode }
        if let projectId { metadata["projectId"] = projectId }
        if let persona { metadata["persona"] = persona }
        // Incognito chats (Phase 6): carried as `metadata.ephemeral = "true"`
        // (the metadata bag is string-valued); the backend also accepts a
        // top-level bool. Ephemeral sessions are excluded from list responses
        // and purged on abort (agent_session_routes.rs create_session).
        if ephemeral { metadata["ephemeral"] = "true" }
        let body = CreateAgentSessionRequest(
            name: name,
            originSurface: originSurface,
            sessionMode: sessionMode,
            agentId: agentId,
            agentName: agentName,
            metadata: metadata.isEmpty ? nil : metadata
        )
        return try await client.post(path: "agent-sessions", body: body)
    }

    /// Merges `metadata` into a session (`PATCH /api/v1/agent-sessions/:id`,
    /// merge semantics — web native-agent-api.ts updateSession).
    func updateSessionMetadata(sessionId: String, metadata: [String: String]) async throws {
        struct Body: Encodable { let metadata: [String: String] }
        try await client.patch(path: "agent-sessions/\(Self.escape(sessionId))", body: Body(metadata: metadata))
    }

    /// Lists all sessions (the production sidebar's history source).
    func listSessions() async throws -> [AgentSession] {
        let response: AgentSessionListResponse = try await client.get(path: "agent-sessions")
        return response.sessions
    }

    // MARK: - Agent registry (REST, /api/v1/agents)

    /// Lists registered agents — the data source for the composer's agent
    /// selector (web: agent.service.ts:67-92 → GET /api/v1/agents).
    func listAgents() async throws -> [AgentSummary] {
        let response: AgentListResponse = try await client.get(path: "agents")
        return response.agents
    }

    /// Lists the flattened runtime-model catalog (`GET /api/v1/models`, a
    /// bare array) — the composer model pill's data source.
    func listModels() async throws -> [RuntimeModel] {
        try await client.get(path: "models")
    }

    /// Lists all messages of a session (history view + foreground reconcile).
    func listMessages(sessionId: String) async throws -> [AgentSessionMessage] {
        try await client.get(path: "agent-sessions/\(Self.escape(sessionId))/messages")
    }

    /// The session's context window (`GET agent-sessions/:id/context`, gizzi
    /// `/session/:id/context`; plan limits left out). nil before the
    /// session's first turn.
    func sessionContext(sessionId: String) async throws -> SessionContextBreakdown? {
        let response: SessionContextResponse = try await client.get(path: "agent-sessions/\(Self.escape(sessionId))/context?limits=false")
        return response.breakdown
    }

    /// Everything the session delegated, newest first: background shells and
    /// subagents (`GET agent-sessions/:id/tasks`, gizzi `SessionTasks.Item`).
    func sessionTasks(sessionId: String) async throws -> [SessionTask] {
        try await client.get(path: "agent-sessions/\(Self.escape(sessionId))/tasks")
    }

    /// Stops a running background task (kills a shell, aborts a subagent).
    func cancelBackgroundTask(sessionId: String, taskId: String) async throws {
        try await client.post(path: "agent-sessions/\(Self.escape(sessionId))/background-tasks/\(Self.escape(taskId))/cancel")
    }

    /// The pull requests the session opened or pushed to
    /// (`GET agent-sessions/prs?ids=<id>` → `{ bySession: { id: [pr] } }`).
    func sessionPullRequests(sessionId: String) async throws -> [LinkedPullRequest] {
        let ids = sessionId.addingPercentEncoding(withAllowedCharacters: .alphanumerics) ?? sessionId
        let response: SessionPullRequestsResponse = try await client.get(path: "agent-sessions/prs?ids=\(ids)")
        return response.bySession?[sessionId] ?? []
    }

    /// Aborts an in-flight generation server-side.
    func abort(sessionId: String) async throws {
        try await client.post(path: "agent-sessions/\(Self.escape(sessionId))/abort")
    }

    /// Conversation revert (Phase 8 edit-resend), live on the backend
    /// (agent_session_routes.rs — probed 2026-07: 200 on a valid session).
    func revert(sessionId: String) async throws {
        try await client.post(path: "agent-sessions/\(Self.escape(sessionId))/revert")
    }

    // MARK: - Uploads (POST /api/v1/uploads)

    /// Uploads one staged attachment; returns the fetchable `{uploadId, url}`
    /// ref the agent-chat body then references.
    func upload(name: String, mediaType: String, dataBase64: String) async throws -> UploadResponse {
        try await client.post(
            path: "uploads",
            body: UploadRequest(name: name, mediaType: mediaType, dataBase64: dataBase64)
        )
    }

    // MARK: - Chat streaming

    /// Body of `POST /api/agent-chat`, the same body m.allternit.com sends
    /// (native-agent-api.ts chatApi.streamChat). `metadata.tools` carries the
    /// "+" sheet's tool options; the bridge turns `attachments` into gizzi
    /// file parts.
    private struct AgentChatRequest: Encodable {
        struct Metadata: Encodable { let tools: ToolOptions }
        let chatId: String
        let message: String
        let agentId: String?
        let systemPrompt: String?
        let runtimeModelId: String?
        let effort: String?
        let attachments: [AttachmentRef]?
        let metadata: Metadata?
    }

    /// Sends `text` to the session and streams the response frames.
    ///
    /// `POST /api/agent-chat` on the resolved computer (relay, mesh or
    /// direct gizzi-code, which mounts it under `/api`); the response body is
    /// the `AgentChatEvent` SSE stream, as on the web. The session-worker
    /// invoke + events path is not used: computers answer invoke but don't
    /// serve `/v1/session-worker/sessions/:id/events` (404), so the reply
    /// never reached the phone.
    ///
    /// Throws `AgentChatClientError.noReachableNode` when no computer resolves.
    func sendMessageStream(sessionId: String, text: String, agentId: String? = nil, systemPrompt: String? = nil, runtimeModelId: String? = nil, effort: String? = nil, attachments: [AttachmentRef]? = nil, tools: ToolOptions? = nil) -> AsyncThrowingStream<AgentChatEvent, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    guard let baseURL = await InstanceConnection.resolve()?.baseURL else {
                        throw AgentChatClientError.noReachableNode
                    }
                    var request = URLRequest(url: baseURL.appendingPathComponent("api/agent-chat"))
                    request.httpMethod = "POST"
                    request.setValue("application/json", forHTTPHeaderField: "Content-Type")
                    request.setValue("text/event-stream", forHTTPHeaderField: "Accept")
                    if let token = try await AuthManager.shared.getToken(), !token.isEmpty {
                        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
                    }
                    // Streams are long-lived; the default 60s idle timeout
                    // would kill slow generations between frames.
                    request.timeoutInterval = 600
                    request.httpBody = try JSONEncoder().encode(AgentChatRequest(
                        chatId: sessionId,
                        message: text,
                        agentId: agentId,
                        systemPrompt: systemPrompt,
                        runtimeModelId: runtimeModelId,
                        effort: effort,
                        attachments: attachments,
                        metadata: tools.map { AgentChatRequest.Metadata(tools: $0) }
                    ))

                    let (bytes, response) = try await InstanceConnection.bytes(for: request)
                    guard let http = response as? HTTPURLResponse else { throw APIError.invalidResponse }
                    guard (200...299).contains(http.statusCode) else {
                        throw APIError.httpError(statusCode: http.statusCode, message: nil)
                    }

                    let decoder = JSONDecoder()
                    for try await line in bytes.lines {
                        try Task.checkCancellation()
                        // SSE frames are `data: {json}`; keep-alive comments
                        // and event lines are skipped.
                        guard line.hasPrefix("data: ") else { continue }
                        let payload = String(line.dropFirst(6))
                        guard !payload.isEmpty else { continue }
                        if payload == "[DONE]" { break }
                        guard let data = payload.data(using: .utf8),
                              let event = try? decoder.decode(AgentChatEvent.self, from: data) else {
                            print("agent-chat: skipping undecodable frame: \(payload)")
                            continue
                        }
                        if case .ignored = event { continue }
                        continuation.yield(event)
                        if event.isTerminal { break }
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }

            // Cancelling the consumer cancels the Task, which cancels the
            // underlying URLSession stream. Server-side abort is the caller's
            // job (ChatViewModel.stopStreaming → abort(sessionId:)).
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    /// Web uses `encodeURIComponent` on session ids (ses_* ids are already
    /// path-safe; this is parity insurance).
    private static func escape(_ sessionId: String) -> String {
        sessionId.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? sessionId
    }
}
