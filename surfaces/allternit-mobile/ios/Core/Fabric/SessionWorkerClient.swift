import Foundation

// MARK: - Session worker event

/// A raw event from `GET /v1/session-worker/sessions/:id/events`.
/// The server sends Bus events shaped as `{ type, properties }` over SSE.
enum SessionWorkerEvent: Decodable, Sendable {
    /// Initial handshake event emitted when the SSE stream opens.
    case connected(ConnectedProperties)
    /// Server heartbeat — safe to ignore for UI rendering.
    case heartbeat(sessionID: String)
    /// A message was created or replaced.
    case messageUpdated(MessageInfo)
    /// A message part was created or replaced.
    case partUpdated(PartInfo)
    /// A field on a part changed incrementally (the streaming path).
    case partDelta(PartDelta)
    /// A part was removed.
    case partRemoved(sessionID: String, messageID: String, partID: String)
    /// Session status changed (busy/idle/error).
    case statusChanged(StatusInfo)
    /// Permission/question asked by the agent.
    case asked(AskedInfo)
    /// Unknown or irrelevant event — parser tolerance.
    case unknown(type: String, properties: AnyCodable?)

    struct ConnectedProperties: Decodable, Sendable {
        let sessionID: String
        let status: AnyCodable?
    }

    struct MessageInfo: Decodable, Sendable {
        let id: String
        let role: String
        let sessionID: String?
        let content: String?
        let thinking: String?
    }

    struct PartInfo: Decodable, Sendable {
        let id: String
        let sessionID: String
        let messageID: String
        let type: String
        let text: String?
        let tool: String?
        let callID: String?
        let name: String?
        let state: PartState?

        struct PartState: Decodable, Sendable {
            let status: String
        }

        /// Flattened tool-state status, if this part is a tool part.
        var toolStatus: String? { state?.status }
    }

    struct PartDelta: Decodable, Sendable {
        let sessionID: String
        let messageID: String
        let partID: String
        let field: String
        let delta: String
    }

    struct StatusInfo: Decodable, Sendable {
        let sessionID: String
        let status: String
        let error: String?
    }

    struct AskedInfo: Decodable, Sendable {
        let sessionID: String
        let id: String
        let title: String?
        let description: String?
    }

    private enum CodingKeys: String, CodingKey {
        case type, properties
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let type = try container.decode(String.self, forKey: .type)
        self = try Self.decode(type: type, container: container)
    }

    private static func decode(type: String, container: KeyedDecodingContainer<CodingKeys>) throws -> SessionWorkerEvent {
        switch type {
        case "session-worker.connected":
            return .connected(try container.decode(ConnectedProperties.self, forKey: .properties))
        case "session-worker.heartbeat":
            struct HeartbeatProps: Decodable { let sessionID: String }
            return .heartbeat(sessionID: try container.decode(HeartbeatProps.self, forKey: .properties).sessionID)
        case "message.updated":
            return .messageUpdated(try container.decode(MessageInfo.self, forKey: .properties))
        case "message.part.updated":
            return .partUpdated(try container.decode(PartInfo.self, forKey: .properties))
        case "message.part.delta":
            return .partDelta(try container.decode(PartDelta.self, forKey: .properties))
        case "message.part.removed":
            struct RemovedProps: Decodable {
                let sessionID: String
                let messageID: String
                let partID: String
            }
            let props = try container.decode(RemovedProps.self, forKey: .properties)
            return .partRemoved(sessionID: props.sessionID, messageID: props.messageID, partID: props.partID)
        case "session.status":
            return .statusChanged(try container.decode(StatusInfo.self, forKey: .properties))
        case "permission.asked", "question.asked":
            return .asked(try container.decode(AskedInfo.self, forKey: .properties))
        default:
            return .unknown(type: type, properties: try container.decodeIfPresent(AnyCodable.self, forKey: .properties))
        }
    }
}

// MARK: - Event → AgentChatEvent mapping

