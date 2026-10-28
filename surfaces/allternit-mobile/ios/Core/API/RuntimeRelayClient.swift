import Foundation

// MARK: - Runtime devices (relay view)

/// One live relay connection a runtime holds (`relayConnections[]` on
/// `GET /api/v1/runtime-devices`, only listed in multi-connection mode).
struct RelayConnectionInfo: Decodable, Sendable {
    let client: String?
    let capabilities: [String]?
}

/// A runtime device as the relay picker needs it — the fields of
/// `GET {cloud}/api/v1/runtime-devices` (runtime_pairing.rs
/// `list_runtime_devices`) beyond the plain `RuntimeDevice` list row:
/// `preferred` (the account's default computer), `wakeable` (a sleeping
/// cloud computer a relay request wakes) and `relayConnections`.
struct RelayRuntimeDevice: Decodable, Sendable, Identifiable, Equatable {
    let id: String
    let name: String
    let runtimeType: String
    let hostname: String?
    let platform: String?
    let status: String
    let lastSeenAt: String?
    let preferred: Bool?
    let wakeable: Bool?
    let relayConnectionCount: Int?

    var isOnline: Bool { status == "online" || status == "busy" }
    var isPreferred: Bool { preferred == true }
    var isWakeable: Bool { wakeable == true }
    var hasRelayConnection: Bool { (relayConnectionCount ?? 0) > 0 }

    var displayName: String {
        if !name.isEmpty { return name }
        return hostname ?? "Unnamed computer"
    }

    var typeLabel: String { RuntimeDevice.typeLabel(for: runtimeType) }

    private enum CodingKeys: String, CodingKey {
        case id, name, runtimeType, hostname, platform, status, lastSeenAt
        case preferred, wakeable, relayConnections
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        name = (try? c.decodeIfPresent(String.self, forKey: .name)) ?? ""
        runtimeType = (try? c.decodeIfPresent(String.self, forKey: .runtimeType)) ?? "desktop"
        hostname = try? c.decodeIfPresent(String.self, forKey: .hostname)
        platform = try? c.decodeIfPresent(String.self, forKey: .platform)
        status = (try? c.decodeIfPresent(String.self, forKey: .status)) ?? "offline"
        lastSeenAt = try? c.decodeIfPresent(String.self, forKey: .lastSeenAt)
        preferred = try? c.decodeIfPresent(Bool.self, forKey: .preferred)
        wakeable = try? c.decodeIfPresent(Bool.self, forKey: .wakeable)
        relayConnectionCount = (try? c.decodeIfPresent([RelayConnectionInfo].self, forKey: .relayConnections))?.count
    }
}

private struct RelayRuntimesResponse: Decodable {
    let runtimes: [RelayRuntimeDevice]
}

// MARK: - Errors

/// Relay failures the UI turns into the "Connect a computer" state. Copy
/// matches the PWA (fetch-interceptor.ts RUNTIME_UNAVAILABLE_MESSAGE /
/// RUNTIME_UNREACHABLE_MESSAGE).
enum RuntimeRelayError: Error, LocalizedError, Sendable, Equatable {
    /// No computer is paired/online to relay through.
    case noComputer
    /// The picked computer did not answer (relay said `runtime_offline`, or
    /// it stayed asleep past the wake wait).
    case unreachable
    /// The socket ticket call succeeded without a ticket.
    case socketUnavailable(String)

    var errorDescription: String? {
        switch self {
        case .noComputer:
            return "No paired Allternit runtime is online. Open Allternit Desktop or start your VPS runtime."
        case .unreachable:
            return "Your computer isn't responding. Open Allternit Desktop or wake your cloud computer, then try again."
        case .socketUnavailable(let message):
            return message
        }
    }
}

// MARK: - Relay client

