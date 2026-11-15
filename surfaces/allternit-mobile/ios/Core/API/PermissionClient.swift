import Foundation

/// Client for gizzi-code's permission/approval queue
/// (`cmd/gizzi-code/src/runtime/tools/guard/permission/next.ts`), mounted at
/// `/v1/permission` (`cmd/gizzi-code/src/runtime/server/server.ts:464`,
/// `cmd/gizzi-code/src/runtime/server/routes/permission.ts`) — same shape as
/// `PtyClient`: the shared client resolves its target per call through
/// `InstanceConnection` (explicit URL → mesh → the relay to the user's
/// computer), so the queue and the `/v1/event` stream work in Release.
///
/// `subscribeToEvents()` streams the bus events behind `permission.ts`'s SSE
/// `/event` route (`Event.Asked`/`Event.Replied`, `next.ts:101-111`) in
/// real time; `CodeModeView.pollPendingPermissions()` consumes it and falls
/// back to a plain `listPending()` poll if the stream drops.
final class PermissionClient: @unchecked Sendable {
    static let shared = PermissionClient()

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

    /// `GET /v1/permission` — every pending request across all sessions
    /// (`permission.ts:73-93`, backed by `PermissionNext.list()` at
    /// `next.ts:470-473`). Callers filter down to their own session id.
    func listPending() async throws -> [PermissionRequest] {
        try await client().get(path: "v1/permission")
    }

    /// `POST /v1/permission/:requestID/reply` — body `{ reply, message? }`
    /// (`permission.ts:42-71`, `PermissionNext.reply` at `next.ts:208-297`).
    /// The route answers the JSON literal `true` (`c.json(true)`), not a
    /// model — decoded as `Bool` and discarded.
    func reply(requestID: String, reply: PermissionReply, message: String? = nil) async throws {
        let _: Bool = try await client().post(
            path: "v1/permission/\(Self.escape(requestID))/reply",
            body: ReplyRequestBody(reply: reply, message: message)
        )
    }

    /// Subscribes to the server-sent events stream (`GET /v1/event`).
    /// Yields parsed permission asked/replied events in real-time.
    func subscribeToEvents() -> AsyncThrowingStream<PermissionBusEvent, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    let client = try await self.client()
                    var request = try await client.authorizedRequest(path: "v1/event")
                    request.setValue("text/event-stream", forHTTPHeaderField: "Accept")
                    request.timeoutInterval = 600

                    let (bytes, response) = try await client.sendStream(request)
                    try client.validate(response)

                    let decoder = JSONDecoder()
                    for try await line in bytes.lines {
                        try Task.checkCancellation()

                        guard line.hasPrefix("data: ") else { continue }
                        let payload = String(line.dropFirst(6))
                        guard !payload.isEmpty else { continue }

                        guard let data = payload.data(using: .utf8) else { continue }

                        if let event = try? decoder.decode(PermissionBusEvent.self, from: data) {
                            if case .ignored = event { continue }
                            continuation.yield(event)
                        }
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    /// Same path-escaping idiom as `PtyClient.escape` (parity with the web's
    /// `encodeURIComponent` on path ids).
    private static func escape(_ requestID: String) -> String {
        requestID.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? requestID
    }
}

private struct ReplyRequestBody: Encodable {
    let reply: PermissionReply
    let message: String?
}

/// Dynamic events received over the permission bus (`/v1/event` route).
enum PermissionBusEvent: Decodable, Sendable {
    case asked(PermissionRequest)
    case replied(PermissionRepliedInfo)
    case ignored

    enum CodingKeys: String, CodingKey {
        case type, properties
    }

    struct PermissionRepliedInfo: Decodable, Sendable {
        let sessionID: String
        let requestID: String
        let reply: PermissionReply
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let type = try container.decode(String.self, forKey: .type)

        switch type {
        case "permission.asked":
            let request = try container.decode(PermissionRequest.self, forKey: .properties)
            self = .asked(request)
        case "permission.replied":
            let info = try container.decode(PermissionRepliedInfo.self, forKey: .properties)
            self = .replied(info)
        default:
            self = .ignored
        }
    }
}

/// Mirrors `PermissionNext.Request` (`next.ts:71-88`) field-for-field. The
/// server's `tool?: { messageID, callID }` isn't needed for Phase 1 UI and is
/// omitted here — `Decodable` ignores unknown/undeclared JSON keys, so
/// leaving it out doesn't block decoding requests that carry it.
struct PermissionRequest: Decodable, Sendable, Identifiable {
    let id: String
    let sessionID: String
    /// e.g. "edit", "write", "patch", "multiedit", "bash".
    let permission: String
    let patterns: [String]
    let metadata: PermissionMetadata
    let always: [String]
}

/// Mirrors `PermissionNext.Reply` (`next.ts:90-91`).
enum PermissionReply: String, Codable, Sendable {
    case once, always, reject
}

/// Narrow decode of `metadata: Record<string, any>` (`next.ts:77`).
///
/// No `AnyCodable`/`AnyDecodable` type exists anywhere in this codebase
/// (checked `Core/` and `Features/`), and Phase 1's UI only ever reads two
/// keys, so this decodes exactly those rather than adding a general-purpose
/// JSON value type:
/// - `diff` — the unified diff string `edit.ts:48-149` attaches via
///   `createTwoFilesPatch` on every edit/write/patch/multiedit call that
///   goes through the permission gate.
/// - `filediff` — `Snapshot.FileDiff`'s `additions`/`deletions` line counts
///   (also set in `edit.ts`, e.g. lines 112-122).
///
/// Both are absent for non-file-edit permissions (e.g. `bash`), which is
/// exactly the fallback-UI case (`ChangesetReviewSheet`). Any other metadata
/// keys are ignored by `Decodable` automatically.
struct PermissionMetadata: Decodable, Sendable {
    let diff: String?
    let filediff: FileDiffCounts?

    struct FileDiffCounts: Decodable, Sendable {
        let additions: Int?
        let deletions: Int?
    }
}
