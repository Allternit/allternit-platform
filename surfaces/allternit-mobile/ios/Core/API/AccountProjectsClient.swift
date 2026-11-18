import Foundation

/// A project saved to the account in cloud-api (`account_projects`), so a
/// person with no computer can still make and see projects and bots. When a
/// computer connects, its allternit-api (account_sync) adopts the row under
/// the same id; from then on the computer's copy wins (clients merge by id).
struct AccountProject: Decodable, Identifiable, Hashable, Sendable {
    enum Kind: String, CaseIterable, Sendable {
        case chat, code, design, bot
    }

    let id: String
    let kind: String
    let title: String
    let description: String?
    let instructions: String?
    /// A JSON object (`metadata.brief` holds a bot's brief; `cover` a folder cover).
    let metadata: [String: JSONValue]?
    let adoptedComputerId: String?
    let createdAt: String?
    let updatedAt: String?
    let deletedAt: String?

    init(id: String, kind: String, title: String, description: String? = nil, instructions: String? = nil,
         metadata: [String: JSONValue]? = nil, adoptedComputerId: String? = nil,
         createdAt: String? = nil, updatedAt: String? = nil, deletedAt: String? = nil) {
        self.id = id
        self.kind = kind
        self.title = title
        self.description = description
        self.instructions = instructions
        self.metadata = metadata
        self.adoptedComputerId = adoptedComputerId
        self.createdAt = createdAt
        self.updatedAt = updatedAt
        self.deletedAt = deletedAt
    }

    enum CodingKeys: String, CodingKey {
        case id, kind, title, description, instructions, metadata
        case adoptedComputerId = "adopted_computer_id"
        case createdAt = "created_at"
        case updatedAt = "updated_at"
        case deletedAt = "deleted_at"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        kind = try c.decodeIfPresent(String.self, forKey: .kind) ?? "chat"
        title = try c.decodeIfPresent(String.self, forKey: .title) ?? ""
        description = try c.decodeIfPresent(String.self, forKey: .description)
        instructions = try c.decodeIfPresent(String.self, forKey: .instructions)
        // An object, or the same object sent as a JSON string.
        switch try? c.decodeIfPresent(JSONValue.self, forKey: .metadata) {
        case .object(let object)?:
            metadata = object
        case .string(let raw)?:
            metadata = (try? JSONDecoder().decode([String: JSONValue].self, from: Data(raw.utf8)))
        default:
            metadata = nil
        }
        adoptedComputerId = try c.decodeIfPresent(String.self, forKey: .adoptedComputerId)
        createdAt = try c.decodeIfPresent(String.self, forKey: .createdAt)
        updatedAt = try c.decodeIfPresent(String.self, forKey: .updatedAt)
        deletedAt = try c.decodeIfPresent(String.self, forKey: .deletedAt)
    }

    func hash(into hasher: inout Hasher) {
        hasher.combine(id)
        hasher.combine(updatedAt)
    }

    var mode: LibraryProject.Mode { LibraryProject.Mode(rawValue: kind) ?? .chat }
    var isDeleted: Bool { deletedAt != nil }

    /// A bot's brief: its `instructions`. A string `metadata.brief` is read
    /// only for rows written before that (never written by this app).
    var brief: String? {
        if let instructions, !instructions.isEmpty { return instructions }
        if case .string(let value)? = metadata?["brief"], !value.isEmpty { return value }
        return nil
    }
}

/// cloud-api `/api/v1/account-projects` (+ `/:id`): the
/// signed-in session only, never a computer. A 404 on the list (route not
/// deployed yet) is an empty list, never an error.
struct AccountProjectsClient: Sendable {
    var baseURL: URL = AppConfig.cloudAPIBaseURL
    var session: URLSession = .shared
    var tokenProvider: @Sendable () async throws -> String? = { try await AuthManager.shared.getToken() }

    static let shared = AccountProjectsClient()
    private static let route = "api/v1/account-projects"

