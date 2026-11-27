import Foundation

/// Typed errors surfaced by APIClient and AgentChatClient.
///
/// Calm errors (phone calm pass, contract C5): `errorDescription` never
/// contains an HTTP status number, a status phrase or a machine code. A
/// server message is kept when it reads as a sentence; otherwise the status
/// gets a plain sentence (`statusSentence`). Mirrors allternit-ai
/// `src/lib/errors/plain-error.ts`.
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
            if isComputeGateRefusal { return CalmCopy.computeRequired }
            return plainErrorText(message: message, status: statusCode)
        case .decoding:
            return "The server's answer couldn't be read. Try again."
        case .transport(let error):
            return plainErrorText(error)
        }
    }

    /// The compute gate's account-state refusals: 409 `compute_required`,
    /// 410 `free_computers_retired`, and cloud-api's 428 "pair a device"
    /// (data_plane_fallback.rs). On a read these show no error: the list shows
    /// its empty state and the no-computer chip explains (contract C5).
    /// Mirrors web `isComputeGateRefusal`.
    var isComputeGateRefusal: Bool {
        switch self {
        case .httpError(let status, let message):
            if status == 428 { return true }
            guard status == 409 || status == 410, let message else { return false }
            // `validate` sets the gate sentence for a gate body; other
            // builders may still carry the raw code in their message.
            return message == CalmCopy.computeRequired
                || message.range(of: #"\b(compute_required|free_computers_retired)\b"#,
                                 options: [.regularExpression, .caseInsensitive]) != nil
        case .transport(let inner):
            return (inner as? APIError)?.isComputeGateRefusal ?? false
        case .invalidResponse, .decoding:
            return false
        }
    }
}

/// Any error, true when it is a compute-gate refusal (see
/// `APIError.isComputeGateRefusal`).
func isComputeGateRefusal(_ error: Error?) -> Bool {
    (error as? APIError)?.isComputeGateRefusal ?? false
}

// MARK: - Calm error text (contract C5)

/// The calm sentences iOS shares with the web phone shell (allternit-ai
/// `PhoneNoComputer.tsx` / `plain-error.ts`), word for word.
enum CalmCopy {
    /// Web `NO_COMPUTER_TEXT`.
    static let noComputer = "No computer connected"
    /// Web `CANT_REACH_TEXT`.
    static let cantReach = "Can't reach your computer"
    /// Web `WAKING_TEXT`.
    static let waking = "Waking your computer…"
    /// Web `CODE_TEXT.compute_required`: what a compute-gate refusal reads as.
    static let computeRequired = "Agents need a computer."
    /// The line under `noComputer` on screens with no composer.
    static let noComputerDetail = "Connect a computer or start Allternit Cloud to use this."
    /// When nothing a person can act on is left.
    static let fallback = "Something went wrong. Try again."
}

/// Machine codes a person can read a sentence for (web `CODE_TEXT`).
private let calmCodeText: [String: String] = [
    "compute_required": CalmCopy.computeRequired,
    "free_computers_retired": CalmCopy.computeRequired,
    "runtime_unavailable": "Can't reach your computer.",
    "runtime_offline": "Can't reach your computer.",
    "runtime_warming": "Your computer is waking up. Try again in a moment.",
    "computer_capacity": "No computers are free right now.",
    // cloud-api's 428 code (error.rs PreconditionRequired): pair a device.
    "precondition_required": CalmCopy.noComputerDetail,
]

private let httpReasonPattern = "(?:Bad Request|Unauthorized|Payment Required|Forbidden|Not Found|Method Not Allowed|Request Timeout|Conflict|Gone|Precondition Failed|Precondition Required|Unprocessable (?:Entity|Content)|Too Many Requests|Internal Server Error|Not Implemented|Bad Gateway|Service Unavailable|Gateway Timeout)"

/// Each layer prefixes its own wrapper; the innermost message says what
/// happened (web `WRAPPER_PREFIXES`).
private let wrapperPrefixPatterns: [String] = [
    #"^invalid error response format:\s*"#,
    #"^gateway request failed:\s*"#,
    #"^error:\s*"#,
    // A leading machine code ("compute_required: …", "RUNTIME_OFFLINE - …").
    #"^[a-z][a-z0-9]*(?:_[a-z0-9]+)+\s*[:\-–—]\s*"#,
]

