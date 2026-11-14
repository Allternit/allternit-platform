import Foundation

// Connect kit v1 (CONTRACT-connections-2026-11.md): one list of everything
// Allternit can use, with the same four status words everywhere.
//
// - cloud-api answers the account rows (`{cloud}/api/v1/connections`,
//   session auth only, so it goes straight to cloud-api with the Clerk
//   Bearer, never through a runtime relay).
// - The active computer's allternit-api answers the rows on that computer
//   (CLI tools, the local brain, device keys), reached through the relay.
// - The app merges both: a runtime row wins over a cloud row with the same
//   id only when its state is `on`.

/// The only four status words.
enum ConnState: String, Codable, Sendable {
    case off, connecting, on, needs

    init(from decoder: Decoder) throws {
        let raw = (try? decoder.singleValueContainer().decode(String.self)) ?? ""
        self = ConnState(rawValue: raw) ?? .off
    }
}

/// One row of `GET /api/v1/connections`. Everything but `id` is optional so a
/// newer server can drop or add fields without breaking older builds.
struct ConnectionWire: Decodable, Equatable, Sendable {
    var id: String
    var group: String?
    var state: ConnState = .off
    var found: String?
    var reason: String?
    var reasonCode: String?
    var runsOn: String?
    var available: Bool = true
    var unavailableReason: String?
    var fallbacks: [String] = []
    var terms: String?
    var saved: String?
    var models: [String] = []
    var source: String?
    /// Runner rows (contract §9.1): the disclosure version and when this
    /// account accepted it (nil = not yet, or a newer version).
    var disclosure: ConnDisclosureState?
    /// Contract §9.1/§9.5: the row's models answer only messages the person
    /// sends in chat. They never go in bot, routine or schedule pickers.
    var personOnly: Bool = false

    private enum CodingKeys: String, CodingKey {
        case id, group, state, found, reason, terms, saved, models, source, fallbacks, available, disclosure
        case reasonCode = "reason_code", runsOn = "runs_on", unavailableReason = "unavailable_reason", personOnly = "person_only"
    }

    private struct TermsObject: Decodable { let status: String? }

    init(id: String, state: ConnState = .off, found: String? = nil, reason: String? = nil, reasonCode: String? = nil,
         available: Bool = true, unavailableReason: String? = nil, fallbacks: [String] = [], source: String? = nil) {
        self.id = id
        self.state = state
        self.found = found
        self.reason = reason
        self.reasonCode = reasonCode
        self.available = available
        self.unavailableReason = unavailableReason
        self.fallbacks = fallbacks
        self.source = source
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        group = try? c.decodeIfPresent(String.self, forKey: .group)
        state = (try? c.decodeIfPresent(ConnState.self, forKey: .state)) ?? .off
        found = try? c.decodeIfPresent(String.self, forKey: .found)
        reason = try? c.decodeIfPresent(String.self, forKey: .reason)
        reasonCode = try? c.decodeIfPresent(String.self, forKey: .reasonCode)
        runsOn = try? c.decodeIfPresent(String.self, forKey: .runsOn)
        available = (try? c.decodeIfPresent(Bool.self, forKey: .available)) ?? true
        unavailableReason = try? c.decodeIfPresent(String.self, forKey: .unavailableReason)
        fallbacks = ((try? c.decodeIfPresent([String].self, forKey: .fallbacks)) ?? nil) ?? []
        // `terms` is a string in the contract and `{status}` in the plan: read either.
        if let text = try? c.decodeIfPresent(String.self, forKey: .terms) {
            terms = text
        } else {
            terms = (try? c.decodeIfPresent(TermsObject.self, forKey: .terms))??.status
        }
        saved = try? c.decodeIfPresent(String.self, forKey: .saved)
        models = ((try? c.decodeIfPresent([String].self, forKey: .models)) ?? nil) ?? []
        source = try? c.decodeIfPresent(String.self, forKey: .source)
        disclosure = try? c.decodeIfPresent(ConnDisclosureState.self, forKey: .disclosure)
        personOnly = (try? c.decodeIfPresent(Bool.self, forKey: .personOnly)) ?? false
    }

    /// `runtime:<id>` rows live on a computer.
    var runtimeId: String? {
        guard let source, source.hasPrefix("runtime:") else { return nil }
        return String(source.dropFirst("runtime:".count))
    }
}

/// `disclosure` on a runner row (contract §9.1).
struct ConnDisclosureState: Decodable, Equatable, Sendable {
    var version: String?
    var acceptedAt: String?

    private enum CodingKeys: String, CodingKey { case version, acceptedAt = "accepted_at" }
}

/// The disclosure text the server sends with `409 disclosure_required`
/// (contract §9.2). Shown exactly as given, never a copy of our own.
struct ConnDisclosure: Decodable, Equatable, Sendable {
    var version: String
    var title: String
    var body: [String]
    var acceptLabel: String
    var declineLabel: String

    private enum CodingKeys: String, CodingKey {
        case version, title, body, acceptLabel = "accept_label", declineLabel = "decline_label"
    }