    func list(kind: AccountProject.Kind? = nil) async throws -> [AccountProject] {
        struct Envelope: Decodable { let projects: [AccountProject] }
        var path = Self.route
        if let kind { path += "?kind=\(kind.rawValue)" }
        do {
            let envelope: Envelope = try await send("GET", path, Optional<String>.none)
            return envelope.projects.filter { !$0.isDeleted }
        } catch let error as APIError where error.httpStatus == 404 {
            return []
        }
    }

    struct CreateBody: Encodable {
        let id: String?
        let kind: String
        let title: String
        let description: String?
        let instructions: String?
        let metadata: [String: JSONValue]?
    }

    func create(id: String? = nil, kind: AccountProject.Kind, title: String, description: String? = nil,
                instructions: String? = nil, metadata: [String: JSONValue]? = nil) async throws -> AccountProject {
        let body = CreateBody(id: id, kind: kind.rawValue, title: title, description: description,
                              instructions: instructions, metadata: metadata)
        return try await sendProject("POST", Self.route, body)
    }

    func get(id: String) async throws -> AccountProject {
        try await sendProject("GET", "\(Self.route)/\(Self.escape(id))", Optional<String>.none)
    }

    struct UpdateBody: Encodable {
        let title: String?
        let description: String?
        let instructions: String?
        let metadata: [String: JSONValue]?
    }

    /// PATCH; the answer's body is not needed (callers reload the list).
    func update(id: String, title: String? = nil, description: String? = nil, instructions: String? = nil,
                metadata: [String: JSONValue]? = nil) async throws {
        let body = UpdateBody(title: title, description: description, instructions: instructions, metadata: metadata)
        _ = try await raw("PATCH", "\(Self.route)/\(Self.escape(id))", body)
    }

    /// 404 means it is already gone, which is the goal.
    func delete(id: String) async throws {
        do {
            _ = try await raw("DELETE", "\(Self.route)/\(Self.escape(id))", Optional<String>.none)
        } catch let error as APIError where error.httpStatus == 404 {
            return
        }
    }


    // MARK: Transport

    /// The project, bare or as `{ "project": {...} }`.
    private func sendProject<B: Encodable>(_ method: String, _ path: String, _ body: B?) async throws -> AccountProject {
        let data = try await raw(method, path, body)
        if let wrapped = try? JSONDecoder().decode(AccountProjectEnvelope.self, from: data) { return wrapped.project }
        do { return try JSONDecoder().decode(AccountProject.self, from: data) } catch { throw APIError.decoding(error) }
    }

    private func send<T: Decodable, B: Encodable>(_ method: String, _ path: String, _ body: B?) async throws -> T {
        let data = try await raw(method, path, body)
        do { return try JSONDecoder().decode(T.self, from: data) } catch { throw APIError.decoding(error) }
    }

    private func raw<B: Encodable>(_ method: String, _ path: String, _ body: B?) async throws -> Data {
        guard let token = try await tokenProvider(), !token.isEmpty else { throw APIError.invalidResponse }
        let parts = path.split(separator: "?", maxSplits: 1).map(String.init)
        var components = URLComponents(url: baseURL.appendingPathComponent(parts[0]), resolvingAgainstBaseURL: false)
        if parts.count > 1 { components?.percentEncodedQuery = parts[1] }
        guard let url = components?.url else { throw APIError.invalidResponse }
        var request = URLRequest(url: url)
        request.httpMethod = method
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        request.timeoutInterval = 20
        if let body {
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = try JSONEncoder().encode(body)
        }
        let data: Data
        let response: URLResponse
        do {
            (data, response) = try await session.data(for: request)
        } catch is CancellationError {
            throw CancellationError()
        } catch {
            throw APIError.transport(error)
        }
        guard let http = response as? HTTPURLResponse else { throw APIError.invalidResponse }
        guard (200...299).contains(http.statusCode) else {
            throw APIError.httpError(statusCode: http.statusCode, message: APIErrorBody.parse(data).message)
        }
        return data
    }

    private static func escape(_ id: String) -> String {
        id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? id
    }
}

/// `{ "project": … }`: Swift does not allow a type nested in a generic function.
private struct AccountProjectEnvelope: Decodable { let project: AccountProject }