/// Status numbers and status phrases, wherever they sit (web `STATUS_TEXT`),
/// plus Foundation's "(SomeDomain error -1004.)" tail.
private let statusTextPatterns: [String] = [
    // " (409)", "(HTTP 500)", "(status 404)", "(status code 502)"
    #"\s*\(\s*(?:HTTP\s*|status\s*(?:code\s*)?:?\s*)?[1-5]\d\d\s*\)"#,
    // "with status code 502", "status: 404", "status 500"
    #"\s*\b(?:with\s+)?status(?:\s+code)?\s*:?\s*[1-5]\d\d\b"#,
    // "HTTP 404", "HTTP/1.1 500"
    #"\s*\bHTTP(?:/\d(?:\.\d)?)?\s*[1-5]\d\d\b"#,
    // "500 Internal Server Error", "404 Not Found"
    #"\s*\b[45]\d\d\s+"# + httpReasonPattern + #"\b"#,
    // "Error 500", "code 409", "error code: 428"
    #"\s*\b(?:error|code)(?:\s+code)?\s*:?\s*[45]\d\d\b"#,
    // NSError tails: "(NSURLErrorDomain error -1004.)", "(Allternit.APIError error 1.)"
    #"\s*\([^()]*\berror\s+-?\d+\.?\)"#,
]

/// Only transport words left: nothing a person can act on.
private let genericLeftoverPattern = "^(?:request failed|request error|failed|error|unknown error|an error occurred|http error|server error|network response was not ok|the operation couldn['’]t be completed|" + httpReasonPattern + ")$"

/// A message that is only a machine code, e.g. `compute_required`.
private let bareCodePattern = #"^[a-z][a-z0-9]*(?:_[a-z0-9]+)+$"#

private func calmMatches(_ text: String, _ pattern: String) -> Bool {
    text.range(of: pattern, options: [.regularExpression, .caseInsensitive]) != nil
}

private func calmReplacing(_ text: String, _ pattern: String, with template: String = "") -> String {
    text.replacingOccurrences(of: pattern, with: template, options: [.regularExpression, .caseInsensitive])
}

/// The sentence for an error that carries a status and no usable message.
/// Same sentences as web `statusSentence`.
func statusSentence(_ status: Int?) -> String? {
    guard let status else { return nil }
    switch status {
    case 401: return "Sign in again to continue."
    case 403: return "You don't have access to that."
    case 404: return "That isn't there anymore."
    case 408, 504: return "That took too long. Try again."
    case 428: return "This needs your approval first."
    case 429: return "Too many requests. Wait a moment and try again."
    case 500..<600: return "Something went wrong on our side. Try again."
    default: return nil
    }
}