/// Talks to the user's own computer (Allternit Desktop, a VPS node or a
/// cloud computer) through allternit-cloud-api's runtime relay — the same
/// path the PWA's fetch interceptor takes on m.allternit.com:
///
/// - HTTP: `POST {cloud}/api/v1/runtime-devices/:id/proxy` with the
///   `BrowserProxyRequest` body `{ method, path, headers, body, bodyEncoding }`
///   (runtime_relay.rs). The response IS the runtime's response (status,
///   allow-listed headers, streamed body), so SSE works unchanged.
/// - WebSocket: `POST .../:id/socket-ticket` `{ path }` → `{ ticket }`, then
///   `wss://{cloud}/api/v1/runtime-devices/:id/socket?ticket=`; the first
///   text frame is the `{"type":"allternit_socket_ready"}` sentinel, after
///   which frames are the runtime's own (fetch-interceptor.ts
///   RuntimeRelayWebSocket).
///
/// Clients address a relayed runtime with a placeholder base URL
/// (`allternit-relay://<runtimeId>`, see `relayBaseURL(runtimeId:)`):
/// `APIClient` and `InstanceConnection` recognise it and send through here,
/// so existing `baseURL.appendingPathComponent(...)` call sites keep working.
final class RuntimeRelayClient: @unchecked Sendable {
    static let shared = RuntimeRelayClient()

    static let scheme = "allternit-relay"

    /// How long a request waits for a sleeping cloud computer (PWA
    /// RUNTIME_WAKE_TIMEOUT_MS is 120 s).
    private static let wakeTimeout: TimeInterval = 120

    private let session: URLSession
    private let tokenProvider: @Sendable () async throws -> String?

    init(session: URLSession = .shared,
         tokenProvider: @escaping @Sendable () async throws -> String? = { try await AuthManager.shared.getToken() }) {
        self.session = session
        self.tokenProvider = tokenProvider
    }

    // MARK: Placeholder base URLs

    /// The base URL clients use for a relayed runtime. Only its host (the
    /// runtime id) and the request's path+query matter; it is never dialled.
    static func relayBaseURL(runtimeId: String) -> URL {
        var components = URLComponents()
        components.scheme = scheme
        let allowed = CharacterSet.alphanumerics.union(CharacterSet(charactersIn: "-_."))
        components.percentEncodedHost = runtimeId.addingPercentEncoding(withAllowedCharacters: allowed) ?? runtimeId
        return components.url ?? URL(string: "\(scheme)://\(runtimeId)")!
    }

    /// The runtime id behind a `relayBaseURL`, nil for any other URL.
    static func runtimeId(in url: URL?) -> String? {
        guard let url, url.scheme == scheme,
              let components = URLComponents(url: url, resolvingAgainstBaseURL: false),
              let host = components.percentEncodedHost, !host.isEmpty else {
            return nil
        }
        return host.removingPercentEncoding ?? host
    }

    static func isRelayURL(_ url: URL?) -> Bool { runtimeId(in: url) != nil }

    // MARK: Device list

    /// `GET {cloud}/api/v1/runtime-devices` — the signed-in user's computers.
    func fetchDevices() async throws -> [RelayRuntimeDevice] {
        let url = AppConfig.cloudAPIBaseURL.appendingPathComponent("api/v1/runtime-devices")
        var request = URLRequest(url: url)
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        try await authorize(&request)
        let (data, response) = try await session.data(for: request)
        try Self.validate(response, data: data)
        do {
            return try JSONDecoder().decode(RelayRuntimesResponse.self, from: data).runtimes
        } catch {
            throw APIError.decoding(error)
        }
    }

    // MARK: HTTP

    /// Sends `request` to runtime `runtimeId` through the relay. Waits out a
    /// waking cloud computer (503 `runtime_warming`); a `runtime_offline`
    /// answer marks the runtime unreachable and throws
    /// `RuntimeRelayError.unreachable`.
    func send(_ request: URLRequest, runtimeId: String) async throws -> (Data, URLResponse) {
        let relay = try await envelopeRequest(for: request, runtimeId: runtimeId)
        let deadline = Date().addingTimeInterval(Self.wakeTimeout)
        var waking = false
        defer { if waking { Task { @MainActor in RuntimeRelayStore.shared.setWaking(false) } } }
        while true {
            let (data, response) = try await session.data(for: relay)
            switch Self.relayCondition(response: response, data: data) {
            case .warming(let delay) where Date().addingTimeInterval(delay) < deadline:
                if !waking {
                    waking = true
                    await MainActor.run { RuntimeRelayStore.shared.setWaking(true) }
                }
                try await Task.sleep(for: .seconds(delay))
                continue
            case .offline, .warming:
                await MainActor.run { RuntimeRelayStore.shared.markUnreachable(runtimeId) }
                throw RuntimeRelayError.unreachable
            case .ok:
                await MainActor.run { RuntimeRelayStore.shared.markReachable(runtimeId) }
                return (data, response)
            }
        }
    }

