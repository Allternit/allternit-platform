import Foundation

/// Facts Allternit learned from conversations (allternit-api memory v2):
///
/// - `GET /api/v1/memory/v2/facts?agent_id=&limit=` → `{ facts, count }`
/// - `PATCH /api/v1/memory/v2/facts/:id` `{ fact?, memory_type? }` → `{ id, previous_id, memory_type }`
///   (400 when the text looks like a secret, 404 when the fact is gone)
/// - `DELETE /api/v1/memory/v2/facts/:id`
struct MemoryFact: Codable, Identifiable, Equatable, Sendable {
    let id: String
    let userId: String?
    let agentId: String?
    let fact: String
    let confidence: Double?
    let validFrom: String?
    let validUntil: String?
    let sourceObservationId: String?
    let memoryType: String?

    enum CodingKeys: String, CodingKey {
        case id, fact, confidence
        case userId = "user_id"
        case agentId = "agent_id"
        case validFrom = "valid_from"
        case validUntil = "valid_until"
        case sourceObservationId = "source_observation_id"
        case memoryType = "memory_type"
    }
}

struct MemoryFactEditResult: Decodable, Sendable {
    let id: String
    let previousId: String?
    let memoryType: String?

    enum CodingKeys: String, CodingKey {
        case id
        case previousId = "previous_id"
        case memoryType = "memory_type"
    }
}

final class MemoryFactsClient: @unchecked Sendable {
    static let shared = MemoryFactsClient()
    private let client: APIClient

    init(client: APIClient = .shared) { self.client = client }

    private struct FactList: Decodable { let facts: [MemoryFact]? }
    private struct EditBody: Encodable {
        let fact: String
        enum CodingKeys: String, CodingKey { case fact }
    }

    func list(agentId: String? = nil, limit: Int = 200) async throws -> [MemoryFact] {
        var path = "memory/v2/facts?limit=\(limit)"
        if let agentId, !agentId.isEmpty {
            let encoded = agentId.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? agentId
            path += "&agent_id=\(encoded)"
        }
        let body: FactList = try await client.get(path: path)
        return body.facts ?? []
    }

    /// Editing supersedes the fact: the result carries the new id.
    func edit(id: String, fact: String) async throws -> MemoryFactEditResult {
        var request = try await client.authorizedRequest(path: "memory/v2/facts/\(settingsPathSegment(id))", method: "PATCH")
        request.httpBody = try JSONEncoder().encode(EditBody(fact: fact))
        let (data, response) = try await client.send(request)
        try client.validate(response, data: data)
        do { return try JSONDecoder().decode(MemoryFactEditResult.self, from: data) } catch { throw APIError.decoding(error) }
    }

    func remove(id: String) async throws {
        try await client.delete(path: "memory/v2/facts/\(settingsPathSegment(id))")
    }
}