/// Removes status numbers and status phrases from any text, keeping the
/// rest. Returns "" when nothing readable is left. Safe for non-error text
/// ("Imported 404 files" is left alone). Mirrors web `stripStatusText`.
func stripStatusText(_ text: String?) -> String {
    var out = (text ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
    guard !out.isEmpty else { return "" }
    for pattern in statusTextPatterns { out = calmReplacing(out, pattern) }
    out = calmReplacing(out, #"\s+([.,;:!?])"#, with: "$1")
    out = calmReplacing(out, #"^[\s:;,.\-–—]+"#)
    out = calmReplacing(out, #"[\s:;,\-–—]+$"#)
    out = calmReplacing(out, #"\s{2,}"#, with: " ")
    return out.trimmingCharacters(in: .whitespacesAndNewlines)
}

private func unwrapErrorText(_ text: String) -> String {
    var out = text.trimmingCharacters(in: .whitespacesAndNewlines)
    var changed = true
    while changed {
        changed = false
        for pattern in wrapperPrefixPatterns {
            let next = calmReplacing(out, pattern).trimmingCharacters(in: .whitespacesAndNewlines)
            if next != out, !next.isEmpty {
                out = next
                changed = true
            }
        }
    }
    return out
}

/// A server or transport message made calm, or nil when nothing a person
/// can read is left (only a status, a wrapper or an unknown code).
func calmMessageText(_ raw: String?) -> String? {
    var text = stripStatusText(unwrapErrorText(raw ?? ""))
    if calmMatches(text, "^(invalid error response format|gateway request failed)$") { return "Couldn't reach the model." }
    if calmMatches(text, bareCodePattern) {
        if let known = calmCodeText[text.lowercased()] { return known }
        text = ""
    }
    let bare = calmReplacing(text, #"[.!]+$"#)
    if text.isEmpty || calmMatches(bare, genericLeftoverPattern) { return nil }
    return text
}

/// One calm sentence from a message, an HTTP status and a machine code.
/// Never returns a status number or a code. Mirrors web `plainErrorText`.
func plainErrorText(message: String?, status: Int? = nil, code: String? = nil,
                    fallback: String = CalmCopy.fallback) -> String {
    if let text = calmMessageText(message) { return text }
    if let code, let known = calmCodeText[code.lowercased()] { return known }
    return statusSentence(status) ?? fallback
}

/// One calm sentence for any error. Every screen that shows an error's text
/// goes through this (or through `APIError.errorDescription`, which uses it).
func plainErrorText(_ error: Error?, fallback: String = CalmCopy.fallback) -> String {
    guard let error else { return fallback }
    if let api = error as? APIError {
        if case .httpError(let status, let message) = api {
            if api.isComputeGateRefusal { return CalmCopy.computeRequired }
            return plainErrorText(message: message, status: status, fallback: fallback)
        }
        return api.errorDescription ?? fallback
    }
    if error is CancellationError { return "The request was cancelled." }
    return plainErrorText(message: error.localizedDescription, fallback: fallback)
}

/// A non-2xx JSON body in any of the shapes our servers send: string
/// `error` (cloud-api: `{error, message, code}`), object `error: {code,
/// message}` (the compute gate), top-level `message`, or `detail` (string,
/// or FastAPI's `[{msg}]`).
struct APIErrorBody: Equatable, Sendable {
    var message: String?
    var code: String?

    static func parse(_ data: Data?) -> APIErrorBody {
        guard let data,
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            return APIErrorBody(message: nil, code: nil)
        }
        var message = object["message"] as? String
        var code = object["code"] as? String
        if let nested = object["error"] as? [String: Any] {
            code = (nested["code"] as? String) ?? code
            message = message ?? (nested["message"] as? String)
        } else if let error = object["error"] as? String {
            if calmMatches(error, bareCodePattern) {
                code = code ?? error
            } else {
                message = message ?? error
            }
        }
        if message == nil {
            if let detail = object["detail"] as? String {
                message = detail
            } else if let details = object["detail"] as? [[String: Any]] {
                message = details.first?["msg"] as? String
            }
        }
        return APIErrorBody(message: message, code: code)
    }

    /// 409 `compute_required` / 410 `free_computers_retired`.
    var isComputeGateCode: Bool {
        let lowered = code?.lowercased()
        return lowered == "compute_required" || lowered == "free_computers_retired"
    }

    /// The message `APIError.httpError` carries: calm, or nil.
    var calmMessage: String? {
        if isComputeGateCode { return CalmCopy.computeRequired }
        if let text = calmMessageText(message) { return text }
        if let code, let known = calmCodeText[code.lowercased()] { return known }
        return nil
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

    /// Throws a typed APIError for non-2xx responses, carrying the server's
    /// message made calm (`APIErrorBody.calmMessage`: no status, no code).
    func validate(_ response: URLResponse, data: Data? = nil, method: String? = nil) throws {
        guard let httpResponse = response as? HTTPURLResponse else {
            throw APIError.invalidResponse
        }
        guard (200...299).contains(httpResponse.statusCode) else {
            #if DEBUG
            let snippet = data.flatMap { String(data: $0.prefix(200), encoding: .utf8) } ?? ""
            NSLog("[APIClient] %d %@ %@", httpResponse.statusCode, httpResponse.url?.absoluteString ?? "-", snippet)
            #else
            // Status and path only (no body), so a failing screen shows in
            // the device log.
            NSLog("[APIClient] %d %@", httpResponse.statusCode, httpResponse.url?.path ?? "-")
            #endif
            let status = httpResponse.statusCode
            let body = APIErrorBody.parse(data)
            // 409 compute_required: the account has no computer; open the gate.
            if status == 409, body.code == "compute_required" {
                Task { @MainActor in ComputeStatusStore.shared.noteComputeRequired(method: method) }
            }
            let error = APIError.httpError(statusCode: status, message: body.calmMessage)
            // A read refused by the gate shows no error (C5): the screen shows
            // its empty state and the no-computer chip reads this flag.
            if error.isComputeGateRefusal, ComputeReadState.isRead(method: method) {
                Task { @MainActor in ComputeReadState.shared.noteRefused() }
            }
            throw error
        }
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
        try validate(response, data: data, method: "GET")
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
        // Start anywhere: a retried session-start returns the first start
        // (cloud-api forwards it, runtime_relay.rs `filtered_headers`).
        "idempotency-key",
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
