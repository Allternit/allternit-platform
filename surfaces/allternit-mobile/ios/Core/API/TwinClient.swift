import Foundation

/// The owner's digital twin: one voice and one shared memory for every bot
/// (allternit-ai src/lib/gateway/twin-api.ts; allternit-api
/// cmd/allternit-api/src/twin_persona.rs, camelCase on the wire):
///
/// - `GET|PUT /api/v1/twin/profile` → `{ profile }`
/// - `GET|POST /api/v1/twin/memory` → `{ memory: [..] }` / `{ memory }`
/// - `PATCH|DELETE /api/v1/twin/memory/:id`, `POST /api/v1/twin/memory/:id/accept`
///
/// The twin lives on the person's computer; `APIClient.shared` reaches it
/// through cloud-api's data-plane fallback.
struct TwinProfile: Codable, Equatable, Sendable {
    var displayName: String
    var speakingStyle: String
    var signature: String
    var ownerDisclosure: String
    var updatedAt: String?
}

enum TwinKind: String, Codable, CaseIterable, Sendable {
    case fact, preference, schedule_rule, person, decision

    var label: String {
        switch self {
        case .fact: return "Fact"
        case .preference: return "Preference"
        case .schedule_rule: return "Schedule rule"
        case .person: return "Person"
        case .decision: return "Decision"
        }
    }
}

/// all = every bot and their vendor bots; bot = one bot; owner = only you.
enum TwinVisibility: String, Codable, Sendable {
    case all, bot, owner

    var label: String {
        switch self {
        case .all: return "All bots"
        case .bot: return "One bot"
        case .owner: return "Only me"
        }
    }
}

struct TwinProvenance: Codable, Equatable, Sendable {
    let source: String
    let botId: String?
    let channel: String?
    let threadId: String?
    let confidence: Double?
    let learnedAt: String?
}

struct TwinMemory: Codable, Identifiable, Equatable, Sendable {
    let id: String
    let kind: TwinKind
    let subject: String
    let content: String
    let visibility: TwinVisibility
    let botId: String?
    /// `active` (bots read it) or `proposed` (a bot learned it; you review it).
    let status: String
    let provenance: TwinProvenance
    let createdAt: String?
    let updatedAt: String?
    let reviewedAt: String?

    var isProposed: Bool { status == "proposed" }
}

struct TwinMemoryInput: Encodable, Sendable {
    let kind: TwinKind
    let subject: String
    let content: String
    let visibility: TwinVisibility
    let botId: String?

    enum CodingKeys: String, CodingKey { case kind, subject, content, visibility, botId }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(kind, forKey: .kind)
        try c.encode(subject, forKey: .subject)
        try c.encode(content, forKey: .content)
        try c.encode(visibility, forKey: .visibility)
        if visibility == .bot, let botId { try c.encode(botId, forKey: .botId) }
    }
}

final class TwinClient: @unchecked Sendable {
    static let shared = TwinClient()
    private let client: APIClient

    init(client: APIClient = .shared) { self.client = client }

    private struct ProfileBody: Decodable { let profile: TwinProfile }
    private struct MemoryList: Decodable { let memory: [TwinMemory]? }
    private struct MemoryBody: Decodable { let memory: TwinMemory }
    private struct ProfileInput: Encodable {
        let displayName: String
        let speakingStyle: String
        let signature: String
        let ownerDisclosure: String
    }

    /// nil when this computer has no twin routes yet (404).
    func profile() async throws -> TwinProfile? {
        do {
            let body: ProfileBody = try await client.get(path: "twin/profile")
            return body.profile
        } catch let APIError.httpError(status, _) where status == 404 {
            return nil
        }
    }

    func saveProfile(_ profile: TwinProfile) async throws -> TwinProfile {
        let input = ProfileInput(displayName: profile.displayName, speakingStyle: profile.speakingStyle,
                                 signature: profile.signature, ownerDisclosure: profile.ownerDisclosure)
        let body: ProfileBody = try await send("twin/profile", method: "PUT", body: input)
        return body.profile
    }

    func memory() async throws -> [TwinMemory] {
        let body: MemoryList = try await client.get(path: "twin/memory")
        return body.memory ?? []
    }

    func add(_ input: TwinMemoryInput) async throws -> TwinMemory {
        let body: MemoryBody = try await client.post(path: "twin/memory", body: input)
        return body.memory
    }

    func update(id: String, _ input: TwinMemoryInput) async throws -> TwinMemory {
        let body: MemoryBody = try await send("twin/memory/\(settingsPathSegment(id))", method: "PATCH", body: input)
        return body.memory
    }

    func accept(id: String) async throws -> TwinMemory {
        let request = try await client.authorizedRequest(path: "twin/memory/\(settingsPathSegment(id))/accept", method: "POST")
        let (data, response) = try await client.send(request)
        try client.validate(response, data: data)
        do { return try JSONDecoder().decode(MemoryBody.self, from: data).memory } catch { throw APIError.decoding(error) }
    }

    func remove(id: String) async throws {
        try await client.delete(path: "twin/memory/\(settingsPathSegment(id))")
    }

    private func send<T: Decodable, B: Encodable>(_ path: String, method: String, body: B) async throws -> T {
        var request = try await client.authorizedRequest(path: path, method: method)
        request.httpBody = try JSONEncoder().encode(body)
        let (data, response) = try await client.send(request)
        try client.validate(response, data: data)
        do { return try JSONDecoder().decode(T.self, from: data) } catch { throw APIError.decoding(error) }
    }
}
