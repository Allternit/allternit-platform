import Foundation

/// What an agent does when it wants to use a tool. Ask is the default and is
/// never stored: the server deletes the row when it receives `ask`.
enum ToolPolicy: String, Codable, CaseIterable, Sendable {
    case ask, allow, block

    var title: String {
        switch self {
        case .ask: return "Ask"
        case .allow: return "Allow"
        case .block: return "Block"
        }
    }
}

/// One row of `GET /api/v1/tool-policies`. `ownerId` is `connectors:<id>` or `mcps:<id>`.
struct ToolPolicyRow: Codable, Sendable, Equatable {
    let ownerId: String
    let toolId: String
    let policy: ToolPolicy
    var updatedAt: String? = nil

    enum CodingKeys: String, CodingKey {
        case ownerId = "owner_id", toolId = "tool_id", policy, updatedAt = "updated_at"
    }
}

private struct ToolPoliciesEnvelope: Decodable {
    let policies: [ToolPolicyRow]
}

private struct ToolPoliciesBody: Encodable {
    let policies: [ToolPolicyRow]
}

/// Client for the account's tool policies (`GET` / `PUT /api/v1/tool-policies`).
/// An old server answers 404; callers show "can't save yet" and disable the controls.
final class ToolPoliciesClient: @unchecked Sendable {
    private let client: APIClient

    init(client: APIClient = .shared) {
        self.client = client
    }

    func list() async throws -> [ToolPolicyRow] {
        let response: ToolPoliciesEnvelope = try await client.get(path: "tool-policies")
        return response.policies
    }

    /// Saves the given rows (`ask` deletes) and returns the full list. The PUT
    /// answers with the list too; the shared `put` helper drops it, so this
    /// reads it back with one GET.
    func save(_ rows: [ToolPolicyRow]) async throws -> [ToolPolicyRow] {
        try await client.put(path: "tool-policies", body: ToolPoliciesBody(policies: rows))
        return try await list()
    }

    static func isNotFound(_ error: Error) -> Bool {
        if case APIError.httpError(let status, _) = error { return status == 404 }
        return false
    }
}
