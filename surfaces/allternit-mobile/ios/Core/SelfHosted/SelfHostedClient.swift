import Foundation

/// Why a call to the self-hosted server failed, in words the Settings page shows.
enum SelfHostedError: Error, Equatable {
    case invalidURL
    case insecureURL            // http:// (HTTPS only)
    case unauthorized           // 401
    case forbidden              // 403
    case unreachable(String)
    case server(status: Int, code: String?, message: String)
    case badResponse

    var message: String {
        switch self {
        case .invalidURL:
            return "That doesn’t look like a server address. Enter it like https://agents.example.com."
        case .insecureURL:
            return "Allternit connects to your server over HTTPS only, so your API key is never sent in the clear. Put the server behind HTTPS (a reverse proxy or a tunnel) and use its https:// address."
        case .unauthorized:
            return "The server refused this API key (401). Check the key starts with ask_ and has not been revoked."
        case .forbidden:
            return "This key isn’t allowed to do that (403). Use a key with a developer or admin role."
        case .unreachable(let why):
            return "Couldn’t reach the server. Check the address and that this iPhone can see it. \(why)"
        case .server(let status, _, let message):
            return "The server answered \(status): \(message)"
        case .badResponse:
            return "The server sent a reply Allternit doesn’t understand. Is this an Allternit Agents Server?"
        }
    }
}

/// URLSession client for the Self-Hosted Agents Server (`selfhost-v1.yaml`),
/// Bearer `ask_…` keys, HTTPS only.
struct SelfHostedClient {
    let baseURL: URL
    let apiKey: String
    var session: URLSession = .shared

    /// Parse what the user typed. Adds https:// when no scheme is given;
    /// refuses http://; strips a trailing slash and any path/query.
    static func normalize(_ input: String) throws -> URL {
        var text = input.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { throw SelfHostedError.invalidURL }
        if !text.contains("://") { text = "https://" + text }
        guard var parts = URLComponents(string: text), let scheme = parts.scheme?.lowercased(),
              let host = parts.host, !host.isEmpty else { throw SelfHostedError.invalidURL }
        guard scheme == "https" else {
            throw scheme == "http" ? SelfHostedError.insecureURL : SelfHostedError.invalidURL
        }
        parts.scheme = scheme
        parts.query = nil; parts.fragment = nil
        while parts.path.hasSuffix("/") { parts.path.removeLast() }
        guard let url = parts.url else { throw SelfHostedError.invalidURL }
        return url
    }

    // MARK: Endpoints

    func serverInfo() async throws -> SHServerInfo {
        try await request("GET", "/v1/server/info", authorized: false)
    }

    func listAgents() async throws -> [SHAgent] {
        (try await request("GET", "/v1/agents?limit=100") as SHList<SHAgent>).data
    }

    func listEnvironments() async throws -> [SHEnvironment] {
        (try await request("GET", "/v1/environments?limit=100") as SHList<SHEnvironment>).data
    }

    func listWorkers(environmentID: String) async throws -> [SHWorker] {
        (try await request("GET", "/v1/environments/\(escape(environmentID))/workers") as SHList<SHWorker>).data
    }

    func createSession(agentID: String, environmentID: String, title: String? = nil) async throws -> SHSession {
        var body: [String: String] = ["agent_id": agentID, "environment_id": environmentID]
        if let title { body["title"] = title }
        return try await request("POST", "/v1/sessions", body: body)
    }

    func getSession(id: String) async throws -> SHSession {
        try await request("GET", "/v1/sessions/\(escape(id))")
    }

    @discardableResult
    func sendMessage(sessionID: String, content: String) async throws -> SHEvent {
        try await request("POST", "/v1/sessions/\(escape(sessionID))/messages", body: ["content": content])
    }

    func interrupt(sessionID: String) async throws {
        let _: SHSession = try await request("POST", "/v1/sessions/\(escape(sessionID))/interrupt")
    }

    // MARK: Event stream