    /// Streaming variant of `send` (SSE). The relay streams the runtime's
    /// body chunk by chunk, so `bytes.lines` works as against the runtime.
    func stream(_ request: URLRequest, runtimeId: String) async throws -> (URLSession.AsyncBytes, URLResponse) {
        let relay = try await envelopeRequest(for: request, runtimeId: runtimeId)
        let deadline = Date().addingTimeInterval(Self.wakeTimeout)
        var waking = false
        defer { if waking { Task { @MainActor in RuntimeRelayStore.shared.setWaking(false) } } }
        while true {
            let (bytes, response) = try await session.bytes(for: relay)
            guard (response as? HTTPURLResponse)?.statusCode == 503 else {
                await MainActor.run { RuntimeRelayStore.shared.markReachable(runtimeId) }
                return (bytes, response)
            }
            var data = Data()
            for try await byte in bytes { data.append(byte) }
            switch Self.relayCondition(response: response, data: data) {
            case .warming(let delay) where Date().addingTimeInterval(delay) < deadline:
                if !waking {
                    waking = true
                    await MainActor.run { RuntimeRelayStore.shared.setWaking(true) }
                }
                try await Task.sleep(for: .seconds(delay))
                continue
            case .offline, .warming:
                await MainActor.run { RuntimeRelayStore.shared.markUnreachable(runtimeId) }
                throw RuntimeRelayError.unreachable
            case .ok:
                // A plain 503 from the runtime itself: surface it as such.
                throw APIError.httpError(statusCode: 503, message: Self.errorMessage(from: data))
            }
        }
    }

    // MARK: WebSocket

