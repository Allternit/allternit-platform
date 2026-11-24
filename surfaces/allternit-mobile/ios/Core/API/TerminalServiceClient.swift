import Foundation

/// Client for the computer's terminal service (web lib/terminal-api.ts): the
/// tmux-backed shells Allternit Desktop and the runtime serve at
/// `/terminal/*`. Through the cloud relay this is what m.allternit.com uses;
/// gizzi-code's `/v1/pty` routes are only on a directly reached gizzi-code
/// server (`PtyClient`), and a relayed computer answers them 405.
final class TerminalServiceClient: @unchecked Sendable {
    /// A relay placeholder (`allternit-relay://<runtimeId>`) or a gateway URL.
    let baseURL: URL

    init(baseURL: URL) {
        self.baseURL = baseURL
    }

    private var api: APIClient {
        APIClient(baseURL: baseURL, tokenProvider: { try await AuthManager.shared.getToken() })
    }

    /// Optional parts of a new terminal (web `createTerminalSession` options):
    /// a program to run instead of the shell (an agent CLI), its args and
    /// env, and the name the terminal lists under.
    struct CreateOptions: Sendable {
        var command: String? = nil
        var args: [String] = []
        var env: [String: String] = [:]
        var title: String? = nil
    }

    /// zsh is only the default: where the computer has none, an empty shell
    /// runs its own default (createTerminalSession's MISSING_SHELL retry).
    func create(cols: Int = 80, rows: Int = 24, cwd: String? = nil, options: CreateOptions = CreateOptions()) async throws -> String {
        do {
            return try await create(shell: "/bin/zsh", cols: cols, rows: rows, cwd: cwd, options: options)
        } catch let error as APIError {
            let text = error.errorDescription ?? ""
            guard text.range(of: "ENOENT|doesn't exist|No such file", options: .regularExpression) != nil else { throw error }
            return try await create(shell: "", cols: cols, rows: rows, cwd: cwd, options: options)
        }
    }

    private func create(shell: String, cols: Int, rows: Int, cwd: String?, options: CreateOptions) async throws -> String {
        let reply: CreateReply = try await api.post(
            path: "terminal/create",
            body: CreateBody(shell: shell, cols: cols, rows: rows, cwd: cwd,
                             command: options.command?.isEmpty == false ? options.command : nil,
                             args: options.args.isEmpty ? nil : options.args,
                             env: options.env.isEmpty ? nil : options.env,
                             title: options.title?.isEmpty == false ? options.title : nil)
        )
        if reply.success == false {
            throw APIError.httpError(statusCode: 500, message: reply.message ?? reply.details ?? "Failed to create terminal")
        }
        guard let id = reply.sessionId ?? reply.session_id ?? reply.data?.session_id, !id.isEmpty else {
            throw APIError.httpError(statusCode: 500, message: "Terminal service did not return a session id")
        }
        return id
    }

    func input(sessionId: String, _ content: String) async throws {
        try await api.post(path: "terminal/\(Self.escape(sessionId))/input",
                           body: InputBody(session_id: sessionId, content: content))
    }

    func resize(sessionId: String, cols: Int, rows: Int) async throws {
        try await api.post(path: "terminal/\(Self.escape(sessionId))/resize",
                           body: ResizeBody(session_id: sessionId, cols: cols, rows: rows))
    }

    /// Ends the shell on the computer. `graceful` asks the runtime to stop
    /// the program politely first (left out: the runtime's default).
    func close(sessionId: String, graceful: Bool? = nil) async throws {
        try await api.post(path: "terminal/\(Self.escape(sessionId))/close",
                           body: CloseBody(session_id: sessionId, graceful: graceful))
    }

    /// `GET /terminal/list?preview=N`: every open terminal on the computer,
    /// with its last `preview` screen rows when asked. `.unsupported` on a
    /// runtime that predates the route (404); throws on other failures so a
    /// poller can back off.
    func list(preview: Int = 0) async throws -> RuntimeListResult<TerminalListItem> {
        let query = preview > 0 ? "?preview=\(preview)" : ""
        let (data, status) = try await RuntimeRaw.send(api, path: "terminal/list\(query)")
        if status == 404 { return .unsupported }
        guard (200...299).contains(status) else { throw RuntimeRaw.error(status: status, data: data, fallback: "Terminal list failed") }
        let reply = (try? JSONDecoder().decode(ListReply.self, from: data)) ?? ListReply(terminals: [])
        return .items(reply.terminals ?? [])
    }