    /// Events after `afterSeq`, oldest first, then live. Reconnects with
    /// `Last-Event-ID` after a drop (2s backoff, growing to 15s) until the
    /// consumer cancels. Auth errors end the stream with the error.
    func streamEvents(sessionID: String, afterSeq: Int = 0) -> AsyncThrowingStream<SHEvent, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                var lastSeq = afterSeq
                var delay: UInt64 = 2
                while !Task.isCancelled {
                    do {
                        var req = try makeRequest("GET", "/v1/sessions/\(escape(sessionID))/events/stream", authorized: true)
                        req.setValue("text/event-stream", forHTTPHeaderField: "Accept")
                        req.timeoutInterval = 90 // keepalives arrive every 15s
                        if lastSeq > 0 { req.setValue(String(lastSeq), forHTTPHeaderField: "Last-Event-ID") }
                        let (bytes, response) = try await session.bytes(for: req)
                        try check(response, body: nil)
                        delay = 2
                        var parser = SSEParser(lastEventID: lastSeq > 0 ? String(lastSeq) : nil)
                        var chunk: [UInt8] = []
                        for try await byte in bytes {
                            chunk.append(byte)
                            if byte == 0x0A {
                                for frame in parser.feed(chunk) {
                                    if let ev = Self.decodeEvent(frame), ev.seq > lastSeq {
                                        lastSeq = ev.seq
                                        continuation.yield(ev)
                                    }
                                }
                                chunk.removeAll(keepingCapacity: true)
                            }
                        }
                    } catch is CancellationError {
                        break
                    } catch let error as SelfHostedError {
                        if case .unreachable = error {} else {
                            continuation.finish(throwing: error); return
                        }
                    } catch {
                        if Task.isCancelled { break }
                    }
                    try? await Task.sleep(nanoseconds: delay * 1_000_000_000)
                    delay = min(delay * 2, 15)
                }
                continuation.finish()
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    static func decodeEvent(_ frame: SSEFrame) -> SHEvent? {
        try? JSONDecoder().decode(SHEvent.self, from: Data(frame.data.utf8))
    }

    // MARK: Plumbing

    private func escape(_ s: String) -> String {
        s.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed.subtracting(CharacterSet(charactersIn: "/"))) ?? s
    }

    private func makeRequest(_ method: String, _ path: String, authorized: Bool) throws -> URLRequest {
        guard baseURL.scheme?.lowercased() == "https",
              let url = URL(string: baseURL.absoluteString + path) else {
            throw baseURL.scheme?.lowercased() == "http" ? SelfHostedError.insecureURL : SelfHostedError.invalidURL
        }
        var req = URLRequest(url: url)
        req.httpMethod = method
        req.timeoutInterval = 15
        req.setValue("application/json", forHTTPHeaderField: "Accept")
        if authorized { req.setValue("Bearer \(apiKey)", forHTTPHeaderField: "Authorization") }
        return req
    }

    private func check(_ response: URLResponse, body: Data?) throws {
        guard let http = response as? HTTPURLResponse else { throw SelfHostedError.badResponse }
        switch http.statusCode {
        case 200..<300: return
        case 401: throw SelfHostedError.unauthorized
        case 403: throw SelfHostedError.forbidden
        default:
            struct Envelope: Decodable { struct E: Decodable { var message: String?; var code: String? }; var error: E? }
            let env = body.flatMap { try? JSONDecoder().decode(Envelope.self, from: $0) }
            throw SelfHostedError.server(status: http.statusCode, code: env?.error?.code,
                                         message: env?.error?.message ?? HTTPURLResponse.localizedString(forStatusCode: http.statusCode))
        }
    }

    private func request<T: Decodable>(_ method: String, _ path: String, body: [String: String]? = nil,
                                       authorized: Bool = true) async throws -> T {
        var req = try makeRequest(method, path, authorized: authorized)
        if let body {
            req.httpBody = try JSONSerialization.data(withJSONObject: body)
            req.setValue("application/json", forHTTPHeaderField: "Content-Type")
            req.setValue(UUID().uuidString, forHTTPHeaderField: "Idempotency-Key")
        }
        let data: Data, response: URLResponse
        do {
            (data, response) = try await session.data(for: req)
        } catch {
            if (error as? URLError)?.code == .cancelled { throw error }
            throw SelfHostedError.unreachable((error as? URLError)?.localizedDescription ?? error.localizedDescription)
        }
        try check(response, body: data)
        do { return try JSONDecoder().decode(T.self, from: data) }
        catch { throw SelfHostedError.badResponse }
    }
}
