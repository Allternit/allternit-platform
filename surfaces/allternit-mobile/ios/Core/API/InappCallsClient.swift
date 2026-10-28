import Foundation

/// Free Allternit calls between linked people (the bot's owner and someone
/// who joined through the bot's invite) — cloud-api
/// `cmd/allternit-cloud-api/src/routes/inapp_calls.rs`, the same routes the
/// web's `src/lib/gateway/inapp-calls.ts` uses:
/// - `GET  /api/v1/phone/incoming` → `{calls:[…]}` (also the presence heartbeat)
/// - `POST /api/v1/phone/contacts/:userId/ring/:room/answer`  → `{callId, threadId, room, token, url}`
/// - `POST /api/v1/phone/contacts/:userId/ring/:room/decline` → `{ok}`
/// - `POST /api/v1/phone/contacts/:userId/ring/:room/cancel`  → `{ok}`
/// - `GET  /api/v1/phone/threads/:id/events?after=` → `{events:[…]}`
///
/// Goes DIRECT to `AppConfig.cloudAPIBaseURL` with the Bearer (like
/// PushTokenClient): the call state lives in the cloud, so a sleeping or
/// offline computer never blocks a ring.
final class InappCallsClient: Sendable {
    static let shared = InappCallsClient()

    private init() {}

    struct IncomingCall: Decodable, Equatable, Sendable, Identifiable {
        struct From: Decodable, Equatable, Sendable {
            let userId: String
            let label: String
        }
        let callId: String
        let room: String
        let threadId: String
        let botId: String
        let from: From
        let expiresAt: String?

        var id: String { room }
    }

    /// A LiveKit join grant for the room.
    struct CallGrant: Decodable, Sendable {
        let callId: String?
        let room: String
        let token: String
        let url: String
        let threadId: String?
    }

    struct ThreadEvent: Decodable, Sendable {
        let id: Int
        let threadId: String
        let kind: String
        let room: String?
    }

    /// A failed call request: HTTP status (0 = couldn't reach the cloud)
    /// plus the server's `error` code (`call_over`, `livekit_not_configured`…).
    struct CallError: Error, LocalizedError, Sendable {
        let status: Int
        let code: String?

        /// Plain words, the same as the web's `inappCallErrorText`.
        var errorDescription: String? {
            if status == 0 { return "Couldn't reach Allternit. Check your connection and try again." }
            switch code {
            case "callee_offline": return "We'll let them know you called."
            case "callee_busy": return "They're already getting another call. Try again in a minute."
            case "too_many_rings": return "You've called a few times already. Give them a little while."
            case "not_a_contact": return "You can only call people who joined through an invite."
            case "call_over", "call_not_found": return "That call has ended."
            case "livekit_not_configured", "livekit_public_url_not_configured": return "Calling isn't available right now."
            default: return "Couldn't complete that (\(status))."
            }
        }

        /// Signed out, or a server without these routes: poll slowly instead.
        var isBackoff: Bool { [401, 403, 404, 405, 501, 503].contains(status) }
    }

    func incoming() async throws -> [IncomingCall] {
        struct Body: Decodable { let calls: [IncomingCall]? }
        let body: Body = try await send("GET", "api/v1/phone/incoming")
        return body.calls ?? []
    }

    func answer(userId: String, room: String) async throws -> CallGrant {
        try await send("POST", "api/v1/phone/contacts/\(Self.enc(userId))/ring/\(Self.enc(room))/answer")
    }

    func decline(userId: String, room: String) async throws {
        let _: OK = try await send("POST", "api/v1/phone/contacts/\(Self.enc(userId))/ring/\(Self.enc(room))/decline")
    }

    func cancel(userId: String, room: String) async throws {
        let _: OK = try await send("POST", "api/v1/phone/contacts/\(Self.enc(userId))/ring/\(Self.enc(room))/cancel")
    }

    func events(threadId: String, after: Int = 0) async throws -> [ThreadEvent] {
        struct Body: Decodable { let events: [ThreadEvent]? }
        let body: Body = try await send("GET", "api/v1/phone/threads/\(Self.enc(threadId))/events?after=\(after)")
        return body.events ?? []
    }

    // MARK: - Transport

    private struct OK: Decodable {}
    private struct ErrorBody: Decodable { let error: String? }

    private static func enc(_ value: String) -> String {
        var allowed = CharacterSet.urlPathAllowed
        allowed.remove(charactersIn: "/?#")
        return value.addingPercentEncoding(withAllowedCharacters: allowed) ?? value
    }

    private func send<T: Decodable>(_ method: String, _ path: String) async throws -> T {
        let client = APIClient.shared
        var components = URLComponents(
            url: AppConfig.cloudAPIBaseURL.appendingPathComponent(String(path.split(separator: "?", maxSplits: 1)[0])),
            resolvingAgainstBaseURL: false
        )
        if let split = path.firstIndex(of: "?") {
            components?.percentEncodedQuery = String(path[path.index(after: split)...])
        }
        guard let url = components?.url else { throw CallError(status: 0, code: nil) }
        var request = try await client.authorizedRequest(url: url, method: method)
        if method == "POST" { request.httpBody = Data("{}".utf8) }
        request.timeoutInterval = 15
        let data: Data
        let response: URLResponse
        do {
            (data, response) = try await client.session.data(for: request)
        } catch {
            throw CallError(status: 0, code: nil)
        }
        guard let http = response as? HTTPURLResponse else { throw CallError(status: 0, code: nil) }
        guard (200...299).contains(http.statusCode) else {
            let code = (try? JSONDecoder().decode(ErrorBody.self, from: data))?.error
            throw CallError(status: http.statusCode, code: code)
        }
        do {
            return try JSONDecoder().decode(T.self, from: data.isEmpty ? Data("{}".utf8) : data)
        } catch {
            throw APIError.decoding(error)
        }
    }
}