    /// `GET /terminal/:id/stream` (SSE): each event's data is
    /// `{ type: "data", data }`, or raw text.
    func stream(sessionId: String) async throws -> URLSession.AsyncBytes {
        let client = api
        var request = try await client.authorizedRequest(path: "terminal/\(Self.escape(sessionId))/stream")
        request.setValue("text/event-stream", forHTTPHeaderField: "Accept")
        request.timeoutInterval = 60 * 60 * 24
        let (bytes, response) = try await client.sendStream(request)
        try client.validate(response)
        return bytes
    }

    private static func escape(_ id: String) -> String {
        id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? id
    }

    private struct CreateBody: Encodable {
        let shell: String
        let cols: Int
        let rows: Int
        let cwd: String?
        let command: String?
        let args: [String]?
        let env: [String: String]?
        let title: String?
    }

    private struct ListReply: Decodable {
        let terminals: [TerminalListItem]?
    }

    private struct CreateReply: Decodable {
        struct Inner: Decodable { let session_id: String? }
        let success: Bool?
        let message: String?
        let details: String?
        let sessionId: String?
        let session_id: String?
        let data: Inner?
    }

    private struct InputBody: Encodable {
        let session_id: String
        let content: String
    }

    private struct ResizeBody: Encodable {
        let session_id: String
        let cols: Int
        let rows: Int
    }

    private struct CloseBody: Encodable {
        let session_id: String
        let graceful: Bool?
    }
}

// MARK: - Terminal list

/// A list call's answer: the rows, or a runtime too old to have the route.
enum RuntimeListResult<Item: Sendable>: Sendable {
    case items([Item])
    case unsupported
}

/// One open terminal as the runtime lists it (`GET /terminal/list`, web
/// `TerminalListItem`). Missing fields decode to empty values, so an older or
/// newer runtime never fails the whole list.
struct TerminalListItem: Decodable, Equatable, Identifiable, Sendable {
    let sessionId: String
    let paneId: String?
    let title: String
    let cwd: String
    let command: [String]
    let running: Bool
    let exitCode: Int?
    let pid: Int?
    let createdAt: String?
    /// Last visible rows, top to bottom; only when `preview` was asked for.
    let preview: [String]?

    var id: String { sessionId }

    init(sessionId: String, paneId: String? = nil, title: String = "", cwd: String = "", command: [String] = [],
         running: Bool = true, exitCode: Int? = nil, pid: Int? = nil, createdAt: String? = nil, preview: [String]? = nil) {
        self.sessionId = sessionId
        self.paneId = paneId
        self.title = title
        self.cwd = cwd
        self.command = command
        self.running = running
        self.exitCode = exitCode
        self.pid = pid
        self.createdAt = createdAt
        self.preview = preview
    }

    private enum CodingKeys: String, CodingKey {
        case sessionId = "session_id", paneId = "pane_id", title, cwd, command, running
        case exitCode = "exit_code", pid, createdAt = "created_at", preview
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        sessionId = try c.decode(String.self, forKey: .sessionId)
        paneId = try? c.decodeIfPresent(String.self, forKey: .paneId)
        title = (try? c.decodeIfPresent(String.self, forKey: .title)) ?? ""
        cwd = (try? c.decodeIfPresent(String.self, forKey: .cwd)) ?? ""
        command = (try? c.decodeIfPresent([String].self, forKey: .command)) ?? []
        running = (try? c.decodeIfPresent(Bool.self, forKey: .running)) ?? true
        exitCode = try? c.decodeIfPresent(Int.self, forKey: .exitCode)
        pid = try? c.decodeIfPresent(Int.self, forKey: .pid)
        createdAt = try? c.decodeIfPresent(String.self, forKey: .createdAt)
        preview = try? c.decodeIfPresent([String].self, forKey: .preview)
    }
}

// MARK: - Live sessions

/// The runtime refused or could not run a live-sessions call: the HTTP status
/// and the `{ error, message }` body (web `AdoptError`). 404 = runtime too old.
struct LiveSessionError: Error, LocalizedError, Equatable, Sendable {
    let status: Int
    let code: String
    let message: String

    var unsupported: Bool { status == 404 }
    var errorDescription: String? { message }
}

/// Raw sends that keep the status and the body, which `APIClient.validate`
/// flattens into a calm message (the adopt routes need the error code).
enum RuntimeRaw {
    static func send(_ api: APIClient, path: String, method: String = "GET", body: Data? = nil) async throws -> (Data, Int) {
        var request = try await api.authorizedRequest(path: path, method: method)
        request.httpBody = body
        let (data, response) = try await api.send(request)
        guard let http = response as? HTTPURLResponse else { throw APIError.invalidResponse }
        return (data, http.statusCode)
    }

