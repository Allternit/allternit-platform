import Foundation

/// Typed errors surfaced by APIClient and AgentChatClient.
enum APIError: Error, LocalizedError, Sendable {
    case invalidResponse
    case httpError(statusCode: Int, message: String?)
    case decoding(Error)
    case transport(Error)

    var errorDescription: String? {
        switch self {
        case .invalidResponse:
            return "The server returned an invalid response."
        case .httpError(let statusCode, let message):
            return message ?? "Request failed with status \(statusCode)."
        case .decoding(let error):
            return "Failed to decode the server response: \(error.localizedDescription)"
        case .transport(let error):
            return error.localizedDescription
        }
    }
}

/// Per-request routing for runtime API calls, resolved from EnvironmentStore
/// on every send so switching environments takes effect immediately.
enum APIRoute: Sendable {
    /// Straight to the configured gateway (local "This Device").
    case direct
    /// Cloud relay: wrap the request in the proxy envelope and POST it to the
    /// paired runtime's proxy URL (fetch-interceptor.ts:441-468).
    case relay(proxyURL: URL)
    /// Cloud selected with no paired runtime — mirrors the web interceptor's
    /// 503 `runtime_unavailable` response (fetch-interceptor.ts:470-478).
    case unavailable(reason: String)
}

/// Thin async/await REST client for the Allternit gateway.
///
/// The request builder *awaits* the runtime device token (falling back to the
/// Clerk Bearer token) before returning the request, so the Authorization
/// header is always attached before the request leaves the client — the v1
/// race (header applied inside a detached Task after the request had already
/// been returned) is gone by construction.
///
/// JSON is decoded with a plain `JSONDecoder`: models carry explicit
/// `CodingKeys` wherever the wire casing differs (snake_case conversations,
/// camelCase replies), so no key-decoding strategy is applied globally.
final class APIClient: @unchecked Sendable {
    static let shared = APIClient(
        tokenProvider: { try await AuthManager.shared.apiToken() },
        routeProvider: { await EnvironmentStore.shared.apiRoute() }
    )

    let baseURL: URL
    let session: URLSession

    /// Awaits the current runtime device token, falling back to the Clerk
    /// session token; nil when signed out.
    private let tokenProvider: @Sendable () async throws -> String?

    /// Resolves how the next request reaches the runtime. Called per send,
    /// never captured — environment switches apply to the very next request.
    private let routeProvider: @Sendable () async -> APIRoute

    /// All stored state is immutable after init — safe to share across tasks.
    init(baseURL: URL = AppConfig.apiBaseURL,
         session: URLSession = .shared,
         tokenProvider: @escaping @Sendable () async throws -> String?,
         routeProvider: (@Sendable () async -> APIRoute)? = nil) {
        self.baseURL = baseURL
        self.session = session
        self.tokenProvider = tokenProvider
        self.routeProvider = routeProvider ?? { .direct }
    }

    // MARK: - Request building

    /// Builds a JSON request, awaiting and attaching the Bearer token first.
    ///
    /// A `?query` suffix on `path` becomes the URL's query (callers
    /// percent-encode query values themselves) — `appendingPathComponent`
    /// alone encodes the "?" into the path on iOS 16+, which would route
    /// the request to a nonexistent resource.
    func authorizedRequest(path: String, method: String = "GET") async throws -> URLRequest {
        var url = baseURL.appendingPathComponent(path)
        if let split = path.firstIndex(of: "?"),
           var components = URLComponents(
               url: baseURL.appendingPathComponent(String(path[..<split])),
               resolvingAgainstBaseURL: false
           ) {
            components.percentEncodedQuery = String(path[path.index(after: split)...])
            if let rebuilt = components.url {
                url = rebuilt
            }
        }
        return try await authorizedRequest(url: url, method: method)
    }

