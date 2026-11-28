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
    /// `phone_action` on a phone approval hold (docs/api/phone-holds.mdx):
    /// Review only, never approved from a row, notification or Siri.
    let subkind: String?
    /// A phone hold's `detail.phone` (what the bot wants to do on which phone).
    let detail: Detail?

    struct Detail: Decodable, Equatable, Sendable {
        let phone: PhoneHoldDetail?
    }

    var canApprove: Bool { actions.contains("approve") }
    var canRetry: Bool { actions.contains("retry") }
    var isPeerHeld: Bool { attentionKind == "peer_held" }
    var isCritical: Bool { kind == "attention" && severity == "critical" }
    var isPhoneAction: Bool { subkind == "phone_action" }
    /// The hold id behind an `approval:<id>` item.
    var holdId: String { id.hasPrefix("approval:") ? String(id.dropFirst("approval:".count)) : id }
}

/// `detail.phone` of a phone approval hold. Every field is optional: the
/// runtime adds `to`/`text` only for a text or call.
struct PhoneHoldDetail: Decodable, Equatable, Sendable {
    let serial: String?
    let phoneName: String?
    let platform: String?
    let app: String?
    let effect: String?
    let tool: String?
    let intent: String?
    let goal: String?
    let runId: String?
    let step: Int?
    let to: String?
    let text: String?
    let element: Element?

    struct Element: Decodable, Equatable, Sendable {
        let label: String?
    }
}


/// A pending AllternitOS kernel approval, from the cloud relay
/// `GET /api/v1/kernel/relay/approvals`. The model never approves one: only
/// the signed-in person does, after reading the exact action.
struct KernelApproval: Decodable, Identifiable, Equatable, Sendable {
    struct Kernel: Decodable, Equatable, Sendable {
        let requestId: String
        let runId: String?
        /// Who asked (the agent or bot the kernel run belongs to).
        let assignee: String?
        let actionClass: String?
        /// The exact action, as the kernel wrote it. Shown whole in the detail.
        let actionSummary: String?
        let actionHash: String?
        /// ISO-8601 or epoch seconds/milliseconds; see `deadlineDate`.
        let deadline: Date?
        let humanOnly: Bool

        private enum CodingKeys: String, CodingKey {
            case requestId, runId, assignee, actionClass, actionSummary, actionHash, deadline, humanOnly
        }

        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            requestId = try c.decode(String.self, forKey: .requestId)
            runId = try? c.decodeIfPresent(String.self, forKey: .runId)
            assignee = try? c.decodeIfPresent(String.self, forKey: .assignee)
            actionClass = try? c.decodeIfPresent(String.self, forKey: .actionClass)
            actionSummary = try? c.decodeIfPresent(String.self, forKey: .actionSummary)
            actionHash = try? c.decodeIfPresent(String.self, forKey: .actionHash)
            // Unknown means human-only: the safe reading.
            humanOnly = (try? c.decodeIfPresent(Bool.self, forKey: .humanOnly)) ?? true
            if let text = try? c.decodeIfPresent(String.self, forKey: .deadline) {
                let fractional = ISO8601DateFormatter()
                fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
                deadline = fractional.date(from: text) ?? ISO8601DateFormatter().date(from: text)
            } else if let number = try? c.decodeIfPresent(Double.self, forKey: .deadline) {
                deadline = Date(timeIntervalSince1970: number > 1e11 ? number / 1000 : number)
            } else {
                deadline = nil
            }
        }
    }

    let runtimeId: String
    let kernel: Kernel
    let resolveRoute: String?

    var requestId: String { kernel.requestId }
    var id: String { Self.key(runtimeId: runtimeId, requestId: requestId) }
    static func key(runtimeId: String, requestId: String) -> String { "\(runtimeId)/\(requestId)" }
}

/// Plain reasons a kernel approval could not be answered.
enum KernelResolveError: LocalizedError, Equatable {
    case forbiddenCaller
    case tokenExpired
    case kernelUnavailable
    case other(String)