    /// Opens a relayed WebSocket to `path` (e.g. `/v1/pty/:id/connect`) on
    /// runtime `runtimeId`. The returned task is already resumed and past
    /// the `allternit_socket_ready` sentinel, so every frame the caller
    /// receives is the runtime's own. Calling `resume()` again is a no-op.
    func webSocket(path: String, runtimeId: String) async throws -> URLSessionWebSocketTask {
        let escapedId = Self.escapePathSegment(runtimeId)
        let ticketURL = AppConfig.cloudAPIBaseURL
            .appendingPathComponent("api/v1/runtime-devices/\(escapedId)/socket-ticket")
        var ticketRequest = URLRequest(url: ticketURL)
        ticketRequest.httpMethod = "POST"
        ticketRequest.setValue("application/json", forHTTPHeaderField: "Content-Type")
        ticketRequest.httpBody = try JSONEncoder().encode(["path": path])
        try await authorize(&ticketRequest)

        // A sleeping computer answers the ticket with 503 "waking up": wait
        // it out like the HTTP relay does (fetch-interceptor.ts:469-482).
        let deadline = Date().addingTimeInterval(Self.wakeTimeout)
        var ticketData = Data()
        while true {
            let (data, response) = try await session.data(for: ticketRequest)
            if let http = response as? HTTPURLResponse, http.statusCode == 503 {
                if Date().addingTimeInterval(3) < deadline,
                   Self.errorMessage(from: data)?.localizedCaseInsensitiveContains("waking") == true {
                    await MainActor.run { RuntimeRelayStore.shared.setWaking(true) }
                    try await Task.sleep(for: .seconds(3))
                    continue
                }
                await MainActor.run {
                    RuntimeRelayStore.shared.setWaking(false)
                    RuntimeRelayStore.shared.markUnreachable(runtimeId)
                }
                throw RuntimeRelayError.unreachable
            }
            await MainActor.run { RuntimeRelayStore.shared.setWaking(false) }
            try Self.validate(response, data: data)
            ticketData = data
            break
        }
        struct TicketResponse: Decodable { let ticket: String? }
        guard let ticket = (try? JSONDecoder().decode(TicketResponse.self, from: ticketData))?.ticket,
              !ticket.isEmpty else {
            throw RuntimeRelayError.socketUnavailable("Runtime socket ticket was not issued")
        }

        guard var components = URLComponents(
            url: AppConfig.cloudAPIBaseURL.appendingPathComponent("api/v1/runtime-devices/\(escapedId)/socket"),
            resolvingAgainstBaseURL: false
        ) else {
            throw APIError.invalidResponse
        }
        components.scheme = components.scheme == "https" ? "wss" : "ws"
        components.queryItems = [URLQueryItem(name: "ticket", value: ticket)]
        guard let socketURL = components.url else { throw APIError.invalidResponse }

        let task = session.webSocketTask(with: socketURL)
        task.resume()
        // Wait for the tunnel handshake; a socket that never gets ready is
        // torn down after 20 s so the caller's receive loop fails cleanly.
        let watchdog = Task {
            try? await Task.sleep(for: .seconds(20))
            if !Task.isCancelled { task.cancel(with: .goingAway, reason: nil) }
        }
        defer { watchdog.cancel() }
        while true {
            let message: URLSessionWebSocketTask.Message
            do {
                message = try await task.receive()
            } catch {
                await MainActor.run { RuntimeRelayStore.shared.markUnreachable(runtimeId) }
                throw RuntimeRelayError.unreachable
            }
            // Frames before the sentinel are dropped, as on the PWA.
            if case .string(let text) = message, text.contains("allternit_socket_ready") {
                await MainActor.run { RuntimeRelayStore.shared.markReachable(runtimeId) }
                return task
            }
        }
    }

    // MARK: Envelope

    /// Body of the relay POST — `BrowserProxyRequest` (runtime_relay.rs,
    /// `#[serde(rename_all = "camelCase")]`).
    private struct Envelope: Encodable {
        let method: String
        let path: String
        let headers: [String: String]
        let body: String
        let bodyEncoding: String
    }

    /// Headers the web interceptor forwards into the envelope; cloud-api
    /// filters again on its side (runtime_relay.rs `filtered_headers`).
    private static let forwardedHeaders: Set<String> = [
        "accept", "content-type", "if-none-match", "if-modified-since",
        "last-event-id", "x-request-id",
        // D16: a person's own act (plan approval); cloud-api forwards it.
        "x-allternit-human-proof",
        "x-allternit-device-id", "x-allternit-device-name", "x-allternit-device-platform",
    ]

    private func envelopeRequest(for request: URLRequest, runtimeId: String) async throws -> URLRequest {
        let method = (request.httpMethod ?? "GET").uppercased()
        var path = "/"
        if let url = request.url,
           let components = URLComponents(url: url, resolvingAgainstBaseURL: false) {
            path = components.percentEncodedPath.isEmpty ? "/" : components.percentEncodedPath
            if let query = components.percentEncodedQuery, !query.isEmpty {
                path += "?\(query)"
            }
        }

        var headers: [String: String] = [:]
        for (name, value) in request.allHTTPHeaderFields ?? [:] {
            let lowercased = name.lowercased()
            if Self.forwardedHeaders.contains(lowercased) { headers[lowercased] = value }
        }

        // Text bodies go as utf8, binary as base64, empty as "" + utf8
        // (fetch-interceptor.ts requestBody()).
        var body = ""
        var bodyEncoding = "utf8"
        if let httpBody = request.httpBody, !httpBody.isEmpty {
            if let text = String(data: httpBody, encoding: .utf8) {
                body = text
            } else {
                body = httpBody.base64EncodedString()
                bodyEncoding = "base64"
            }
        }

        var relay = URLRequest(url: AppConfig.runtimeProxyURL(runtimeId: runtimeId))
        relay.httpMethod = "POST"
        relay.setValue("application/json", forHTTPHeaderField: "Content-Type")
        relay.timeoutInterval = request.timeoutInterval
        try await authorize(&relay)
        relay.httpBody = try JSONEncoder().encode(
            Envelope(method: method, path: path, headers: headers, body: body, bodyEncoding: bodyEncoding)
        )
        return relay
    }