extension SessionWorkerEvent {
    /// Best-effort mapping from session-worker Bus events to the
    /// `AgentChatEvent` frames the chat UI already understands.
    ///
    /// The session-worker stream is richer than the legacy agent-chat frame
    /// set, so some events map to `.ignored` until the UI grows handlers for
    /// them.
    var asAgentChatEvent: AgentChatEvent {
        switch self {
        case .partDelta(let delta):
            if delta.field == "text" {
                return .textDelta(.init(messageId: delta.messageID, partId: delta.partID, text: delta.delta))
            }
            if delta.field == "reasoning" {
                return .thinkingDelta(delta.delta)
            }
            return .ignored

        case .partUpdated(let part):
            if part.type == "tool", let name = part.tool ?? part.name, let callID = part.callID {
                switch part.toolStatus {
                case "pending", "running":
                    return .toolCall(.init(toolCallId: callID, toolName: name))
                case "completed":
                    return .toolResult(.init(toolCallId: callID, toolName: name))
                case "error":
                    return .toolError(.init(toolCallId: callID, toolName: name, error: "Tool execution failed"))
                default:
                    return .ignored
                }
            }
            return .ignored

        case .statusChanged(let info):
            if info.status == "error" {
                return .finish(.init(messageId: nil, status: "error", metadata: .init(status: "error", error: info.error, errorDetails: nil)))
            }
            if info.status == "idle" {
                return .done
            }
            return .ignored

        case .asked(let info):
            return .toolCall(.init(toolCallId: info.id, toolName: info.title ?? "Permission"))

        case .connected, .heartbeat, .messageUpdated, .partRemoved, .unknown:
            return .ignored
        }
    }
}

// MARK: - Client

/// Client for the capability-native Fabric session-worker endpoints mounted
/// under `/v1/session-worker` and `/v1/fabric` on a gizzi-code node.
final class SessionWorkerClient: @unchecked Sendable {
    let baseURL: URL
    private let session: URLSession
    private let tokenProvider: @Sendable () async throws -> String?

    init(baseURL: URL,
         session: URLSession = .shared,
         tokenProvider: @escaping @Sendable () async throws -> String?) {
        self.baseURL = baseURL
        self.session = session
        self.tokenProvider = tokenProvider
    }

    // MARK: - Fabric peers

    /// `GET /v1/fabric/peers` — returns nodes matching the optional query.
    func fetchPeers(query: CapabilityQuery? = nil) async throws -> [NodeIdentity] {
        var components = URLComponents(url: baseURL.appendingPathComponent(Self.routed("v1/fabric/peers", base: baseURL)),
                                       resolvingAgainstBaseURL: false)
        if let query {
            var items: [URLQueryItem] = []
            if let name = query.name { items.append(.init(name: "name", value: name)) }
            if let kind = query.kind { items.append(.init(name: "kind", value: kind.rawValue)) }
            if let resource = query.resource { items.append(.init(name: "resource", value: resource)) }
            if let nodeId = query.nodeId { items.append(.init(name: "nodeId", value: nodeId)) }
            components?.queryItems = items
        }
        guard let url = components?.url else { throw APIError.invalidResponse }
        return try await get(url: url)
    }

    /// `GET /v1/fabric/peers/local` — returns the connected node's identity.
    func fetchLocalPeer() async throws -> NodeIdentity {
        try await get(path: "v1/fabric/peers/local")
    }

    // MARK: - Leases

    /// `POST /v1/fabric/leases` — mints a signed lease for a capability.
    func issueLease(request: IssueLeaseRequest) async throws -> FabricLease {
        try await post(path: "v1/fabric/leases", body: request)
    }

    /// Convenience lease helper for `harness.session.message`.
    func leaseSessionMessage(grantee: String, ttlSeconds: Int = 300) async throws -> FabricLease {
        try await issueLease(request: IssueLeaseRequest(
            capabilityId: "harness.session.message",
            grantee: grantee,
            ttlSeconds: ttlSeconds,
            constraints: nil,
            policy: nil
        ))
    }

    // MARK: - Invocation