    init(version: String, title: String, body: [String], acceptLabel: String, declineLabel: String) {
        self.version = version
        self.title = title
        self.body = body
        self.acceptLabel = acceptLabel
        self.declineLabel = declineLabel
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        version = try c.decode(String.self, forKey: .version)
        title = (try? c.decodeIfPresent(String.self, forKey: .title)) ?? ""
        body = ((try? c.decodeIfPresent([String].self, forKey: .body)) ?? nil) ?? []
        acceptLabel = ((try? c.decodeIfPresent(String.self, forKey: .acceptLabel)) ?? nil) ?? "I understand, connect"
        declineLabel = ((try? c.decodeIfPresent(String.self, forKey: .declineLabel)) ?? nil) ?? "Use a key instead"
    }
}

/// A connections call the server refused with a code (contract §9.2, §9.4).
enum ConnectionsError: Error, Equatable, Sendable {
    /// `409 disclosure_required`: show the text, then connect again with
    /// `accept_disclosure` set to its version.
    case disclosureRequired(ConnDisclosure)
    /// Any other `{error:{code, message}}` body.
    case refused(status: Int, code: String, message: String?)

    private struct Body: Decodable {
        struct Inner: Decodable {
            let code: String?
            let message: String?
            let disclosure: ConnDisclosure?
        }
        let error: Inner?
    }

    /// Reads a failed response body. nil when it carries no error code (the
    /// caller then falls back to the plain HTTP status).
    static func parse(status: Int, data: Data) -> ConnectionsError? {
        guard let body = try? JSONDecoder().decode(Body.self, from: data), let inner = body.error,
              let code = inner.code, !code.isEmpty else { return nil }
        if code == "disclosure_required", let disclosure = inner.disclosure {
            return .disclosureRequired(disclosure)
        }
        return .refused(status: status, code: code, message: inner.message)
    }
}

struct ConnectionsEnvelope: Decodable, Sendable {
    let version: Int?
    let connections: [ConnectionWire]
}

/// `{login}` from `/connect`: what the sheet shows next.
struct ConnLogin: Decodable, Equatable, Sendable {
    enum Kind: String, Decodable, Sendable {
        case deviceCode = "device_code", linkPaste = "link_paste", oauth, paste, scan, pair, unknown

        init(from decoder: Decoder) throws {
            let raw = (try? decoder.singleValueContainer().decode(String.self)) ?? ""
            self = Kind(rawValue: raw) ?? .unknown
        }
    }

    var kind: Kind
    var code: String?
    var url: String?
    var urlWithCode: String?
    var expiresAt: String?
    var poll: Bool

    private enum CodingKeys: String, CodingKey {
        case kind, code, url, poll, urlWithCode = "url_with_code", expiresAt = "expires_at"
    }

    init(kind: Kind, code: String? = nil, url: String? = nil, urlWithCode: String? = nil, expiresAt: String? = nil, poll: Bool = true) {
        self.kind = kind
        self.code = code
        self.url = url
        self.urlWithCode = urlWithCode
        self.expiresAt = expiresAt
        self.poll = poll
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        kind = (try? c.decode(Kind.self, forKey: .kind)) ?? .unknown
        code = try? c.decodeIfPresent(String.self, forKey: .code)
        url = try? c.decodeIfPresent(String.self, forKey: .url)
        urlWithCode = try? c.decodeIfPresent(String.self, forKey: .urlWithCode)
        expiresAt = try? c.decodeIfPresent(String.self, forKey: .expiresAt)
        poll = (try? c.decodeIfPresent(Bool.self, forKey: .poll)) ?? true
    }

    /// The page to open: the one with the code in it when the vendor has one.
    var openURL: URL? {
        (urlWithCode ?? url).flatMap { URL(string: $0) }
    }

    var expiry: Date? { expiresAt.flatMap(VoiceKeyRemote.parseDate) }
}

/// What `/connect` answers.
struct ConnConnectResult: Decodable, Sendable {
    var login: ConnLogin?
    var state: ConnState?
}

/// What `/login` answers while the vendor sign-in runs.
struct ConnLoginPoll: Decodable, Sendable {
    enum Status: String, Decodable, Sendable {
        case pending, done, expired, failed

        init(from decoder: Decoder) throws {
            let raw = (try? decoder.singleValueContainer().decode(String.self)) ?? ""
            self = Status(rawValue: raw) ?? .failed
        }
    }

    var status: Status
    var row: ConnectionWire?
}

/// Where a connection's calls go.
enum ConnTarget: Equatable, Sendable {
    case cloud
    case runtime(String)
}

enum ConnectionsMerge {
    /// One list from both sources (contract §3): unknown ids are dropped, a
    /// runtime row replaces a cloud row with the same id only when it is
    /// `on`, and a runtime row with no cloud row is used as it is.
    static func merge(cloud: [ConnectionWire], runtime: [ConnectionWire], known: Set<String>) -> [String: ConnectionWire] {
        var out: [String: ConnectionWire] = [:]
        for row in cloud where known.contains(row.id) { out[row.id] = row }
        for row in runtime where known.contains(row.id) {
            if out[row.id] == nil || row.state == .on { out[row.id] = row }
        }
        return out
    }
}

