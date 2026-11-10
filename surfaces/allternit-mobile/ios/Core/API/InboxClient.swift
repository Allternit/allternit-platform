import Foundation

/// The owner's "Needs you" inbox — runtime `GET /api/v1/inbox?scope=needs_you`
/// (allternit-api on the user's computer, relayed like Cowork sessions; the
/// web's src/lib/inbox/inbox-api.ts). Items come from where the work lives,
/// so approve / deny / dismiss / snooze / retry change the real thing.
struct InboxItem: Decodable, Identifiable, Equatable, Sendable {
    struct Primary: Decodable, Equatable, Sendable {
        /// approve · open · reply · callback · retry
        let action: String
        let label: String
    }

    struct Provenance: Decodable, Equatable, Sendable {
        let trust: String?
        let sourceKind: String?
    }

    let id: String
    let kind: String
    let botId: String
    let botName: String
    let threadId: String?
    let person: String?
    let summary: String
    let at: String
    let primary: Primary
    let actions: [String]
    let severity: String?
    let attentionKind: String?
    let provenance: Provenance?
    /// `peer_held` items: what the outside agent said.
    let text: String?
    /// Hash of the text and recipient; sent back with Approve so a changed item answers 409.
    let contentHash: String?

    var canApprove: Bool { actions.contains("approve") }
    var canRetry: Bool { actions.contains("retry") }
    var isPeerHeld: Bool { attentionKind == "peer_held" }
    var isCritical: Bool { kind == "attention" && severity == "critical" }
}

final class InboxClient: @unchecked Sendable {
    static let shared = InboxClient()

    private let client: APIClient

    init(client: APIClient = .shared) {
        self.client = client
    }

    /// nil = this computer's runtime has no inbox yet (404 / 503).
    func list() async throws -> [InboxItem]? {
        let request = try await client.authorizedRequest(path: "inbox?scope=needs_you")
        do {
            let (data, response) = try await InstanceConnection.runtimeAPIData(for: request, session: client.session)
            try client.validate(response, data: data)
            struct Body: Decodable { let needsYou: [InboxItem]? }
            do {
                return try JSONDecoder().decode(Body.self, from: data).needsYou ?? []
            } catch {
                throw APIError.decoding(error)
            }
        } catch APIError.httpError(let status, _) where status == 404 || status == 503 {
            return nil
        }
    }

    func approve(_ id: String) async throws { try await act(id, "approve") }
    func deny(_ id: String) async throws { try await act(id, "approve", body: ["decision": "deny"]) }
    /// Also how a held message from an outside agent is refused.
    func dismiss(_ id: String) async throws { try await act(id, "dismiss") }
    func retry(_ id: String) async throws { try await act(id, "retry") }
    func snooze(_ id: String, minutes: Int = 60) async throws { try await act(id, "snooze", body: ["minutes": minutes]) }

    /// Background path (notification buttons, Live Activity, App Intents):
    /// sends `bearer` as given, never asking AuthManager, and relays to
    /// `runtimeId` (the item's own computer) when set, else the active one.
    /// Unlike the sheet's calls it does not swallow 404, so the caller can
    /// tell "already handled" from "done".
    func act(_ id: String, verb: String, body: [String: Any] = [:], runtimeId: String?, bearer: String) async throws {
        let escaped = id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed.subtracting(["/"])) ?? id
        var request = URLRequest(url: client.baseURL.appendingPathComponent("inbox/\(escaped)/\(verb)"))
        request.httpMethod = "POST"
        request.timeoutInterval = 20
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        request.setValue("Bearer \(bearer)", forHTTPHeaderField: "Authorization")
        request.httpBody = try JSONSerialization.data(withJSONObject: body)
        let data: Data
        let response: URLResponse
        if let runtimeId {
            let relay = RuntimeRelayClient(session: client.session, tokenProvider: { bearer })
            (data, response) = try await relay.send(request, runtimeId: runtimeId)
        } else {
            (data, response) = try await InstanceConnection.runtimeAPIData(for: request, session: client.session)
        }
        try client.validate(response, data: data)
    }

    private func act(_ id: String, _ verb: String, body: [String: Any] = [:]) async throws {
        let escaped = id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed.subtracting(["/"])) ?? id
        var request = try await client.authorizedRequest(path: "inbox/\(escaped)/\(verb)", method: "POST")
        request.httpBody = try JSONSerialization.data(withJSONObject: body)
        do {
            let (data, response) = try await InstanceConnection.runtimeAPIData(for: request, session: client.session)
            try client.validate(response, data: data)
        } catch APIError.httpError(let status, _) where status == 404 {
            // Already handled elsewhere (another device, the bot).
        }
    }
}
