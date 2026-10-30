import Foundation

/// Client for the computer's terminal service (web lib/terminal-api.ts): the
/// tmux-backed shells Allternit Desktop and the runtime serve at
/// `/terminal/*`. Through the cloud relay this is what m.allternit.com uses;
/// gizzi-code's `/v1/pty` routes are only on a directly reached gizzi-code
/// server (`PtyClient`), and a relayed computer answers them 405.
final class TerminalServiceClient: @unchecked Sendable {
    /// A relay placeholder (`allternit-relay://<runtimeId>`) or a gateway URL.
    let baseURL: URL

    init(baseURL: URL) {
        self.baseURL = baseURL
    }

    private var api: APIClient {
        APIClient(baseURL: baseURL, tokenProvider: { try await AuthManager.shared.getToken() })
    }

    /// zsh is only the default: where the computer has none, an empty shell
    /// runs its own default (createTerminalSession's MISSING_SHELL retry).
    func create(cols: Int = 80, rows: Int = 24, cwd: String? = nil) async throws -> String {
        do {
            return try await create(shell: "/bin/zsh", cols: cols, rows: rows, cwd: cwd)
        } catch let error as APIError {
            let text = error.errorDescription ?? ""
            guard text.range(of: "ENOENT|doesn't exist|No such file", options: .regularExpression) != nil else { throw error }
            return try await create(shell: "", cols: cols, rows: rows, cwd: cwd)
        }
    }

    private func create(shell: String, cols: Int, rows: Int, cwd: String?) async throws -> String {
        let reply: CreateReply = try await api.post(
            path: "terminal/create",
            body: CreateBody(shell: shell, cols: cols, rows: rows, cwd: cwd)
        )
        if reply.success == false {
            throw APIError.httpError(statusCode: 500, message: reply.message ?? reply.details ?? "Failed to create terminal")
        }
        guard let id = reply.sessionId ?? reply.session_id ?? reply.data?.session_id, !id.isEmpty else {
            throw APIError.httpError(statusCode: 500, message: "Terminal service did not return a session id")
        }
        return id
    }

    func input(sessionId: String, _ content: String) async throws {
        try await api.post(path: "terminal/\(Self.escape(sessionId))/input",
                           body: InputBody(session_id: sessionId, content: content))
    }

    func resize(sessionId: String, cols: Int, rows: Int) async throws {
        try await api.post(path: "terminal/\(Self.escape(sessionId))/resize",
                           body: ResizeBody(session_id: sessionId, cols: cols, rows: rows))
    }

    /// Ends the shell on the computer.
    func close(sessionId: String) async throws {
        try await api.post(path: "terminal/\(Self.escape(sessionId))/close",
                           body: CloseBody(session_id: sessionId))
    }

    /// `GET /terminal/:id/stream` (SSE): each event's data is
    /// `{ type: "data", data }`, or raw text.
    func stream(sessionId: String) async throws -> URLSession.AsyncBytes {
        let client = api
        var request = try await client.authorizedRequest(path: "terminal/\(Self.escape(sessionId))/stream")
        request.setValue("text/event-stream", forHTTPHeaderField: "Accept")
        request.timeoutInterval = 60 * 60 * 24
        let (bytes, response) = try await client.sendStream(request)
        try client.validate(response)
        return bytes
    }

    private static func escape(_ id: String) -> String {
        id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? id
    }

    private struct CreateBody: Encodable {
        let shell: String
        let cols: Int
        let rows: Int
        let cwd: String?
    }

    private struct CreateReply: Decodable {
        struct Inner: Decodable { let session_id: String? }
        let success: Bool?
        let message: String?
        let details: String?
        let sessionId: String?
        let session_id: String?
        let data: Inner?
    }

    private struct InputBody: Encodable {
        let session_id: String
        let content: String
    }

    private struct ResizeBody: Encodable {
        let session_id: String
        let cols: Int
        let rows: Int
    }

    private struct CloseBody: Encodable {
        let session_id: String
    }
}