/// The calls (contract §3–4). Stateless; the store owns the results.
enum ConnectionsClient {
    private static var cloudBase: URL { AppConfig.cloudAPIBaseURL.appendingPathComponent("api/v1/connections") }

    private static func url(_ target: ConnTarget, _ rest: String = "") -> URL {
        switch target {
        case .cloud:
            return rest.isEmpty ? cloudBase : cloudBase.appendingPathComponent(rest)
        case .runtime(let id):
            let base = RuntimeRelayClient.relayBaseURL(runtimeId: id).appendingPathComponent("api/v1/connections")
            return rest.isEmpty ? base : base.appendingPathComponent(rest)
        }
    }

    /// Every cloud call says `caps=runner`: this app knows the disclosure
    /// step, so plan rows are offered to it (contract §9.1, D10). Servers
    /// ignore caps they don't know.
    static let cloudQuery = [URLQueryItem(name: "caps", value: "runner")]

    private static func send(_ target: ConnTarget, _ rest: String = "", method: String = "GET", body: [String: Any]? = nil) async throws -> Data {
        let client = APIClient.shared
        var full = url(target, rest)
        if target == .cloud, var components = URLComponents(url: full, resolvingAgainstBaseURL: false) {
            components.queryItems = cloudQuery
            full = components.url ?? full
        }
        var request = try await client.authorizedRequest(url: full, method: method)
        request.cachePolicy = .reloadIgnoringLocalCacheData
        if let body { request.httpBody = try JSONSerialization.data(withJSONObject: body) }
        let (data, response): (Data, URLResponse)
        switch target {
        case .cloud:
            // Session auth only: straight to cloud-api, never the runtime relay.
            if SampleURLProtocol.handles(request.url) {
                (data, response) = try await client.send(request)
            } else {
                (data, response) = try await client.session.data(for: request)
            }
        case .runtime:
            (data, response) = try await client.send(request)
        }
        // Coded refusals (disclosure_required, runner_busy, …) keep their code.
        if let http = response as? HTTPURLResponse, !(200...299).contains(http.statusCode),
           let coded = ConnectionsError.parse(status: http.statusCode, data: data) {
            throw coded
        }
        try client.validate(response, data: data)
        return data
    }

    private static func decode<T: Decodable>(_ type: T.Type, _ data: Data) throws -> T {
        do { return try JSONDecoder().decode(type, from: data) } catch { throw APIError.decoding(error) }
    }

    static func list(_ target: ConnTarget) async throws -> [ConnectionWire] {
        try decode(ConnectionsEnvelope.self, try await send(target)).connections
    }

    /// `acceptDisclosure` is the version the person just accepted (runner rows).
    static func connect(_ id: String, target: ConnTarget, way: String? = nil, acceptDisclosure: String? = nil) async throws -> ConnConnectResult {
        var body: [String: Any] = [:]
        if let way { body["way"] = way }
        if let acceptDisclosure { body["accept_disclosure"] = acceptDisclosure }
        let data = try await send(target, "\(id)/connect", method: "POST", body: body)
        return try decode(ConnConnectResult.self, data)
    }

    static func submit(_ id: String, target: ConnTarget, value: String) async throws -> ConnectionWire {
        try decode(ConnectionWire.self, try await send(target, "\(id)/submit", method: "POST", body: ["value": value]))
    }

    static func login(_ id: String, target: ConnTarget) async throws -> ConnLoginPoll {
        try decode(ConnLoginPoll.self, try await send(target, "\(id)/login"))
    }

    static func disconnect(_ id: String, target: ConnTarget) async throws {
        _ = try await send(target, id, method: "DELETE")
    }

    /// A short plain sentence for a failed call.
    static func describe(_ error: Error) -> String {
        if let coded = error as? ConnectionsError {
            switch coded {
            case .disclosureRequired: return "Read and accept the note first."
            case .refused(let status, let code, let message):
                switch code {
                case "runner_busy": return "Busy. Try again"
                case "could_not_start": return "Could not start. Try again"
                case "update_app": return "Update the app to connect this plan."
                case "not_found": return "This isn't offered on your account right now. Use a key for now."
                case "login_expired": return "Sign-in expired. Sign in again."
                case "vendor_limited": return message ?? "The vendor limited this plan."
                default:
                    if let message, !message.isEmpty { return message }
                    return describe(APIError.httpError(statusCode: status, message: nil))
                }
            }
        }
        if case APIError.httpError(let status, let message) = error {
            switch status {
            case 401: return "Sign in to Allternit on this phone, then try again."
            case 403: return "Your Allternit session can't use this. Sign in again."
            case 404: return "This isn't ready on the server yet. Try again later."
            default: return message ?? "The server answered \(status). Try again."
            }
        }
        if error is RuntimeRelayError { return "Your computer is offline." }
        if error is URLError { return "No connection. Try again." }
        return "Could not start. Try again."
    }
}
