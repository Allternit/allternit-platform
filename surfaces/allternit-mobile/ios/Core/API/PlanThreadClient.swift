import Foundation

/// cloud-api's plan chat store (CONTRACT-connections-2026-11 §9.10):
/// `POST/GET /api/v1/plan-threads`, `GET .../:id`, `POST .../:id/messages`, `DELETE .../:id`.
/// Needs the person's signed-in session; every failure throws, and callers
/// fall back to device-only without telling the person.
struct PlanThreadSummary: Decodable, Equatable, Sendable {
    let id: String
    let title: String?
    let model: String
    let messageCount: Int
    let createdAt: String
    let updatedAt: String

    enum CodingKeys: String, CodingKey {
        case id, title, model
        case messageCount = "message_count"
        case createdAt = "created_at"
        case updatedAt = "updated_at"
    }
}

struct PlanThreadMessage: Codable, Equatable, Sendable {
    let role: String
    let content: String
    let model: String?
    let at: String?

    init(role: String, content: String, model: String? = nil, at: String? = nil) {
        self.role = role
        self.content = content
        self.model = model
        self.at = at
    }
}

struct PlanThreadDetail: Decodable, Equatable, Sendable {
    let summary: PlanThreadSummary
    let messages: [PlanThreadMessage]

    init(from decoder: Decoder) throws {
        summary = try PlanThreadSummary(from: decoder)
        messages = try decoder.container(keyedBy: Keys.self).decodeIfPresent([PlanThreadMessage].self, forKey: .messages) ?? []
    }
    private enum Keys: String, CodingKey { case messages }
}

struct PlanThreadClient: Sendable {
    static let maxPerAppend = 50
    static let idPrefix = "pth_"

    var baseURL: URL = AppConfig.cloudAPIBaseURL
    var session: URLSession = .shared
    var tokenProvider: @Sendable () async throws -> String? = { try await AuthManager.shared.getToken() }

    static let shared = PlanThreadClient()

    /// A plan/… or copilot/… model: the ones the store keeps (not `chatgpt/…`).
    static func isStored(model: String) -> Bool { model.hasPrefix("plan/") || model.hasPrefix("copilot/") }
    static func isThreadId(_ id: String) -> Bool { id.hasPrefix(idPrefix) }

    func create(model: String, title: String? = nil, messages: [PlanThreadMessage]? = nil) async throws -> PlanThreadSummary {
        struct Body: Encodable { let model: String; let title: String?; let messages: [PlanThreadMessage]? }
        return try await send("POST", "api/v1/plan-threads", Body(model: model, title: title, messages: messages))
    }

    func list() async throws -> [PlanThreadSummary] {
        struct Envelope: Decodable { let threads: [PlanThreadSummary] }
        let envelope: Envelope = try await send("GET", "api/v1/plan-threads", Optional<String>.none)
        return envelope.threads
    }

    func get(id: String) async throws -> PlanThreadDetail {
        try await send("GET", "api/v1/plan-threads/\(id)", Optional<String>.none)
    }

    @discardableResult
    func append(id: String, messages: [PlanThreadMessage]) async throws -> PlanThreadSummary {
        struct Body: Encodable { let messages: [PlanThreadMessage] }
        return try await send("POST", "api/v1/plan-threads/\(id)/messages", Body(messages: Array(messages.prefix(Self.maxPerAppend))))
    }

    /// DELETE /api/v1/plan-threads/:id (204). A 404 means it is already gone, which is the goal.
    func delete(id: String) async throws {
        guard let token = try await tokenProvider(), !token.isEmpty else { throw APIError.invalidResponse }
        let escaped = id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? id
        var request = URLRequest(url: baseURL.appendingPathComponent("api/v1/plan-threads/\(escaped)"))
        request.httpMethod = "DELETE"
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        request.timeoutInterval = 20
        let (_, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse, (200...299).contains(http.statusCode) || http.statusCode == 404 else {
            throw APIError.invalidResponse
        }
    }

    private func send<T: Decodable, B: Encodable>(_ method: String, _ path: String, _ body: B?) async throws -> T {
        guard let token = try await tokenProvider(), !token.isEmpty else { throw APIError.invalidResponse }
        var request = URLRequest(url: baseURL.appendingPathComponent(path))
        request.httpMethod = method
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        request.timeoutInterval = 20
        if let body {
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = try JSONEncoder().encode(body)
        }
        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse, (200...299).contains(http.statusCode) else {
            throw APIError.invalidResponse
        }
        return try JSONDecoder().decode(T.self, from: data)
    }
}

extension PlanThreadSummary {
    /// The chat list row for a thread: an `AgentSession` the existing list shows.
    var asSession: AgentSession? {
        var object: [String: Any] = [
            "id": id, "created_at": createdAt, "updated_at": updatedAt,
            "message_count": messageCount, "metadata": ["originSurface": "chat"],
        ]
        if let title, !title.isEmpty { object["name"] = title }
        guard let data = try? JSONSerialization.data(withJSONObject: object) else { return nil }
        return try? JSONDecoder().decode(AgentSession.self, from: data)
    }
}
