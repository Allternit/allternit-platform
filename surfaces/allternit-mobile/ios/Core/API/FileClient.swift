import Foundation

/// One entry in a directory listing (`GET v1/file/tree`,
/// `cmd/gizzi-code/src/runtime/server/routes/file.ts`). `path` is relative
/// to the instance's project root; `absolute` is the full on-disk path.
/// `ignored` marks git-ignored entries (still listed, dimmed in the UI).
struct FileNode: Decodable, Identifiable, Hashable, Sendable {
    let name: String
    let path: String
    let absolute: String
    let type: NodeType
    let ignored: Bool

    var id: String { path }

    enum NodeType: String, Decodable, Sendable {
        case file, directory
    }
}

/// A file's contents (`GET v1/file/read`). Binary files carry `encoding ==
/// "base64"` with the raw bytes in `content`; text files are plain UTF-8.
/// `diff` is populated for free when the file has uncommitted git changes.
struct FileReadResult: Decodable, Sendable {
    let type: ContentType
    let content: String
    let diff: String?
    let encoding: String?
    let mimeType: String?

    enum ContentType: String, Decodable, Sendable {
        case text, binary
    }
}

/// Client for gizzi-code's file routes (`routes/file.ts`, mounted at
/// `v1/file`) — directory listing and file reads for the Code tab's file
/// browser. Like `PtyClient`/`CronClient` this connects to whatever
/// `baseURL` the caller resolved (see `InstanceConnection`); it does no
/// instance/mesh resolution itself.
final class FileClient: @unchecked Sendable {
    private let endpointBaseURL: URL

    init(baseURL: URL = AppConfig.gizziCodeBaseURL) {
        self.endpointBaseURL = baseURL
    }

    /// Per-call target: the given base URL, or — for the static default —
    /// whatever `InstanceConnection.resolve()` picks now (explicit URL →
    /// mesh → the relay to the user's computer).
    private func client() async throws -> APIClient {
        try await InstanceConnection.gizziClient(for: endpointBaseURL)
    }

    /// `GET v1/file/tree?path=` — one directory level, not recursive. `path`
    /// nil (or empty) lists the project root.
    func tree(path: String? = nil) async throws -> [FileNode] {
        var route = "v1/file/tree"
        if let path, !path.isEmpty {
            route += "?path=\(Self.escapeQuery(path))"
        }
        return try await client().get(path: route)
    }

    /// `GET v1/file/read?path=`.
    func read(path: String) async throws -> FileReadResult {
        try await client().get(path: "v1/file/read?path=\(Self.escapeQuery(path))")
    }

    private static func escapeQuery(_ value: String) -> String {
        value.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? value
    }
}
