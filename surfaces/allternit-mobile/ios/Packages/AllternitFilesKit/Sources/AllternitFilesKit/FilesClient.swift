import Foundation

/// Settings shared by every `FilesClient`.
public struct FilesClientConfiguration: Sendable {
    /// cloud-api origin without `/api/v1`.
    public var cloudBaseURL: URL
    /// Bearer for cloud-api (endpoints, wake consent, relay). The extension
    /// reads the Files token from `FilesSharedStore`.
    public var tokenProvider: @Sendable () async throws -> String?
    public var deviceId: String
    /// Names conflict copies (`device_name`) and `X-Allternit-Device-Name`.
    public var deviceName: String
    public var devicePlatform: String
    /// Transports this client may use. The File Provider extension passes
    /// `[.relay]` (spike 5); the app may add `.mesh` / `.loopback` later.
    public var allowedTransports: Set<TransportKind>
    /// Control calls.
    public var http: FilesHTTP
    /// Bulk bytes (chunk PUTs, content downloads). The extension passes a
    /// `BackgroundFilesHTTP`.
    public var transferHTTP: FilesHTTP
    /// Envelopes, upload journals and partial downloads.
    public var scratchDirectory: URL
    /// Wake tickets shared across processes (the UI sheet gets one, the
    /// extension uses it). Nil keeps tickets in memory only.
    public var sharedStore: FilesSharedStore?
    /// Upload sessions resume when the same file is sent again.
    public var requestTimeout: TimeInterval

    public init(cloudBaseURL: URL,
                tokenProvider: @escaping @Sendable () async throws -> String?,
                deviceId: String,
                deviceName: String,
                devicePlatform: String = "ios",
                allowedTransports: Set<TransportKind> = [.relay],
                http: FilesHTTP = URLSessionFilesHTTP(),
                transferHTTP: FilesHTTP? = nil,
                scratchDirectory: URL,
                sharedStore: FilesSharedStore? = nil,
                requestTimeout: TimeInterval = 60) {
        self.cloudBaseURL = cloudBaseURL
        self.tokenProvider = tokenProvider
        self.deviceId = deviceId
        self.deviceName = deviceName
        self.devicePlatform = devicePlatform
        self.allowedTransports = allowedTransports
        self.http = http
        self.transferHTTP = transferHTTP ?? http
        self.scratchDirectory = scratchDirectory
        self.sharedStore = sharedStore
        self.requestTimeout = requestTimeout
    }
}

/// When a content request needs the computer awake.
public enum FilesWakePolicy: Sendable {
    /// Throw `wakeConsentRequired`; the caller shows the prompt.
    case never
    /// Wake without asking when the computer's setting is `auto` (SPEC
    /// section 9); throw for `ask` and `never`.
    case whenAutomatic
}

/// The `files/2` client for one computer. Thread-safe; many calls may run at
/// once. Every route in SPEC section 2 that the iOS surfaces need is here;
/// anything else goes through `send(_:)`.
public final class FilesClient: @unchecked Sendable {
    public let computerId: ComputerId
    public let configuration: FilesClientConfiguration

    private let lock = NSLock()
    private var cachedEndpoints: (EndpointsResponse, Date)?
    private var memoryTicket: (String, Date)?

    private static let endpointsTTL: TimeInterval = 300

    /// `computerId` is the id from SPEC "Computer ids" (iOS: the runtime
    /// device id cloud-api lists).
    public init(computerId: ComputerId, configuration: FilesClientConfiguration) {
        self.computerId = computerId
        self.configuration = configuration
    }

    // MARK: Endpoints (cloud-api)

    /// `GET /api/v1/computer-files/:id/endpoints`, cached 5 minutes.
    public func endpoints(refresh: Bool = false) async throws -> EndpointsResponse {
        if !refresh, let cached = lock.withLock({ cachedEndpoints }), Date().timeIntervalSince(cached.1) < Self.endpointsTTL {
            return cached.0
        }
        let url = cloudURL("api/v1/computer-files/\(Self.escape(computerId))/endpoints")
        var request = URLRequest(url: url)
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        request.timeoutInterval = configuration.requestTimeout
        let (data, response) = try await cloud(request)
        let decoded: EndpointsResponse = try decode(data, status: response.statusCode)
        lock.withLock { cachedEndpoints = (decoded, Date()) }
        return decoded
    }

