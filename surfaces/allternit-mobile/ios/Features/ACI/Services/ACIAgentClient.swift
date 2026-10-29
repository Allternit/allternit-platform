import Foundation

/// Client for the ACI (Agent-Computer-Interface) run protocol:
///
/// - `POST /api/aci/run`            → start a run; returns `{sessionId, adapterId}`
/// - `GET  /api/aci/stream/:id`     → SSE stream of `ACIEvent` frames
///   (`data: {json}\n\n` — state / screenshot / trace / done)
/// - `POST /api/aci/stop/:id`       → stop a run
/// - `POST /api/aci/approve/:id`    → approve a WaitingApproval action
///   (`?deny=true` to deny)
///
/// Reference: the web store's run lifecycle in
/// surfaces/ai.allternit.com/src/capsules/browser/browserAgent.store.ts:546-660
/// and its stop/approve calls at lines 913/940/949. The ACI routes are mounted
/// directly under `/api` (cmd/allternit-api/src/main.rs
/// `.nest("/api", aci_router())`) — NOT under `/api/v1` — so requests go
/// through `APIClient.authorizedRequest(url:)` against `AppConfig.aciBaseURL`,
/// the same pattern AgentChatClient uses for `/api/agent-chat`.
///
/// The ACI routes live on the user's own allternit-api, not on cloud-api, so
/// requests go out through `InstanceConnection.runtimeAPIData/Bytes`: in
/// Release that is the runtime relay to the active computer (as the PWA
/// relays them); a DEBUG build pointed at a local allternit-api goes direct.
final class ACIAgentClient: @unchecked Sendable {
    private let client: APIClient
    private let baseURL: URL

    init(client: APIClient = .shared, baseURL: URL = AppConfig.aciBaseURL) {
        self.client = client
        self.baseURL = baseURL
    }

    /// Body of `POST /api/aci/run` (browserAgent.store.ts:563-570). Defaults
    /// mirror the web store's initial values (browserAgent.store.ts:507-521).
    private struct ACIRunRequest: Encodable {
        let goal: String
        let model: String
        let allowedSites: [String]
        let openLinksInBrowser: Bool
        let autoVerify: Bool
        let sessionPersistence: String
        /// `app:<phoneId>` when the run should drive this iPhone's own ACI
        /// browser (through the phone's control socket) instead of a browser
        /// on the runtime. Omitted otherwise.
        let targetDevice: String?
    }

    /// Response of `POST /api/aci/run` (browserAgent.store.ts:573).
    struct ACIRun: Decodable, Sendable {
        let sessionId: String
        let adapterId: String?
    }

    // MARK: - Run lifecycle

    /// Starts an ACI run for `goal`; returns the session id to stream/control.
    ///
    /// Parameter defaults are the web store's: model
    /// `anthropic/claude-sonnet-4.6`, no site allowlist, links stay in the
    /// agent viewport, auto-verify on, sessions not persisted.
    func startRun(goal: String,
                  model: String = "anthropic/claude-sonnet-4.6",
                  allowedSites: [String] = [],
                  openLinksInBrowser: Bool = false,
                  autoVerify: Bool = true,
                  sessionPersistence: String = "dont-keep",
                  targetDevice: String? = nil) async throws -> ACIRun {
        let body = ACIRunRequest(
            goal: goal,
            model: model,
            allowedSites: allowedSites,
            openLinksInBrowser: openLinksInBrowser,
            autoVerify: autoVerify,
            sessionPersistence: sessionPersistence,
            targetDevice: targetDevice
        )
        var request = try await client.authorizedRequest(
            url: baseURL.appendingPathComponent("run"), method: "POST"
        )
        request.httpBody = try JSONEncoder().encode(body)

        let (data, response) = try await InstanceConnection.runtimeAPIData(for: request, session: client.session)
        try client.validate(response, data: data)
        do {
            return try JSONDecoder().decode(ACIRun.self, from: data)
        } catch {
            throw APIError.decoding(error)
        }
    }

    /// Streams the run's events from `GET /api/aci/stream/:id`.
    ///
    /// The stream finishes on the terminal `done` frame, when the connection
    /// ends, or when the consuming task is cancelled; per-frame decode
    /// failures are skipped (the web parser ignores malformed events).
    /// Server-side stop is the caller's job (`stop(runId:)`) — cancelling the
    /// consumer only tears down the local connection.
    func stream(runId: String) -> AsyncThrowingStream<ACIEvent, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    var request = try await client.authorizedRequest(
                        url: baseURL.appendingPathComponent("stream/\(Self.escape(runId))")
                    )
                    request.setValue("text/event-stream", forHTTPHeaderField: "Accept")
                    // Runs are long-lived; the default 60s idle timeout would
                    // kill quiet stretches between screenshots.
                    request.timeoutInterval = 600

                    let (bytes, response) = try await InstanceConnection.runtimeAPIBytes(for: request, session: client.session)
                    try client.validate(response)

                    let decoder = JSONDecoder()
                    for try await line in bytes.lines {
                        try Task.checkCancellation()

                        // SSE frames are `data: {json}`; blank lines, keep-alive
                        // comments and event lines are skipped.
                        guard line.hasPrefix("data: ") else { continue }
                        let payload = String(line.dropFirst(6))
                        guard !payload.isEmpty else { continue }

                        guard let data = payload.data(using: .utf8),
                              let event = try? decoder.decode(ACIEvent.self, from: data) else {
                            print("aci: skipping undecodable frame: \(payload)")
                            continue
                        }

                        // Unknown/tolerated frames decode to `.ignored`.
                        if case .ignored = event { continue }

                        continuation.yield(event)

                        // `done` is terminal — the web closes the EventSource.
                        if case .done = event {
                            continuation.finish()
                            return
                        }
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }

            // Cancelling the consumer cancels the Task, which cancels the
            // underlying URLSession stream.
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    /// Stops the run server-side (`POST /api/aci/stop/:id`). The web
    /// fire-and-forgets this (`.catch(() => {})`); callers decide whether to
    /// await or discard the result.
    func stop(runId: String) async throws {
        let request = try await client.authorizedRequest(
            url: baseURL.appendingPathComponent("stop/\(Self.escape(runId))"),
            method: "POST"
        )
        let (data, response) = try await InstanceConnection.runtimeAPIData(for: request, session: client.session)
        try client.validate(response, data: data)
    }

    /// Approves — or with `deny: true` denies — the action a run in
    /// `WaitingApproval` is parked on (`POST /api/aci/approve/:id[?deny=true]`,
    /// browserAgent.store.ts:940-949).
    func approve(runId: String, deny: Bool = false) async throws {
        var url = baseURL.appendingPathComponent("approve/\(Self.escape(runId))")
        if deny, var components = URLComponents(url: url, resolvingAgainstBaseURL: false) {
            components.queryItems = [URLQueryItem(name: "deny", value: "true")]
            url = components.url ?? url
        }
        let request = try await client.authorizedRequest(url: url, method: "POST")
        let (data, response) = try await InstanceConnection.runtimeAPIData(for: request, session: client.session)
        try client.validate(response, data: data)
    }

    /// Web uses plain session ids in the path; percent-encode anyway as
    /// parity insurance (same as AgentChatClient.escape).
    private static func escape(_ runId: String) -> String {
        runId.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? runId
    }
}