    var errorDescription: String? {
        switch self {
        case .forbiddenCaller: return "Only you can answer this, from your signed-in account on this phone."
        case .tokenExpired: return "This request ran out of time. Ask the agent to send it again."
        case .kernelUnavailable: return "Your computer's kernel can't be reached right now. Try again in a moment."
        case .other(let text): return text
        }
    }
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
            // One row the app can't read (a newer kind) must not hide the rest.
            struct Lossy: Decodable {
                let item: InboxItem?
                init(from decoder: Decoder) throws { item = try? InboxItem(from: decoder) }
            }
            struct Body: Decodable { let needsYou: [Lossy]? }
            do {
                // `kernel:<request_id>` rows come from the relay (`kernelApprovals()`),
                // which can resolve them; the generic approve verb answers 409 USE_KERNEL_ROUTE.
                return try JSONDecoder().decode(Body.self, from: data).needsYou?
                    .compactMap(\.item).filter { !$0.id.hasPrefix("kernel:") } ?? []
            } catch {
                throw APIError.decoding(error)
            }
        } catch APIError.httpError(let status, _) where status == 404 || status == 503 {
            return nil
        }
    }

    func approve(_ id: String) async throws { try await act(id, "approve") }
    /// Approve from the open item (the Review pane): `surface: "app"` and the
    /// hash it showed, so a hold that changed meanwhile answers 409.
    func approveReviewed(_ id: String, contentHash: String?) async throws {
        var body: [String: Any] = ["surface": "app"]
        if let contentHash, !contentHash.isEmpty { body["contentHash"] = contentHash }
        try await act(id, "approve", body: body)
    }
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

    // MARK: Kernel approvals (cloud relay, Clerk session only)

    private static let pathSegment = CharacterSet.urlPathAllowed.subtracting(["/"])

    /// Pending kernel approvals, newest first as the server sends them.
    func kernelApprovals() async throws -> [KernelApproval] {
        let request = try await client.authorizedRequest(path: "kernel/relay/approvals")
        let (data, response) = try await client.session.data(for: request)
        try client.validate(response, data: data)
        struct Lossy: Decodable {
            let item: KernelApproval?
            init(from decoder: Decoder) throws { item = try? KernelApproval(from: decoder) }
        }
        struct Body: Decodable { let items: [Lossy]? }
        do {
            return try JSONDecoder().decode(Body.self, from: data).items?.compactMap(\.item) ?? []
        } catch {
            throw APIError.decoding(error)
        }
    }

    /// Approve or deny one kernel request. Goes to the cloud with the signed-in
    /// session, never from a notification button, so the exact action was read first.
    func resolveKernel(_ approval: KernelApproval, approve: Bool, note: String? = nil) async throws {
        let rt = approval.runtimeId.addingPercentEncoding(withAllowedCharacters: Self.pathSegment) ?? approval.runtimeId
        let rq = approval.requestId.addingPercentEncoding(withAllowedCharacters: Self.pathSegment) ?? approval.requestId
        var request = try await client.authorizedRequest(path: "kernel/relay/approvals/\(rt)/\(rq)/resolve", method: "POST")
        var body: [String: Any] = ["decision": approve ? "approve" : "deny"]
        if let note, !note.isEmpty { body["note"] = note }
        request.httpBody = try JSONSerialization.data(withJSONObject: body)
        let (data, response) = try await client.session.data(for: request)
        guard let http = response as? HTTPURLResponse else { throw APIError.invalidResponse }
        if (200...299).contains(http.statusCode) { return }
        let parsed = APIErrorBody.parse(data)
        switch (parsed.code, http.statusCode) {
        case ("forbidden_caller", _), (_, 403): throw KernelResolveError.forbiddenCaller
        case ("token_expired", _), (_, 409): throw KernelResolveError.tokenExpired
        case ("kernel_unavailable", _), (_, 503): throw KernelResolveError.kernelUnavailable
        default:
            try client.validate(response, data: data)
            throw KernelResolveError.other("That didn't go through. Try again.")
        }
    }
}
