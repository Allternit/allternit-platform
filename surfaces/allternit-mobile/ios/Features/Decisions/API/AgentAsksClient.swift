import Foundation

/// Questions and permissions an agent is waiting on — the same routes the
/// PWA uses (allternit-ai native-agent-api.ts `questionsApi` /
/// `permissionsApi`; served by allternit-api agent_session_routes.rs and
/// relayed by cloud-api routes/agent_sessions.rs).
///
/// - `GET  /api/v1/questions[?sessionId=]` → `{ questions: [Question.Request] }`
/// - `POST /api/v1/questions/:id/reply` `{ answers: [{ questionIndex, answer }] }`
/// - `POST /api/v1/questions/:id/reject` `{}`
/// - `GET  /api/v1/permissions?sessionId=` → `{ permissions: [PermissionNext.Request] }`
/// - `POST /api/v1/permissions/:id/reply` `{ reply: once|always|reject }`
final class AgentAsksClient: @unchecked Sendable {
    static let shared = AgentAsksClient()

    private let client: APIClient

    init(client: APIClient = .shared) {
        self.client = client
    }

    private struct QuestionsEnvelope: Decodable { let questions: [AgentQuestionRequest]? }
    private struct PermissionsEnvelope: Decodable { let permissions: [AgentPermissionAsk]? }

    /// One answer: a label, several labels (multi-select / a tree path), or
    /// typed text. Empty string = skipped.
    struct ReplyAnswer: Encodable, Sendable, Equatable {
        enum Value: Sendable, Equatable {
            case one(String)
            case many([String])
        }
        let questionIndex: Int
        let answer: Value

        private enum CodingKeys: String, CodingKey { case questionIndex, answer }

        func encode(to encoder: Encoder) throws {
            var c = encoder.container(keyedBy: CodingKeys.self)
            try c.encode(questionIndex, forKey: .questionIndex)
            switch answer {
            case .one(let text): try c.encode(text, forKey: .answer)
            case .many(let labels): try c.encode(labels, forKey: .answer)
            }
        }
    }

    private struct ReplyBody: Encodable { let answers: [ReplyAnswer] }
    private struct EmptyBody: Encodable {}
    private struct PermissionReplyBody: Encodable { let reply: String }

    func listQuestions(sessionId: String? = nil) async throws -> [AgentQuestionRequest] {
        let path: String
        if let sessionId {
            path = "questions?sessionId=\(Self.query(sessionId))"
        } else {
            path = "questions"
        }
        let envelope: QuestionsEnvelope = try await client.get(path: path)
        return (envelope.questions ?? []).filter { !$0.questions.isEmpty }
    }

    func replyQuestion(id: String, answers: [ReplyAnswer]) async throws {
        try await client.post(path: "questions/\(Self.escape(id))/reply", body: ReplyBody(answers: answers))
    }

    func rejectQuestion(id: String) async throws {
        try await client.post(path: "questions/\(Self.escape(id))/reject", body: EmptyBody())
    }

    func listPermissions(sessionId: String) async throws -> [AgentPermissionAsk] {
        let envelope: PermissionsEnvelope = try await client.get(path: "permissions?sessionId=\(Self.query(sessionId))")
        return envelope.permissions ?? []
    }

    /// `reply`: "once" | "always" | "reject".
    func replyPermission(id: String, reply: String) async throws {
        try await client.post(path: "permissions/\(Self.escape(id))/reply", body: PermissionReplyBody(reply: reply))
    }

    private static func escape(_ id: String) -> String {
        id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? id
    }

    private static func query(_ value: String) -> String {
        value.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed.subtracting(CharacterSet(charactersIn: "&=+"))) ?? value
    }
}

/// Factory approvals (owned by allternit-api, mounted at `/api/factory`):
/// the same calls the PWA's `factoryClient` makes (lib/factory/client.ts).
///
/// - `GET  /api/factory/approvals?state=pending` → `{ approvals: [Approval] }`
/// - `POST /api/factory/approvals/:id/resolve` `{ decision: approve|reject }`
///
/// Errors come back as `{ error: { code, fact, action } }`; the `fact` and
/// `action` are shown as-is.
final class FactoryApprovalsClient: @unchecked Sendable {
    static let shared = FactoryApprovalsClient()

    private let client: APIClient
    private let baseURL: URL

    init(client: APIClient = .shared, baseURL: URL = AppConfig.factoryBaseURL) {
        self.client = client
        self.baseURL = baseURL
    }

    struct Approval: Decodable, Sendable, Identifiable, Equatable {
        let id: String
        let dagId: String
        let nodeId: String
        let title: String
        let summary: String
        let risk: String
        let riskReason: String?
        let requestedAt: String?
        let state: String
        let expiresAt: String?

        var isHighRisk: Bool { risk == "high" }
    }

    struct FactoryError: LocalizedError {
        let message: String
        var errorDescription: String? { message }
    }

    private struct Envelope: Decodable { let approvals: [Approval]? }
    private struct ErrorEnvelope: Decodable {
        struct Inner: Decodable { let fact: String?; let action: String? }
        let error: Inner?
    }
    private struct ResolveBody: Encodable { let decision: String }

    func listPending() async throws -> [Approval] {
        var components = URLComponents(url: baseURL.appendingPathComponent("approvals"), resolvingAgainstBaseURL: false)
        components?.queryItems = [URLQueryItem(name: "state", value: "pending")]
        guard let url = components?.url else { throw APIError.invalidResponse }
        let request = try await client.authorizedRequest(url: url)
        let data = try await perform(request)
        do {
            return try JSONDecoder().decode(Envelope.self, from: data).approvals ?? []
        } catch {
            throw APIError.decoding(error)
        }
    }

    /// `decision`: "approve" | "reject".
    func resolve(id: String, decision: String) async throws {
        let segment = id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? id
        var request = try await client.authorizedRequest(
            url: baseURL.appendingPathComponent("approvals").appendingPathComponent(segment).appendingPathComponent("resolve"),
            method: "POST"
        )
        request.httpBody = try JSONEncoder().encode(ResolveBody(decision: decision))
        _ = try await perform(request)
    }

    private func perform(_ request: URLRequest) async throws -> Data {
        let (data, response) = try await client.send(request)
        guard let http = response as? HTTPURLResponse else { throw APIError.invalidResponse }
        guard (200...299).contains(http.statusCode) else {
            if let envelope = try? JSONDecoder().decode(ErrorEnvelope.self, from: data),
               let fact = envelope.error?.fact, !fact.isEmpty {
                let action = envelope.error?.action.map { " \($0)" } ?? ""
                throw FactoryError(message: fact + action)
            }
            try client.validate(response, data: data)
            throw APIError.httpError(statusCode: http.statusCode, message: nil)
        }
        return data
    }
}