    static func error(status: Int, data: Data, fallback: String) -> LiveSessionError {
        struct Body: Decodable { let error: String?; let message: String? }
        let body = try? JSONDecoder().decode(Body.self, from: data)
        let message = body?.message?.trimmingCharacters(in: .whitespacesAndNewlines)
        return LiveSessionError(status: status, code: body?.error ?? "unknown",
                                message: message?.isEmpty == false ? message! : "\(fallback): \(status)")
    }
}

/// One process that has a live session open (`holders[]`).
struct LiveSessionHolder: Decodable, Equatable, Sendable {
    let pid: Int
    /// "Terminal", "iTerm2", "Allternit", "Ghostty", "other".
    let app: String
    let terminalId: String?
    let paneId: String?
    let tty: String?
    let startedAt: String?

    init(pid: Int, app: String, terminalId: String? = nil, paneId: String? = nil, tty: String? = nil, startedAt: String? = nil) {
        self.pid = pid
        self.app = app
        self.terminalId = terminalId
        self.paneId = paneId
        self.tty = tty
        self.startedAt = startedAt
    }
}

/// A Move job (`move` on a live session, `GET .../adopt/:jobId`).
struct AdoptJob: Decodable, Equatable, Sendable {
    /// waiting_idle | stopping | starting | done | failed
    let jobId: String
    let state: String
    let terminalId: String?
    let code: String?
    let error: String?
    /// Client-only line under the progress text (a cancel that came too late).
    var notice: String? = nil

    var finished: Bool { state == "done" || state == "failed" }

    private enum CodingKeys: String, CodingKey { case jobId, state, terminalId, code, error }

    init(jobId: String, state: String, terminalId: String? = nil, code: String? = nil, error: String? = nil, notice: String? = nil) {
        self.jobId = jobId
        self.state = state
        self.terminalId = terminalId
        self.code = code
        self.error = error
        self.notice = notice
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        jobId = (try? c.decodeIfPresent(String.self, forKey: .jobId)) ?? ""
        state = (try? c.decodeIfPresent(String.self, forKey: .state)) ?? "waiting_idle"
        terminalId = try? c.decodeIfPresent(String.self, forKey: .terminalId)
        code = try? c.decodeIfPresent(String.self, forKey: .code)
        error = try? c.decodeIfPresent(String.self, forKey: .error)
    }
}

/// An agent CLI session on the connected computer (`GET /api/v1/live-sessions`,
/// camelCase; web `LiveSession`). Only the fields the phone shows.
struct LiveSession: Decodable, Equatable, Identifiable, Sendable {
    let address: String
    let name: String
    let harness: String
    let machineName: String
    let pid: Int?
    let sessionId: String
    let cwd: String
    /// busy | idle | offline
    let status: String
    let holders: [LiveSessionHolder]
    let carriedFlags: [String]
    let canCopy: Bool
    /// Running outside Allternit.
    let external: Bool
    /// now | busy | open_elsewhere | no_resume | here
    let movable: String?
    /// The exact argv the runtime runs for each action; shown, never built here.
    let moveCommand: [String]?
    let copyCommand: [String]?
    let newCommand: [String]?
    let move: AdoptJob?

    var id: String { sessionId }

    init(address: String = "", name: String = "", harness: String = "", machineName: String = "", pid: Int? = nil,
         sessionId: String, cwd: String = "", status: String = "idle", holders: [LiveSessionHolder] = [],
         carriedFlags: [String] = [], canCopy: Bool = false, external: Bool = true, movable: String? = nil,
         moveCommand: [String]? = nil, copyCommand: [String]? = nil, newCommand: [String]? = nil, move: AdoptJob? = nil) {
        self.address = address
        self.name = name
        self.harness = harness
        self.machineName = machineName
        self.pid = pid
        self.sessionId = sessionId
        self.cwd = cwd
        self.status = status
        self.holders = holders
        self.carriedFlags = carriedFlags
        self.canCopy = canCopy
        self.external = external
        self.movable = movable
        self.moveCommand = moveCommand
        self.copyCommand = copyCommand
        self.newCommand = newCommand
        self.move = move
    }