    /// The transport to use now: the allowed candidate with the lowest
    /// `priority` (SPEC section 10).
    public func transport() async throws -> FilesTransport {
        let endpoints = try await endpoints()
        let allowed = endpoints.candidates
            .filter { configuration.allowedTransports.contains($0.kind) && $0.kind != .gateway }
            .sorted { $0.priority < $1.priority }
        guard let pick = allowed.first, let url = URL(string: pick.baseUrl) else {
            throw FilesError.noTransport(available: endpoints.candidates.map(\.kind))
        }
        if pick.kind == .relay { return .relay(proxyURL: url) }
        // A mesh address goes through the Desktop app's loopback proxy when
        // it runs one (macOS); otherwise straight to the address, which works
        // where the system itself is on the mesh.
        if pick.kind == .mesh, let proxied = configuration.sharedStore?.meshProxyURL(for: url) {
            return .direct(baseURL: proxied)
        }
        return .direct(baseURL: url)
    }

    // MARK: Wake consent (cloud-api)

    /// `POST <consent_path>` (default `/api/v1/computer-files/:id/wake-consent`).
    /// Call it only after the user agreed (setting `ask`) or when the setting
    /// is `auto`. The ticket is kept for later content requests and shared
    /// with the other processes through `FilesSharedStore`.
    @discardableResult
    public func requestWake(reason: WakeReason, itemId: ItemId? = nil, hint: WakeHint? = nil) async throws -> WakeConsentResponse {
        let path = hint?.consentPath ?? "/api/v1/computer-files/\(Self.escape(computerId))/wake-consent"
        var request = URLRequest(url: cloudURL(String(path.drop(while: { $0 == "/" }))))
        request.httpMethod = "POST"
        request.timeoutInterval = configuration.requestTimeout
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        request.httpBody = try JSONEncoder().encode(WakeConsentRequest(reason: reason, itemId: itemId, deviceId: configuration.deviceId))
        let (data, response) = try await cloud(request)
        let consent: WakeConsentResponse = try decode(data, status: response.statusCode)
        let expiry = Self.parseDate(consent.expiresAt) ?? Date().addingTimeInterval(110)
        lock.withLock { memoryTicket = (consent.ticket, expiry) }
        configuration.sharedStore?.setWakeTicket(consent.ticket, expiresAt: expiry, computerId: computerId)
        configuration.sharedStore?.clearPendingWake(computerId: computerId)
        lock.withLock { cachedEndpoints = nil }
        return consent
    }

    /// A valid wake ticket from this process or another one, if any.
    public func currentWakeTicket() -> String? {
        if let (ticket, expiry) = lock.withLock({ memoryTicket }), expiry.timeIntervalSinceNow > 5 { return ticket }
        return configuration.sharedStore?.wakeTicket(computerId: computerId)
    }

    // MARK: Metadata (answered from the asleep snapshot when asleep)

    public func ping() async throws -> PingResponse {
        try await json(FilesRequest(path: "/ping"))
    }

    /// `GET /roots`.
    public func roots() async throws -> RootsResponse {
        try await json(FilesRequest(path: "/roots"))
    }

    /// `GET /roots/:root_id/list`. Follow `nextPageToken` for more.
    public func list(rootId: RootId, parentId: ItemId, pageToken: PageToken? = nil, limit: Int? = nil) async throws -> ListResponse {
        var query = [URLQueryItem(name: "parent_id", value: parentId)]
        if let pageToken { query.append(URLQueryItem(name: "page_token", value: pageToken)) }
        if let limit { query.append(URLQueryItem(name: "limit", value: String(limit))) }
        return try await json(FilesRequest(path: "/roots/\(Self.escape(rootId))/list", query: query))
    }

    /// `GET /items/:item_id`.
    public func stat(_ itemId: ItemId) async throws -> StatResponse {
        try await json(FilesRequest(path: "/items/\(Self.escape(itemId))"))
    }

    // MARK: Changes (poll)

