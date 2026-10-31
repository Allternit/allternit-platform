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
    /// Permission asked by the agent (`permission.asked`).
    case asked(AskedInfo)
    /// Question asked by the agent (`question.asked`, gizzi `Question.Request`).
    case questionAsked(AgentQuestionRequest)
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

        /// gizzi `MessageV2.ToolPart.state`: `{ status, input, title?, error?,
        /// time: { start, end? } }`. Decoded tolerantly — a tool state this
        /// client doesn't know must not drop the frame.
        struct PartState: Decodable, Sendable {
            let status: String
            let input: [String: String]
            let title: String?
            let error: String?
            let startedAt: Date?
            let finishedAt: Date?

            private enum CodingKeys: String, CodingKey { case status, input, title, error, time }
            private struct Time: Decodable { let start: Double?; let end: Double? }

            init(from decoder: Decoder) throws {
                let c = try decoder.container(keyedBy: CodingKeys.self)
                status = ((try? c.decodeIfPresent(String.self, forKey: .status)) ?? nil) ?? "pending"
                title = (try? c.decodeIfPresent(String.self, forKey: .title)) ?? nil
                error = (try? c.decodeIfPresent(String.self, forKey: .error)) ?? nil
                let raw = (try? c.decodeIfPresent([String: AnyCodable].self, forKey: .input)) ?? nil
                input = PartState.flatten(raw)
                let time = (try? c.decodeIfPresent(Time.self, forKey: .time)) ?? nil
                startedAt = time?.start.map { Date(timeIntervalSince1970: $0 / 1000) }
                finishedAt = time?.end.map { Date(timeIntervalSince1970: $0 / 1000) }
            }

            /// String-valued input fields (numbers/bools stringified; nested
            /// objects dropped) — what the rail needs to describe a step.
            static func flatten(_ raw: [String: AnyCodable]?) -> [String: String] {
                var out: [String: String] = [:]
                for (key, value) in raw ?? [:] {
                    switch value.value {
                    case let s as String: out[key] = s
                    case let b as Bool: out[key] = b ? "true" : "false"
                    case let i as Int: out[key] = String(i)
                    case let d as Double: out[key] = String(d)
                    default: break
                    }
                }
                return out
            }
        }

        /// Flattened tool-state status, if this part is a tool part.
        var toolStatus: String? { state?.status }

        /// The tool detail the chat's tool calls rail draws.
        var toolDetail: AgentChatEvent.ToolDetail? {
            guard let state else { return nil }
            return AgentChatEvent.ToolDetail(
                input: state.input,
                title: state.title,
                error: state.error,
                startedAt: state.startedAt,
                finishedAt: state.finishedAt
            )
        }
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
        /// "idle" | "busy" | "retry" | "error". gizzi sends `status` as
        /// `{ type, … }` (SessionStatus.Info); older shims send a string.
        let status: String
        let error: String?

        private enum CodingKeys: String, CodingKey { case sessionID, status, error }
        private struct Info: Decodable { let type: String?; let message: String? }

        init(sessionID: String, status: String, error: String?) {
            self.sessionID = sessionID
            self.status = status
            self.error = error
        }

        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            sessionID = ((try? c.decodeIfPresent(String.self, forKey: .sessionID)) ?? nil) ?? ""
            let info = (try? c.decodeIfPresent(Info.self, forKey: .status)) ?? nil
            status = ((try? c.decodeIfPresent(String.self, forKey: .status)) ?? nil) ?? info?.type ?? "busy"
            error = ((try? c.decodeIfPresent(String.self, forKey: .error)) ?? nil) ?? info?.message
        }
    }

    struct AskedInfo: Decodable, Sendable {
        let sessionID: String
        let id: String
        let title: String?
        let description: String?
        /// gizzi `PermissionNext.Request` fields (absent on older shims).
        let permission: String?
        let patterns: [String]?
        let tool: ToolRef?

        struct ToolRef: Decodable, Sendable { let messageID: String?; let callID: String? }

        /// The same payload as the shared permission model the rail and the
        /// permission card use. Nil when the frame isn't a gizzi permission.
        var permissionAsk: AgentPermissionAsk? {
            guard permission != nil else { return nil }
            var obj: [String: Any] = ["id": id, "sessionID": sessionID, "permission": permission ?? "tool", "patterns": patterns ?? []]
            if let callID = tool?.callID { obj["tool"] = ["callID": callID] }
            guard let data = try? JSONSerialization.data(withJSONObject: obj) else { return nil }
            return try? JSONDecoder().decode(AgentPermissionAsk.self, from: data)
        }
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
        case "session-worker.connected", "remote.connected":
            return .connected(try container.decode(ConnectedProperties.self, forKey: .properties))
        case "session-worker.heartbeat", "remote.heartbeat":
            struct HeartbeatProps: Decodable { let sessionID: String }
            return .heartbeat(sessionID: try container.decode(HeartbeatProps.self, forKey: .properties).sessionID)
        case "message.updated":
            return .messageUpdated(try container.decode(MessageInfo.self, forKey: .properties))
        case "message.part.updated":
            // gizzi's bus sends `{ part, delta? }`; older shims sent the part flat.
            struct PartEnvelope: Decodable { let part: PartInfo }
            if let envelope = try? container.decode(PartEnvelope.self, forKey: .properties) {
                return .partUpdated(envelope.part)
            }
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
        case "session.idle":
            struct IdleProps: Decodable { let sessionID: String }
            let props = try container.decode(IdleProps.self, forKey: .properties)
            return .statusChanged(StatusInfo(sessionID: props.sessionID, status: "idle", error: nil))
        case "permission.asked":
            return .asked(try container.decode(AskedInfo.self, forKey: .properties))
        case "question.asked":
            return .questionAsked(try container.decode(AgentQuestionRequest.self, forKey: .properties))
        default:
            return .unknown(type: type, properties: try container.decodeIfPresent(AnyCodable.self, forKey: .properties))
        }
    }
}

// MARK: - Client

/// Client for the Fabric peer endpoints (`/v1/fabric/peers*`) on a
/// gizzi-code node. Chat turns don't go through here: they stream from
/// `POST /api/agent-chat` (AgentChatClient).
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
}
