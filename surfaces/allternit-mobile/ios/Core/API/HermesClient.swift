import Foundation
import CryptoKit

// Hermes parity routes (SPEC-hermes-parity-2026-10-31.md): the bot summary
// that feeds the bot page tiles, the memory map, and the workspace-file
// read/write that carries a content hash so two editors cannot overwrite
// each other.

// MARK: - Bot summary

/// `GET /api/v1/bot-hub/:id/summary`. A value with no real source is null
/// on the wire and nil here; screens hide it.
struct BotSummary: Decodable, Sendable, Equatable {
    struct Soul: Decodable, Sendable, Equatable {
        let updatedAt: String?
        let bytes: Int?
        enum CodingKeys: String, CodingKey { case updatedAt = "updated_at", bytes }
    }
    struct Memory: Decodable, Sendable, Equatable { let count: Int? }
    struct Skills: Decodable, Sendable, Equatable { let on: Int?; let total: Int? }
    struct Usage: Decodable, Sendable, Equatable {
        let sessions30d: Int?
        let apiCalls30d: Int?
        let spendUsd30d: Double?
        enum CodingKeys: String, CodingKey {
            case sessions30d = "sessions_30d", apiCalls30d = "api_calls_30d", spendUsd30d = "spend_usd_30d"
        }
    }

    let model: String?
    let soul: Soul?
    let memory: Memory?
    let skills: Skills?
    let usage: Usage?
}

// MARK: - Memory map

struct MemoryMapItem: Decodable, Sendable, Hashable, Identifiable {
    let id: String
    let kind: String
    let label: String
    let uses: Int?
    let lastAt: String?
    let ref: String?
    enum CodingKeys: String, CodingKey { case id, kind, label, uses, lastAt = "last_at", ref }
}

struct MemoryMapLink: Decodable, Sendable, Hashable {
    /// `from` depends on `to`.
    let from: String
    let to: String
    let kind: String
}

/// `GET /api/v1/memory-map?bot=<id>`; omit `bot` for the whole account.
struct MemoryMap: Decodable, Sendable {
    let items: [MemoryMapItem]
    let links: [MemoryMapLink]
    let truncated: Bool

    init(items: [MemoryMapItem], links: [MemoryMapLink], truncated: Bool) {
        self.items = items; self.links = links; self.truncated = truncated
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        items = (try? c.decode([MemoryMapItem].self, forKey: .items)) ?? []
        links = (try? c.decode([MemoryMapLink].self, forKey: .links)) ?? []
        truncated = (try? c.decode(Bool.self, forKey: .truncated)) ?? false
    }
    enum CodingKeys: String, CodingKey { case items, links, truncated }
}

// MARK: - Workspace file with a revision hash

struct HashedWorkspaceFile: Sendable {
    let path: String
    let content: String
    /// sha256 of the content; nil from an older API that does not send it.
    let hash: String?
}

enum WorkspaceSaveResult: Sendable {
    case saved(hash: String)
    /// Someone else changed the file; `content`/`hash` are the current ones.
    case conflict(hash: String?, content: String)
}

final class HermesClient: @unchecked Sendable {
    static let shared = HermesClient()
    private let client: APIClient
    init(client: APIClient = .shared) { self.client = client }

    func summary(botId: String) async throws -> BotSummary {
        try await client.get(path: "bot-hub/\(Self.escape(botId))/summary")
    }

    func memoryMap(botId: String?) async throws -> MemoryMap {
        if let botId {
            return try await client.get(path: "memory-map?bot=\(Self.escape(botId))")
        }
        return try await client.get(path: "memory-map")
    }

    // MARK: Workspace files

    private struct FileBody: Decodable {
        let path: String?
        let content: String?
        let hash: String?
    }

    private struct PutBody: Encodable {
        let path: String
        let content: String
        let expectedHash: String?
        enum CodingKeys: String, CodingKey { case path, content, expectedHash = "expected_hash" }
        func encode(to encoder: Encoder) throws {
            var c = encoder.container(keyedBy: CodingKeys.self)
            try c.encode(path, forKey: .path)
            try c.encode(content, forKey: .content)
            // Omitted = the old "overwrite" behaviour.
            try c.encodeIfPresent(expectedHash, forKey: .expectedHash)
        }
    }

    func readFile(agentId: String, path: String) async throws -> HashedWorkspaceFile {
        let q = path.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? path
        let body: FileBody = try await client.get(path: "agents/\(Self.escape(agentId))/workspace/file?path=\(q)")
        let content = body.content ?? ""
        return HashedWorkspaceFile(path: body.path ?? path, content: content, hash: body.hash)
    }

    /// PUT with `expected_hash`. A 409 returns `.conflict` with the current
    /// content instead of throwing, so the editor can offer reload / keep mine.
    func writeFile(agentId: String, path: String, content: String, expectedHash: String?) async throws -> WorkspaceSaveResult {
        var request = try await client.authorizedRequest(path: "agents/\(Self.escape(agentId))/workspace/file", method: "PUT")
        request.httpBody = try JSONEncoder().encode(PutBody(path: path, content: content, expectedHash: expectedHash))
        let (data, response) = try await client.send(request)
        if let http = response as? HTTPURLResponse, http.statusCode == 409 {
            let body = try? JSONDecoder().decode(FileBody.self, from: data)
            return .conflict(hash: body?.hash, content: body?.content ?? "")
        }
        try client.validate(response, data: data)
        let echoed = (try? JSONDecoder().decode(FileBody.self, from: data))?.hash
        return .saved(hash: echoed ?? Self.sha256Hex(content))
    }

    static func sha256Hex(_ text: String) -> String {
        SHA256.hash(data: Data(text.utf8)).map { String(format: "%02x", $0) }.joined()
    }

    private static func escape(_ s: String) -> String {
        s.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? s
    }
}