    /// `GET /roots/:root_id/changes`. `waitS` (at most 25) long-polls. A
    /// cursor that is too old, or a `410 cursor_expired`, comes back as
    /// `.reset`: list the root again and continue from its new cursor.
    public func changes(rootId: RootId, cursor: Cursor, limit: Int = 500, waitS: Int = 0) async throws -> FilesChangeBatch {
        var query = [URLQueryItem(name: "cursor", value: cursor), URLQueryItem(name: "limit", value: String(limit))]
        if waitS > 0 { query.append(URLQueryItem(name: "wait_s", value: String(min(waitS, 25)))) }
        do {
            let response: ChangesResponse = try await json(
                FilesRequest(path: "/roots/\(Self.escape(rootId))/changes", query: query),
                timeout: TimeInterval(waitS) + configuration.requestTimeout
            )
            return response.resetRequired ? .reset(freshness: response.freshness) : .changes(response)
        } catch let error as FilesError where error.code == .cursorExpired {
            return .reset(freshness: nil)
        }
    }

    /// Polls one root forever (long poll, `wait_s` 25). While the computer is
    /// asleep or unreachable it backs off 60 s and keeps the cursor; it never
    /// wakes the computer. Cancel the task to stop.
    public func changeStream(rootId: RootId, from cursor: Cursor) -> AsyncThrowingStream<FilesChangeBatch, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                var cursor = cursor
                while !Task.isCancelled {
                    do {
                        let batch = try await changes(rootId: rootId, cursor: cursor, waitS: 25)
                        if case .changes(let response) = batch { cursor = response.cursor }
                        continuation.yield(batch)
                        if case .reset = batch { break }
                    } catch let error as FilesError where error.isComputerUnavailable {
                        try? await Task.sleep(nanoseconds: 60_000_000_000)
                    } catch {
                        continuation.finish(throwing: error)
                        return
                    }
                }
                continuation.finish()
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    // MARK: Mutations (base_rev guarded)

    /// `POST /mkdir`.
    public func mkdir(parentId: ItemId, name: String, onConflict: OnConflict = .keepBoth) async throws -> MutationResponse {
        try await json(post("/mkdir", MkdirRequest(parentId: parentId, name: name, onConflict: onConflict)), wake: .never)
    }

    /// `POST /move` (rename and/or move). A stale `baseRev` is always
    /// `rev_mismatch` (SPEC section 4).
    public func move(_ move: MoveRequest) async throws -> MutationResponse {
        try await json(post("/move", move), wake: .never)
    }

    /// `POST /delete` (to trash unless `permanent`).
    public func delete(_ delete: DeleteRequest) async throws -> DeleteResponse {
        try await json(post("/delete", delete), wake: .never)
    }

    /// `POST /pin` for this device, and records the pin in `FilesSharedStore`.
    public func pin(itemId: ItemId, pinned: Bool) async throws -> PinResponse {
        let response: PinResponse = try await json(post("/pin", PinRequest(itemId: itemId, deviceId: configuration.deviceId, pinned: pinned)), wake: .never)
        configuration.sharedStore?.setPinned(pinned, computerId: computerId, itemId: itemId)
        return response
    }

    // MARK: Generic

    /// Sends any `files/2` request and decodes the JSON answer. Content
    /// routes get the wake ticket header; `wake` decides whether a missing
    /// consent is asked for silently (setting `auto`) or thrown.
    public func json<T: Decodable>(_ request: FilesRequest, wake: FilesWakePolicy = .never, timeout: TimeInterval? = nil) async throws -> T {
        let (data, status) = try await withWake(policy: wake, reason: .openFile, itemId: nil) {
            let (data, response) = try await self.send(request, http: self.configuration.http, timeout: timeout)
            guard (200..<300).contains(response.statusCode) else {
                throw ErrorDecoding.error(status: response.statusCode, data: data)
            }
            return (data, response.statusCode)
        }
        return try decode(data, status: status)
    }

    /// One raw round trip through the current transport. Non-2xx answers are
    /// returned, not thrown.
    public func send(_ request: FilesRequest, http: FilesHTTP? = nil, timeout: TimeInterval? = nil) async throws -> (Data, HTTPURLResponse) {
        let prepared = try await prepare(request, timeout: timeout)
        defer { if let file = prepared.bodyFile { try? FileManager.default.removeItem(at: file) } }
        let client = http ?? configuration.http
        do {
            if let file = prepared.bodyFile {
                return try await client.upload(for: prepared.request, fromFile: file)
            }
            return try await client.data(for: prepared.request)
        } catch let error as URLError where error.code != .cancelled {
            lock.withLock { cachedEndpoints = nil }
            throw error
        }
    }

    // MARK: Internals

