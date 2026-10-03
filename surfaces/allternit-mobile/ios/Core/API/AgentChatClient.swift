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
/// without an upload round-trip. The Fabric session-worker forwards each as
/// a gizzi `{"type":"file","url":...}` part.
struct AttachmentRef: Encodable, Sendable {
    let url: String?
    let dataBase64: String?
    let mediaType: String
    let name: String?
}

/// Errors specific to the capability-native chat path.
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

/// Client for the capability-native agent-chat protocol.
///
/// Sessions and messages are still managed by the platform gateway
/// (`POST /api/v1/agent-sessions`, `GET /api/v1/agent-sessions/:id/messages`,
/// etc.). Streaming a turn happens through the Fabric session-worker on the
/// resolved gizzi-code node: lease `harness.session.message`, invoke it, then
/// stream `AgentChatEvent` frames from the session-worker events endpoint.
///
/// The legacy `POST /api/agent-chat` streaming bridge has been removed; there
/// is no fallback path. If no node resolves, the stream fails fast with
/// `AgentChatClientError.noReachableNode`.
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
                       persona: String? = nil) async throws -> AgentSession {
        var metadata: [String: String] = [:]
        if let agentModeId { metadata["agentModeId"] = agentModeId }
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

    /// Sends `text` to the session and streams the response frames.
    ///
    /// Uses the capability-native Fabric path exclusively: resolve a gizzi-code
    /// node through `InstanceConnection`, lease `harness.session.message`,
    /// invoke it, then stream `AgentChatEvent` frames from the session-worker
    /// events endpoint.
    ///
    /// Throws `AgentChatClientError.noReachableNode` when no instance or static
    /// gizzi-code URL is available.
    func sendMessageStream(sessionId: String, text: String, agentId: String? = nil, systemPrompt: String? = nil, runtimeModelId: String? = nil, effort: String? = nil, attachments: [AttachmentRef]? = nil, tools: ToolOptions? = nil) -> AsyncThrowingStream<AgentChatEvent, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    let resolved = await InstanceConnection.resolve()
                    guard let baseURL = resolved?.baseURL else {
                        throw AgentChatClientError.noReachableNode
                    }

                    let worker = SessionWorkerClient(
                        baseURL: baseURL,
                        tokenProvider: { try await AuthManager.shared.getToken() }
                    )
                    let messageLease = try await worker.leaseSessionMessage(grantee: "ios-client")
                    let eventsLease = try await worker.issueLease(request: IssueLeaseRequest(
                        capabilityId: "harness.session.events",
                        grantee: "ios-client",
                        ttlSeconds: 300,
                        constraints: nil,
                        policy: nil
                    ))
                    let workerAttachments = (attachments ?? []).compactMap { ref -> SessionWorkerAttachment? in
                        guard let url = ref.url else { return nil }
                        return SessionWorkerAttachment(mime: ref.mediaType, url: url, filename: ref.name)
                    }
                    _ = try await worker.sendSessionMessage(
                        sessionID: sessionId,
                        text: text,
                        attachments: workerAttachments,
                        agentId: agentId,
                        systemPrompt: systemPrompt,
                        runtimeModelId: runtimeModelId,
                        effort: effort,
                        tools: tools,
                        lease: messageLease
                    )

                    for try await event in worker.streamAgentChatEvents(sessionID: sessionId, lease: eventsLease) {
                        try Task.checkCancellation()
                        continuation.yield(event)
                        if event.isTerminal {
                            continuation.finish()
                            return
                        }
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