    /// Builds a JSON request for an absolute URL outside `baseURL`, with the
    /// same awaited Bearer injection. Needed for `POST /api/agent-chat`, which
    /// is mounted directly under `/api` on allternit-api — NOT under the
    /// `/api/v1` router — so it can't be reached by appending to `baseURL`.
    func authorizedRequest(url: URL, method: String = "GET") async throws -> URLRequest {
        var request = URLRequest(url: url)
        request.httpMethod = method
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue("application/json", forHTTPHeaderField: "Accept")

        let token = try await tokenProvider()
        if let token, !token.isEmpty {
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        #if DEBUG
        // `-skip-auth` test mode: authenticate via the gateway's desktop
        // bootstrap path (auth.rs extract_desktop_bootstrap_user — user-id +
        // any desktop token). DEBUG-only shim for local UI testing; real
        // clients send only the Clerk Bearer. The dev Bearer additionally
        // satisfies allternit-cloud-api's dev shortcut
        // (auth/middleware.rs + dispatch handoff) when `-cloud-url` points
        // at a locally running cloud-api.
        if launchArgumentEnabled("skip-auth") {
            request.setValue("dev-ios-tester", forHTTPHeaderField: "x-allternit-user-id")
            request.setValue("dev", forHTTPHeaderField: "x-allternit-desktop-access-token")
            // The dev bearer is no longer hardcoded (B1): provide it via the
            // ALLTERNIT_DEV_API_TOKEN environment variable in your scheme when
            // pointing `-cloud-url` at a cloud-api with the fallback enabled.
            if request.value(forHTTPHeaderField: "Authorization") == nil,
               let devToken = ProcessInfo.processInfo.environment["ALLTERNIT_DEV_API_TOKEN"] {
                request.setValue("Bearer \(devToken)", forHTTPHeaderField: "Authorization")
            }
        }
        #endif
        return request
    }

    // MARK: - Response validation

    /// Throws a typed APIError for non-2xx responses, surfacing the gateway's
    /// `{ "message" | "error" }` body when present (same shape the web reads).
    func validate(_ response: URLResponse, data: Data? = nil) throws {
        guard let httpResponse = response as? HTTPURLResponse else {
            throw APIError.invalidResponse
        }
        guard (200...299).contains(httpResponse.statusCode) else {
            #if DEBUG
            let snippet = data.flatMap { String(data: $0.prefix(200), encoding: .utf8) } ?? ""
            NSLog("[APIClient] %d %@ %@", httpResponse.statusCode, httpResponse.url?.absoluteString ?? "-", snippet)
            #endif
            throw APIError.httpError(
                statusCode: httpResponse.statusCode,
                message: Self.errorMessage(from: data)
            )
        }
    }

    private static func errorMessage(from data: Data?) -> String? {
        guard let data,
              let body = try? JSONDecoder().decode(ErrorBody.self, from: data) else {
            return nil
        }
        return body.message ?? body.error
    }

    private struct ErrorBody: Decodable {
        let message: String?
        let error: String?
    }

    // MARK: - Standard REST requests

    /// Sends a request through the current API route. In `.relay` mode the
    /// original URL's host is irrelevant — only its path+query travels inside
    /// the proxy envelope, exactly like the web interceptor (which builds
    /// same-origin `/api/*` URLs and forwards `runtimePath(url)`).
    func send(_ request: URLRequest) async throws -> (Data, URLResponse) {
        // `-sample-workspace` (DEBUG): SampleURLProtocol answers every call,
        // whatever the route or computer.
        if SampleURLProtocol.handles(request.url) {
            return try await session.data(for: request)
        }
        // A client built on an `allternit-relay://<runtimeId>` placeholder
        // (InstanceConnection.resolve → the user's computer via the cloud
        // relay) always goes through RuntimeRelayClient.
        if let runtimeId = RuntimeRelayClient.runtimeId(in: request.url) {
            return try await RuntimeRelayClient.shared.send(request, runtimeId: runtimeId)
        }
        switch await routeProvider() {
        case .direct:
            return try await session.data(for: request)
        case .unavailable(let reason):
            throw APIError.httpError(statusCode: 503, message: reason)
        case .relay(let proxyURL):
            return try await session.data(for: try await relayRequest(for: request, proxyURL: proxyURL))
        }
    }

    /// Streaming variant of `send(_:)` (agent-chat SSE). The relay returns the
    /// runtime's status/headers/streaming body untouched, so SSE backpressure
    /// is preserved end-to-end (fetch-interceptor.ts:465-467).
    func sendStream(_ request: URLRequest) async throws -> (URLSession.AsyncBytes, URLResponse) {
        if SampleURLProtocol.handles(request.url) {
            return try await session.bytes(for: request)
        }
        if let runtimeId = RuntimeRelayClient.runtimeId(in: request.url) {
            return try await RuntimeRelayClient.shared.stream(request, runtimeId: runtimeId)
        }
        switch await routeProvider() {
        case .direct:
            return try await session.bytes(for: request)
        case .unavailable(let reason):
            throw APIError.httpError(statusCode: 503, message: reason)
        case .relay(let proxyURL):
            return try await session.bytes(for: try await relayRequest(for: request, proxyURL: proxyURL))
        }
    }

    func get<T: Decodable>(path: String) async throws -> T {
        let request = try await authorizedRequest(path: path)
        let (data, response) = try await send(request)
        try validate(response, data: data)
        do {
            return try JSONDecoder().decode(T.self, from: data)
        } catch {
            throw APIError.decoding(error)
        }
    }

    @discardableResult
    func post<T: Decodable, B: Encodable>(path: String, body: B) async throws -> T {
        var request = try await authorizedRequest(path: path, method: "POST")
        request.httpBody = try JSONEncoder().encode(body)

        let (data, response) = try await send(request)
        try validate(response, data: data)
        do {
            return try JSONDecoder().decode(T.self, from: data)
        } catch {
            throw APIError.decoding(error)
        }
    }

    /// POST with a JSON body and no decoded response payload.
    func post<B: Encodable>(path: String, body: B) async throws {
        var request = try await authorizedRequest(path: path, method: "POST")
        request.httpBody = try JSONEncoder().encode(body)
        let (data, response) = try await send(request)
        try validate(response, data: data)
    }

    /// POST with no body and no response payload (e.g. reply cancel).
    func post(path: String) async throws {
        let request = try await authorizedRequest(path: path, method: "POST")
        let (data, response) = try await send(request)
        try validate(response, data: data)
    }

    /// PUT with a JSON body and no decoded payload (e.g. project rename /
    /// instructions save — the cowork routes answer `{"success": true}`).
    func put<B: Encodable>(path: String, body: B) async throws {
        var request = try await authorizedRequest(path: path, method: "PUT")
        request.httpBody = try JSONEncoder().encode(body)
        let (data, response) = try await send(request)
        try validate(response, data: data)
    }

    /// PATCH with a JSON body and no decoded payload (e.g. session archive —
    /// the agent-session routes answer with the updated session, which bulk
    /// callers don't need).
    func patch<B: Encodable>(path: String, body: B) async throws {
        var request = try await authorizedRequest(path: path, method: "PATCH")
        request.httpBody = try JSONEncoder().encode(body)

        let (data, response) = try await send(request)
        try validate(response, data: data)
    }

    /// DELETE with no request/response body (e.g. connector disconnect).
    func delete(path: String) async throws {
        let request = try await authorizedRequest(path: path, method: "DELETE")
        let (data, response) = try await send(request)
        try validate(response, data: data)
    }

    // MARK: - Cloud relay envelope

    /// Body of the relay POST — `{ method, path, headers, body, bodyEncoding }`
    /// (fetch-interceptor.ts:454-463). Key casing matches the web exactly
    /// (`bodyEncoding` is camelCase on the wire).
    private struct RelayEnvelope: Encodable {
        let method: String
        let path: String
        let headers: [String: String]
        let body: String
        let bodyEncoding: String
    }

    /// The only headers the web interceptor forwards into the envelope
    /// (fetch-interceptor.ts:446-449).
    private static let relayForwardedHeaders: Set<String> = [
        "accept", "content-type", "if-none-match", "if-modified-since",
        "last-event-id", "x-request-id",
        // D16: a person's own act (plan approval); cloud-api forwards it.
        "x-allternit-human-proof",
    ]

    /// Builds the cloud-relay request for an outgoing runtime call
    /// (fetch-interceptor.ts:441-468): POST `{cloud}/api/v1/runtime-devices/
    /// <id>/proxy` with the envelope above, `Content-Type: application/json`
    /// and the Clerk Bearer. The response IS the runtime's response.
    private func relayRequest(for request: URLRequest, proxyURL: URL) async throws -> URLRequest {
        let method = (request.httpMethod ?? "GET").uppercased()

        // `runtimePath()` — pathname + search of the original URL.
        var path = request.url?.path ?? "/"
        if let query = request.url?.query, !query.isEmpty {
            path += "?\(query)"
        }

        var forwarded: [String: String] = [:]
        for (name, value) in request.allHTTPHeaderFields ?? [:] {
            let lowercased = name.lowercased()
            if Self.relayForwardedHeaders.contains(lowercased) {
                forwarded[lowercased] = value
            }
        }

        // `requestBody()` — string bodies go as utf8, binary as base64,
        // empty as "" + utf8.
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

        var relay = URLRequest(url: proxyURL)
        relay.httpMethod = "POST"
        relay.setValue("application/json", forHTTPHeaderField: "Content-Type")
        // Preserve per-request timeouts (agent-chat streams use 600s).
        relay.timeoutInterval = request.timeoutInterval
        if let token = try await tokenProvider(), !token.isEmpty {
            relay.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        relay.httpBody = try JSONEncoder().encode(
            RelayEnvelope(method: method, path: path, headers: forwarded, body: body, bodyEncoding: bodyEncoding)
        )
        return relay
    }
}