    /// `POST /v1/session-worker/invoke` — invokes a capability with an
    /// optional lease. Sends `X-Allternit-Lease` when `lease` is provided.
    func invoke(capability: String,
                inputs: [String: AnyCodable],
                lease: FabricLease? = nil) async throws -> InvocationResult {
        var request = try await authorizedRequest(path: "v1/session-worker/invoke", method: "POST")
        if let lease {
            request.setValue(lease.signature, forHTTPHeaderField: "X-Allternit-Lease")
        }
        request.httpBody = try JSONEncoder().encode(CapabilityInvocationRequest(capability: capability, inputs: inputs))
        let (data, response) = try await InstanceConnection.data(for: request, session: session)
        try validate(response, data: data)
        return try JSONDecoder().decode(InvocationResult.self, from: data)
    }

    /// Convenience invocation for `harness.session.message`.
    func sendSessionMessage(sessionID: String,
                            text: String,
                            attachments: [SessionWorkerAttachment] = [],
                            agentId: String? = nil,
                            systemPrompt: String? = nil,
                            runtimeModelId: String? = nil,
                            effort: String? = nil,
                            tools: ToolOptions? = nil,
                            lease: FabricLease) async throws -> InvocationResult {
        let attachmentInputs = attachments.map { attachment in
            [
                "mime": AnyCodable(attachment.mime),
                "url": AnyCodable(attachment.url),
                "filename": AnyCodable(attachment.filename),
            ]
        }
        var inputs: [String: AnyCodable] = [
            "sessionID": AnyCodable(sessionID),
            "text": AnyCodable(text),
        ]
        if !attachmentInputs.isEmpty {
            inputs["attachments"] = AnyCodable(attachmentInputs)
        }
        if let agentId, !agentId.isEmpty {
            inputs["agent"] = AnyCodable(agentId)
        }
        if let systemPrompt, !systemPrompt.isEmpty {
            inputs["system"] = AnyCodable(systemPrompt)
        }
        if let runtimeModelId, !runtimeModelId.isEmpty,
           let model = Self.parseRuntimeModelId(runtimeModelId) {
            inputs["model"] = AnyCodable(model)
        }
        if let effort, !effort.isEmpty {
            inputs["effort"] = AnyCodable(effort)
        }
        var metadata: [String: AnyCodable] = [:]
        if let tools {
            metadata["tools"] = AnyCodable(tools)
        }
        if !metadata.isEmpty {
            inputs["metadata"] = AnyCodable(metadata)
        }
        return try await invoke(capability: "harness.session.message", inputs: inputs, lease: lease)
    }

    private static func parseRuntimeModelId(_ id: String) -> [String: String]? {
        let separators = CharacterSet(charactersIn: "/:")
        let parts = id.components(separatedBy: separators).filter { !$0.isEmpty }
        guard parts.count >= 2 else { return nil }
        return ["providerID": parts[0], "modelID": parts[1]]
    }

    // MARK: - Session events (SSE)

    /// `GET /v1/session-worker/sessions/:id/events` — returns an async stream
    /// of `SessionWorkerEvent` values parsed from SSE `data:` lines.
    /// Pass a `harness.session.events` lease when the runtime enforces leases.
    func streamEvents(sessionID: String, lease: FabricLease? = nil) -> AsyncThrowingStream<SessionWorkerEvent, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    var request = try await self.authorizedRequest(
                        path: "v1/session-worker/sessions/\(Self.escape(sessionID))/events"
                    )
                    if let lease {
                        request.setValue(lease.signature, forHTTPHeaderField: "X-Allternit-Lease")
                    }
                    let (bytes, response) = try await InstanceConnection.bytes(for: request, session: self.session)
                    try self.validate(response, data: nil)

