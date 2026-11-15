import Foundation

/// One file's accumulated change within a gizzi-code session
/// (`GET v1/session/:id/diff`, `Snapshot.FileDiff`) — full before/after
/// content (not a unified-diff string), scoped to everything the agent
/// changed from the session's first tool-use step to its last.
struct FileDiff: Decodable, Identifiable, Sendable {
    let file: String
    let before: String
    let after: String
    let additions: Int
    let deletions: Int
    let status: String?

    var id: String { file }
}

/// Client for gizzi-code's session-diff route (`routes/session.ts`,
/// `GET v1/session/:id/diff`). Like `FileClient`/`PtyClient` this connects
/// to whatever `baseURL` the caller resolved (see `InstanceConnection`).
final class SessionDiffClient: @unchecked Sendable {
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

    /// `GET v1/session/:id/diff`. The result is storage-backed and only as
    /// fresh as the session's last `summarize` call — an empty array can
    /// mean either "no changes" or "not summarized yet"; callers should
    /// offer a manual refresh rather than treating `[]` as a hard error.
    func diff(sessionID: String) async throws -> [FileDiff] {
        try await client().get(path: "v1/session/\(Self.escape(sessionID))/diff")
    }

    private static func escape(_ id: String) -> String {
        id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? id
    }
}