    private func authorize(_ request: inout URLRequest) async throws {
        if let token = try await tokenProvider(), !token.isEmpty {
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        #if DEBUG
        if request.value(forHTTPHeaderField: "Authorization") == nil,
           let devToken = ProcessInfo.processInfo.environment["ALLTERNIT_DEV_API_TOKEN"] {
            request.setValue("Bearer \(devToken)", forHTTPHeaderField: "Authorization")
        }
        #endif
    }

    // MARK: Response classification

    private enum RelayCondition {
        case ok
        case warming(TimeInterval)
        case offline
    }

    /// Classifies a relay answer: cloud-api's own 503 bodies are
    /// `{"error":"runtime_warming","retryAfterSeconds":N}` and
    /// `{"error":"runtime_offline"}` (runtime_relay.rs
    /// relay_request_to_runtime_with); anything else is the runtime's.
    private static func relayCondition(response: URLResponse, data: Data) -> RelayCondition {
        guard (response as? HTTPURLResponse)?.statusCode == 503,
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let error = object["error"] as? String else {
            return .ok
        }
        switch error {
        case "runtime_warming":
            let suggested = (object["retryAfterSeconds"] as? Double) ?? 3
            return .warming(min(max(suggested, 1), 5))
        case "runtime_offline", "runtime_unavailable":
            return .offline
        default:
            return .ok
        }
    }

    private static func validate(_ response: URLResponse, data: Data) throws {
        guard let http = response as? HTTPURLResponse else { throw APIError.invalidResponse }
        guard (200...299).contains(http.statusCode) else {
            throw APIError.httpError(statusCode: http.statusCode, message: errorMessage(from: data))
        }
    }

    private static func errorMessage(from data: Data) -> String? {
        guard let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return nil }
        return (object["message"] as? String) ?? (object["error"] as? String)
    }

    private static func escapePathSegment(_ value: String) -> String {
        let allowed = CharacterSet.alphanumerics.union(CharacterSet(charactersIn: "-_.~"))
        return value.addingPercentEncoding(withAllowedCharacters: allowed) ?? value
    }
}

// MARK: - Active computer

/// Which of the user's computers runtime calls relay through, plus the
/// availability the "Connect a computer" state reads. Port of the PWA's
/// `pickActiveRuntime` (lib/runtime-target.ts): the account's default
/// computer wins whenever it answers, then the persisted pick, then the
/// first online one (preferring rows holding a live relay connection), then
/// a sleeping cloud computer (the first request wakes it).
@MainActor
final class RuntimeRelayStore: ObservableObject {
    static let shared = RuntimeRelayStore()

    enum Availability: Equatable {
        /// Not fetched yet.
        case unknown
        /// A computer is online (or a sleeping cloud computer will wake).
        case available
        /// The account has computers but none answers.
        case unreachable
        /// No computer is paired at all.
        case none
    }

    @Published private(set) var devices: [RelayRuntimeDevice] = []
    @Published private(set) var isLoading = false
    @Published private(set) var lastError: String?
    @Published private(set) var isWaking = false
    /// Bumped whenever the relay marks a runtime unreachable/reachable, so
    /// views re-read `availability`.
    @Published private(set) var reachabilityRevision = 0

    /// The user's explicit pick (persisted). Shares the PWA's localStorage
    /// key name; nil = automatic.
    @Published private(set) var selectedRuntimeId: String? {
        didSet { defaults.set(selectedRuntimeId, forKey: Keys.selected) }
    }

    private let client: RuntimeRelayClient
    private let defaults: UserDefaults
    private var lastRefreshed: Date?
    private var unreachableAt: [String: Date] = [:]
    private var inFlight: Task<Void, Never>?