                    let decoder = JSONDecoder()
                    for try await line in bytes.lines {
                        try Task.checkCancellation()
                        guard line.hasPrefix("data: ") else { continue }
                        let payload = String(line.dropFirst(6))
                        guard !payload.isEmpty else { continue }
                        if payload == "[DONE]" { continuation.finish(); return }
                        guard let data = payload.data(using: .utf8),
                              let event = try? decoder.decode(SessionWorkerEvent.self, from: data) else {
                            print("session-worker: skipping undecodable frame: \(payload)")
                            continue
                        }
                        continuation.yield(event)
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    /// `GET /v1/session-worker/sessions/:id/events` mapped to the existing
    /// `AgentChatEvent` model so `ChatViewModel` does not change.
    func streamAgentChatEvents(sessionID: String, lease: FabricLease? = nil) -> AsyncThrowingStream<AgentChatEvent, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    for try await event in self.streamEvents(sessionID: sessionID, lease: lease) {
                        try Task.checkCancellation()
                        let mapped = event.asAgentChatEvent
                        if case .ignored = mapped { continue }
                        continuation.yield(mapped)
                        if mapped.isTerminal {
                            continuation.finish()
                            return
                        }
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    // MARK: - Generic REST helpers

    private func get<T: Decodable>(path: String) async throws -> T {
        let request = try await authorizedRequest(path: path)
        let (data, response) = try await InstanceConnection.data(for: request, session: session)
        try validate(response, data: data)
        return try JSONDecoder().decode(T.self, from: data)
    }

    private func get<T: Decodable>(url: URL) async throws -> T {
        let request = try await authorizedRequest(url: url)
        let (data, response) = try await InstanceConnection.data(for: request, session: session)
        try validate(response, data: data)
        return try JSONDecoder().decode(T.self, from: data)
    }

    private func post<T: Decodable, B: Encodable>(path: String, body: B) async throws -> T {
        var request = try await authorizedRequest(path: path, method: "POST")
        request.httpBody = try JSONEncoder().encode(body)
        let (data, response) = try await InstanceConnection.data(for: request, session: session)
        try validate(response, data: data)
        return try JSONDecoder().decode(T.self, from: data)
    }

    private func authorizedRequest(path: String, method: String = "GET") async throws -> URLRequest {
        let url = baseURL.appendingPathComponent(Self.routed(path, base: baseURL))
        return try await authorizedRequest(url: url, method: method)
    }

    private func authorizedRequest(url: URL, method: String = "GET") async throws -> URLRequest {
        var request = URLRequest(url: url)
        request.httpMethod = method
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        request.setValue("text/event-stream", forHTTPHeaderField: "Accept")
        if let token = try await tokenProvider(), !token.isEmpty {
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        #if DEBUG
        if CommandLine.arguments.contains("-skip-auth") {
            request.setValue("dev-ios-tester", forHTTPHeaderField: "x-allternit-user-id")
            request.setValue("dev", forHTTPHeaderField: "x-allternit-desktop-access-token")
            if request.value(forHTTPHeaderField: "Authorization") == nil {
                request.setValue("Bearer dev-api-token", forHTTPHeaderField: "Authorization")
            }
        }
        #endif
        return request
    }

    private func validate(_ response: URLResponse, data: Data?) throws {
        guard let httpResponse = response as? HTTPURLResponse else {
            throw APIError.invalidResponse
        }
        guard (200...299).contains(httpResponse.statusCode) else {
            throw APIError.httpError(statusCode: httpResponse.statusCode, message: nil)
        }
    }

    /// Over the runtime relay the computer's allternit-api answers, and it
    /// mirrors gizzi's `/v1/fabric/*` and `/v1/session-worker/*` under
    /// `/api/v1/...` (allternit-api fabric_routes.rs; Allternit Desktop
    /// serves the same `/api/v1/fabric/*` paths from its relay).
    private static func routed(_ path: String, base: URL) -> String {
        RuntimeRelayClient.isRelayURL(base) ? "api/\(path)" : path
    }

    private static func escape(_ sessionID: String) -> String {
        sessionID.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? sessionID
    }
}

// MARK: - Attachment input

/// Attachment shape expected by `harness.session.message` inputs
/// (`{ mime, url, filename }`).
struct SessionWorkerAttachment: Sendable {
    let mime: String
    let url: String
    let filename: String?
}