    func prepare(_ request: FilesRequest, timeout: TimeInterval? = nil) async throws -> FilesTransport.Prepared {
        guard let token = try await configuration.tokenProvider(), !token.isEmpty else { throw FilesError.signedOut }
        var request = request
        request.headers["Accept"] = request.headers["Accept"] ?? "application/json"
        request.headers["X-Allternit-Device-Id"] = configuration.deviceId
        request.headers["X-Allternit-Device-Name"] = configuration.deviceName
        request.headers["X-Allternit-Device-Platform"] = configuration.devicePlatform
        if let ticket = currentWakeTicket() { request.headers["X-Allternit-Wake-Ticket"] = ticket }
        let transport = try await transport()
        return try transport.prepare(request, bearer: token, scratch: configuration.scratchDirectory,
                                     timeout: timeout ?? configuration.requestTimeout)
    }

    /// Runs `operation`; on `wake_consent_required` with setting `auto` and
    /// policy `.whenAutomatic`, asks for a ticket and runs it once more. On
    /// `waking` (a ticket is waking the computer) it waits and retries for up
    /// to 60 s. Otherwise it records a pending wake for the UI and rethrows.
    func withWake<T>(policy: FilesWakePolicy, reason: WakeReason, itemId: ItemId?, _ operation: () async throws -> T) async throws -> T {
        var woke = false
        let deadline = Date().addingTimeInterval(60)
        while true {
            do {
                return try await operation()
            } catch FilesError.wakeConsentRequired(let hint, let state) {
                if case .whenAutomatic = policy, hint?.setting == .auto, !woke {
                    try await requestWake(reason: reason, itemId: itemId, hint: hint)
                    woke = true
                    continue
                }
                configuration.sharedStore?.addPendingWake(FilesPendingWake(computerId: computerId, itemId: itemId, reason: reason, hint: hint))
                throw FilesError.wakeConsentRequired(hint: hint, state: state)
            } catch let error as FilesError {
                let retryAfter: TimeInterval?
                switch error {
                case .warming(let after): retryAfter = after
                case .api(_, let body) where body.code == .waking: retryAfter = TimeInterval(body.retryAfterS ?? 3)
                default: retryAfter = nil
                }
                guard let retryAfter, currentWakeTicket() != nil, Date().addingTimeInterval(retryAfter) < deadline else { throw error }
                try await Task.sleep(nanoseconds: UInt64(min(max(retryAfter, 1), 5) * 1_000_000_000))
            }
        }
    }

    private func post<B: Encodable>(_ path: String, _ body: B) throws -> FilesRequest {
        FilesRequest(method: "POST", path: path, headers: ["Content-Type": "application/json"], body: .json(try JSONEncoder().encode(body)))
    }

    private func cloud(_ request: URLRequest) async throws -> (Data, HTTPURLResponse) {
        guard let token = try await configuration.tokenProvider(), !token.isEmpty else { throw FilesError.signedOut }
        var request = request
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        request.setValue(configuration.deviceId, forHTTPHeaderField: "X-Allternit-Device-Id")
        return try await configuration.http.data(for: request)
    }

    func decode<T: Decodable>(_ data: Data, status: Int) throws -> T {
        guard (200..<300).contains(status) else { throw ErrorDecoding.error(status: status, data: data) }
        do {
            return try JSONDecoder().decode(T.self, from: data)
        } catch {
            throw FilesError.invalidResponse("\(T.self): \(error)")
        }
    }

    /// `path` is already percent-encoded, so it is joined as text
    /// (`appendingPathComponent` would encode the `%` again).
    private func cloudURL(_ path: String) -> URL {
        var base = configuration.cloudBaseURL.absoluteString
        while base.hasSuffix("/") { base.removeLast() }
        return URL(string: base + "/" + path) ?? configuration.cloudBaseURL
    }

    static func escape(_ segment: String) -> String {
        var allowed = CharacterSet.alphanumerics
        allowed.insert(charactersIn: "-._~")
        return segment.addingPercentEncoding(withAllowedCharacters: allowed) ?? segment
    }

    static func parseDate(_ value: String) -> Date? {
        let fractional = ISO8601DateFormatter()
        fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return fractional.date(from: value) ?? ISO8601DateFormatter().date(from: value)
    }
}

/// One answer from the change feed.
public enum FilesChangeBatch: Sendable {
    case changes(ChangesResponse)
    /// The cursor is no longer usable: list again from scratch.
    case reset(freshness: Freshness?)
}
