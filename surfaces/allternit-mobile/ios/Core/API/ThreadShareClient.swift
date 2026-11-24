import Foundation

// MARK: - Thread shares (cmd/allternit-cloud-api `/api/v2/thread-shares`)
//
// A share link is a read-only SNAPSHOT of a chat: anyone with the link sees
// the user/assistant text (plus tool NAMES) as of the last create/update.
// Requests go direct to allternit-cloud-api with the Clerk session token,
// the same way ArtifactsV2Client does.

struct ThreadShareEntry: Decodable, Equatable, Sendable {
    let token: String
    let url: String
    let updatedAt: String?
}

struct ThreadShareSnapshotItem: Encodable, Equatable, Sendable {
    let role: String // "user" | "assistant"
    let text: String
    var images: [String]? = nil
    var tools: [String]? = nil
}

enum ThreadShareError: LocalizedError, Equatable {
    case signedOut
    case tooLarge
    case empty
    case offline
    case other(String)

    var errorDescription: String? {
        switch self {
        case .signedOut: return "Sign in to Allternit Cloud to share a link."
        case .tooLarge: return "This chat is too large to share as a link."
        case .empty: return "There is nothing to share yet. Send a message first."
        case .offline: return "You appear to be offline. Try again when you are connected."
        case .other(let m): return m
        }
    }
}

final class ThreadShareClient: @unchecked Sendable {
    static let shared = ThreadShareClient()

    private let baseURL: URL
    private let session: URLSession

    init(baseURL: URL = AppConfig.cloudAPIBaseURL, session: URLSession = .shared) {
        self.baseURL = baseURL
        self.session = session
    }

    /// Per-image cap and total cap (server contract).
    static let maxImageBytes = 300 * 1024
    static let maxImages = 10

    // MARK: Snapshot

    /// User/assistant text only, tool NAMES only (no payloads), no reasoning.
    /// Images are not carried: the chat model holds no small data URLs.
    static func buildSnapshot(from messages: [MessageRecord]) -> [ThreadShareSnapshotItem] {
        messages.compactMap { m in
            guard m.role == "user" || m.role == "assistant" else { return nil }
            let text = m.content.trimmingCharacters(in: .whitespacesAndNewlines)
            var tools: [String] = []
            for call in m.toolCalls where !call.toolName.isEmpty && !tools.contains(call.toolName) {
                tools.append(call.toolName)
            }
            if text.isEmpty && tools.isEmpty { return nil }
            return ThreadShareSnapshotItem(role: m.role, text: text, images: nil, tools: tools.isEmpty ? nil : tools)
        }
    }

    // MARK: Calls

    /// 201 new / 200 existing share for the same session.
    func createOrUpdate(sessionId: String, title: String, snapshot: [ThreadShareSnapshotItem]) async throws -> ThreadShareEntry {
        struct Body: Encodable { let sessionId: String; let title: String; let snapshot: [ThreadShareSnapshotItem] }
        let body = try JSONEncoder().encode(Body(sessionId: sessionId, title: title, snapshot: snapshot))
        return try decode(try await send(path: "api/v2/thread-shares", method: "POST", body: body))
    }

    func update(token: String, title: String, snapshot: [ThreadShareSnapshotItem]) async throws -> ThreadShareEntry {
        struct Body: Encodable { let snapshot: [ThreadShareSnapshotItem]; let title: String }
        let body = try JSONEncoder().encode(Body(snapshot: snapshot, title: title))
        let data = try await send(path: "api/v2/thread-shares/\(Self.escape(token))", method: "PUT", body: body)
        // PUT may answer with the entry or just the new updatedAt; fall back
        // to the known token/url when the body is not an entry.
        if let entry = try? JSONDecoder().decode(ThreadShareEntry.self, from: data) { return entry }
        throw ThreadShareError.other("Updated, but the server reply was unreadable.")
    }

    /// nil when this session has no share (404).
    func lookup(sessionId: String) async throws -> ThreadShareEntry? {
        do {
            let data = try await send(path: "api/v2/thread-shares", query: [URLQueryItem(name: "sessionId", value: sessionId)])
            return try decode(data)
        } catch let e as ThreadShareError {
            if case .other(let m) = e, m == "__404" { return nil }
            throw e
        }
    }

    func delete(token: String) async throws {
        _ = try await send(path: "api/v2/thread-shares/\(Self.escape(token))", method: "DELETE")
    }

    // MARK: Transport

    private static func escape(_ id: String) -> String {
        id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed.subtracting(CharacterSet(charactersIn: "/"))) ?? id
    }

    private func decode(_ data: Data) throws -> ThreadShareEntry {
        do { return try JSONDecoder().decode(ThreadShareEntry.self, from: data) }
        catch { throw ThreadShareError.other("The server reply was unreadable.") }
    }

    private func send(path: String, query: [URLQueryItem] = [], method: String = "GET", body: Data? = nil) async throws -> Data {
        guard var components = URLComponents(url: baseURL.appendingPathComponent(path), resolvingAgainstBaseURL: false) else {
            throw ThreadShareError.other("Bad request.")
        }
        if !query.isEmpty { components.queryItems = query }
        guard let url = components.url else { throw ThreadShareError.other("Bad request.") }
        guard let token = await ArtifactsV2Client.bearerToken() else { throw ThreadShareError.signedOut }
        var request = URLRequest(url: url)
        request.httpMethod = method
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        if let body {
            request.httpBody = body
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }
        let data: Data
        let response: URLResponse
        do {
            (data, response) = try await session.data(for: request)
        } catch let e as URLError where [.notConnectedToInternet, .networkConnectionLost, .timedOut, .cannotConnectToHost].contains(e.code) {
            throw ThreadShareError.offline
        } catch {
            throw ThreadShareError.other("Could not reach Allternit Cloud.")
        }
        guard let http = response as? HTTPURLResponse else { throw ThreadShareError.other("Invalid response.") }
        switch http.statusCode {
        case 200...299: return data
        case 401: throw ThreadShareError.signedOut
        case 404 where method == "GET": throw ThreadShareError.other("__404")
        case 413: throw ThreadShareError.tooLarge
        case 422: throw ThreadShareError.empty
        default: throw ThreadShareError.other("Something went wrong (\(http.statusCode)). Try again.")
        }
    }
}