    private static let unreachableTTL: TimeInterval = 120
    private static let staleAfter: TimeInterval = 30

    private enum Keys {
        static let selected = "allternit.relay-runtime-id"
    }

    init(client: RuntimeRelayClient = .shared, defaults: UserDefaults = .standard) {
        self.client = client
        self.defaults = defaults
        self.selectedRuntimeId = defaults.string(forKey: Keys.selected)
    }

    // MARK: Selection

    /// The runtime relayed calls go to right now, nil when none qualifies.
    var activeRuntimeId: String? { pick(now: Date()) }

    var activeDevice: RelayRuntimeDevice? {
        guard let id = activeRuntimeId else { return nil }
        return devices.first { $0.id == id }
    }

    var availability: Availability {
        _ = reachabilityRevision
        if lastRefreshed == nil && devices.isEmpty { return lastError == nil ? .unknown : .none }
        if devices.isEmpty { return .none }
        guard let id = activeRuntimeId, let device = devices.first(where: { $0.id == id }) else {
            return .unreachable
        }
        if device.isWakeable && !device.isOnline { return .available }
        return recentlyUnreachable(id, now: Date()) ? .unreachable : .available
    }

    /// Pins relayed calls to one computer; nil returns to automatic.
    func select(_ runtimeId: String?) {
        selectedRuntimeId = runtimeId
        if let runtimeId { unreachableAt[runtimeId] = nil }
        reachabilityRevision += 1
    }

    /// Refreshes when the list is older than 30 s, then answers the active
    /// runtime id. What every relayed call awaits.
    func resolveActiveRuntimeId() async -> String? {
        if let lastRefreshed, Date().timeIntervalSince(lastRefreshed) < Self.staleAfter {
            return activeRuntimeId
        }
        await refresh()
        return activeRuntimeId
    }

    /// Refetches the device list; concurrent callers share one request.
    func refresh() async {
        if let inFlight {
            await inFlight.value
            return
        }
        let task = Task { [weak self] in
            guard let self else { return }
            self.isLoading = true
            defer { self.isLoading = false }
            do {
                self.devices = try await self.client.fetchDevices()
                self.lastError = nil
                self.lastRefreshed = Date()
            } catch is CancellationError {
                // Keep the current list.
            } catch {
                self.lastError = error.localizedDescription
                self.lastRefreshed = Date()
            }
        }
        inFlight = task
        await task.value
        inFlight = nil
    }

    // MARK: Relay feedback

    func markUnreachable(_ runtimeId: String) {
        unreachableAt[runtimeId] = Date()
        reachabilityRevision += 1
    }

    func markReachable(_ runtimeId: String) {
        guard unreachableAt[runtimeId] != nil else { return }
        unreachableAt[runtimeId] = nil
        reachabilityRevision += 1
    }

    func setWaking(_ waking: Bool) {
        if isWaking != waking { isWaking = waking }
    }

    // MARK: Private

    private func recentlyUnreachable(_ id: String, now: Date) -> Bool {
        guard let at = unreachableAt[id] else { return false }
        return now.timeIntervalSince(at) < Self.unreachableTTL
    }

    /// `pickActiveRuntime` (lib/runtime-target.ts).
    private func pick(now: Date) -> String? {
        let selected = selectedRuntimeId ?? EnvironmentStore.shared.pairedRuntimeId
        let heartbeating = devices.filter(\.isOnline)
        let answering = heartbeating.filter { !recentlyUnreachable($0.id, now: now) }
        let online = answering.isEmpty ? heartbeating : answering
        let usable = online.contains(where: \.hasRelayConnection) ? online.filter(\.hasRelayConnection) : online
        if let preferred = usable.first(where: \.isPreferred) { return preferred.id }
        if let selected, usable.contains(where: { $0.id == selected }) { return selected }
        if let first = usable.first { return first.id }
        let wakeable = devices.filter(\.isWakeable)
        if let preferred = wakeable.first(where: \.isPreferred) { return preferred.id }
        if let selected, wakeable.contains(where: { $0.id == selected }) { return selected }
        return wakeable.first?.id
    }
}