    private enum CodingKeys: String, CodingKey {
        case address, name, harness, machineName, pid, sessionId, cwd, status, holders
        case carriedFlags, canCopy, external, movable, moveCommand, copyCommand, newCommand, move
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        sessionId = try c.decode(String.self, forKey: .sessionId)
        address = (try? c.decodeIfPresent(String.self, forKey: .address)) ?? ""
        name = (try? c.decodeIfPresent(String.self, forKey: .name)) ?? ""
        harness = (try? c.decodeIfPresent(String.self, forKey: .harness)) ?? ""
        machineName = (try? c.decodeIfPresent(String.self, forKey: .machineName)) ?? ""
        pid = try? c.decodeIfPresent(Int.self, forKey: .pid)
        cwd = (try? c.decodeIfPresent(String.self, forKey: .cwd)) ?? ""
        status = (try? c.decodeIfPresent(String.self, forKey: .status)) ?? "idle"
        holders = (try? c.decodeIfPresent([LiveSessionHolder].self, forKey: .holders)) ?? []
        carriedFlags = (try? c.decodeIfPresent([String].self, forKey: .carriedFlags)) ?? []
        canCopy = (try? c.decodeIfPresent(Bool.self, forKey: .canCopy)) ?? false
        external = (try? c.decodeIfPresent(Bool.self, forKey: .external)) ?? false
        movable = try? c.decodeIfPresent(String.self, forKey: .movable)
        moveCommand = try? c.decodeIfPresent([String].self, forKey: .moveCommand)
        copyCommand = try? c.decodeIfPresent([String].self, forKey: .copyCommand)
        newCommand = try? c.decodeIfPresent([String].self, forKey: .newCommand)
        move = try? c.decodeIfPresent(AdoptJob.self, forKey: .move)
    }
}

/// What an adopt call started: a Move job to follow, or the new terminal
/// (copy / new), or a cancel that went through.
enum AdoptResult: Equatable, Sendable {
    case job(AdoptJob)
    case terminal(String)
    case cancelled
}

/// The live-sessions routes (web live-sessions-api.ts), on the same base URL,
/// auth and relay path as `TerminalServiceClient`.
final class LiveSessionsClient: @unchecked Sendable {
    let baseURL: URL

    init(baseURL: URL) {
        self.baseURL = baseURL
    }

    private var api: APIClient {
        APIClient(baseURL: baseURL, tokenProvider: { try await AuthManager.shared.getToken() })
    }

    /// `.unsupported` when the runtime has no live-sessions route (404).
    func list() async throws -> RuntimeListResult<LiveSession> {
        let (data, status) = try await RuntimeRaw.send(api, path: "api/v1/live-sessions")
        if status == 404 { return .unsupported }
        guard (200...299).contains(status) else { throw RuntimeRaw.error(status: status, data: data, fallback: "Live sessions failed") }
        struct Wrapped: Decodable { let sessions: [LiveSession]? }
        if let rows = try? JSONDecoder().decode([LiveSession].self, from: data) { return .items(rows) }
        if let wrapped = try? JSONDecoder().decode(Wrapped.self, from: data) { return .items(wrapped.sessions ?? []) }
        return .items([])
    }

    /// `POST /api/v1/live-sessions/adopt`. mode: move | copy | new | cancel.
    func adopt(sessionId: String, address: String, harness: String, mode: String, cols: Int, rows: Int) async throws -> AdoptResult {
        struct Body: Encodable { let sessionId: String?; let address: String?; let harness: String; let mode: String; let cols: Int; let rows: Int }
        struct Reply: Decodable { let jobId: String?; let state: String?; let terminalId: String? }
        let body = try JSONEncoder().encode(Body(sessionId: sessionId.isEmpty ? nil : sessionId,
                                                 address: address.isEmpty ? nil : address,
                                                 harness: harness, mode: mode, cols: cols, rows: rows))
        let (data, status) = try await RuntimeRaw.send(api, path: "api/v1/live-sessions/adopt", method: "POST", body: body)
        guard (200...299).contains(status) else { throw RuntimeRaw.error(status: status, data: data, fallback: "Adopt failed") }
        if mode == "cancel" { return .cancelled }
        let reply = try? JSONDecoder().decode(Reply.self, from: data)
        if mode == "move" {
            guard let jobId = reply?.jobId, !jobId.isEmpty else {
                throw LiveSessionError(status: 502, code: "bad_response", message: "The runtime did not return a move job")
            }
            return .job(AdoptJob(jobId: jobId, state: reply?.state ?? "waiting_idle"))
        }
        guard let terminalId = reply?.terminalId, !terminalId.isEmpty else {
            throw LiveSessionError(status: 502, code: "bad_response", message: "The runtime did not return a terminal")
        }
        return .terminal(terminalId)
    }

    /// `GET /api/v1/live-sessions/adopt/:jobId`.
    func job(_ jobId: String) async throws -> AdoptJob {
        let escaped = jobId.addingPercentEncoding(withAllowedCharacters: .alphanumerics.union(CharacterSet(charactersIn: "-_."))) ?? jobId
        let (data, status) = try await RuntimeRaw.send(api, path: "api/v1/live-sessions/adopt/\(escaped)")
        guard (200...299).contains(status) else { throw RuntimeRaw.error(status: status, data: data, fallback: "Adopt job failed") }
        do {
            return try JSONDecoder().decode(AdoptJob.self, from: data)
        } catch {
            throw APIError.decoding(error)
        }
    }
}
