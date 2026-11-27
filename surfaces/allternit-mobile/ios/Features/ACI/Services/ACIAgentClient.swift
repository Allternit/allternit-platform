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
        /// Run being repeated on another computer ("Re-run on my Mac").
        /// Wire name is snake_case (CONTRACT.md); a fresh run, no logins carried.
        let rerun_of: String?
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
                  targetDevice: String? = nil,
                  rerunOf: String? = nil,
                  runtimeId: String? = nil) async throws -> ACIRun {
        let body = ACIRunRequest(
            goal: goal,
            model: model,
            allowedSites: allowedSites,
            openLinksInBrowser: openLinksInBrowser,
            autoVerify: autoVerify,
            sessionPersistence: sessionPersistence,
            targetDevice: targetDevice,
            rerun_of: rerunOf
        )
        let (data, response) = try await send(
            path: "aci/run", runtimeId: runtimeId, bearer: nil,
            body: try JSONEncoder().encode(body)
        )
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

    // MARK: - Agent browser: gates, take over, block

    /// Sends a POST to `/api/<path>` on the computer. With `runtimeId` it goes
    /// through that computer's relay (a push action or a re-run target that is
    /// not the active computer); `bearer` is the background credential when
    /// there is no UI to ask AuthManager. Otherwise the same connection path
    /// as every other `/api/aci/*` call.
    private func send(path: String, runtimeId: String?, bearer: String?,
                      body: Data? = nil, method: String = "POST",
                      query: [URLQueryItem] = []) async throws -> (Data, URLResponse) {
        let apiRoot = baseURL.deletingLastPathComponent() // .../api
        var request: URLRequest
        if let runtimeId, !runtimeId.isEmpty {
            request = URLRequest(url: Self.applying(query, to: RuntimeRelayClient.relayBaseURL(runtimeId: runtimeId)
                .appendingPathComponent("api").appendingPathComponent(path)))
            request.httpMethod = method
            request.timeoutInterval = 20
            request.setValue("application/json", forHTTPHeaderField: "Accept")
            if let bearer { request.setValue("Bearer \(bearer)", forHTTPHeaderField: "Authorization") }
            else if let token = try await AuthManager.shared.getToken() {
                request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
            }
        } else if bearer == nil {
            request = try await client.authorizedRequest(
                url: Self.applying(query, to: url(apiRoot, path)), method: method)
        } else {
            request = URLRequest(url: Self.applying(query, to: url(apiRoot, path)))
            request.httpMethod = method
            request.timeoutInterval = 20
            request.setValue("Bearer \(bearer ?? "")", forHTTPHeaderField: "Authorization")
        }
        if let body {
            request.httpBody = body
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }
        let data: Data
        let response: URLResponse
        if let runtimeId, !runtimeId.isEmpty {
            let relay = RuntimeRelayClient(session: client.session, tokenProvider: {
                if let bearer { return bearer }
                return try await AuthManager.shared.getToken()
            })
            (data, response) = try await relay.send(request, runtimeId: runtimeId)
        } else {
            (data, response) = try await InstanceConnection.runtimeAPIData(for: request, session: client.session)
        }
        try client.validate(response, data: data)
        return (data, response)
    }

    private static func applying(_ query: [URLQueryItem], to url: URL) -> URL {
        guard !query.isEmpty, var parts = URLComponents(url: url, resolvingAgainstBaseURL: false) else { return url }
        parts.queryItems = query
        return parts.url ?? url
    }

    private func url(_ root: URL, _ path: String) -> URL {
        path.split(separator: "/").reduce(root) { $0.appendingPathComponent(String($1)) }
    }

    /// Approves a credential fill or gated action
    /// (`POST /api/aci/handoff/:approval_id/approve`). `scope: "run"` is
    /// "Allow for this run" (same credential name + domain, this run only);
    /// nil is once. Callers ask for Face ID / the passcode first.
    func approveHandoff(approvalId: String, scope: String? = nil,
                        runtimeId: String? = nil, bearer: String? = nil) async throws {
        var body: Data?
        if let scope { body = try JSONSerialization.data(withJSONObject: ["scope": scope]) }
        _ = try await send(path: "aci/handoff/\(Self.escape(approvalId))/approve",
                           runtimeId: runtimeId, bearer: bearer, body: body)
    }

    /// Denies it (`POST /api/aci/handoff/:approval_id/deny`). No auth prompt.
    func denyHandoff(approvalId: String, runtimeId: String? = nil, bearer: String? = nil) async throws {
        _ = try await send(path: "aci/handoff/\(Self.escape(approvalId))/deny",
                           runtimeId: runtimeId, bearer: bearer)
    }

    /// Stops a run with an explicit relay target and credential (the push
    /// "Stop" button). `stop(runId:)` is the in-app one.
    func stopRun(runId: String, runtimeId: String?, bearer: String?) async throws {
        _ = try await send(path: "aci/stop/\(Self.escape(runId))", runtimeId: runtimeId, bearer: bearer)
    }

    /// "Continue" after the bot asked for you (`POST .../human-done`).
    func humanDone(computerId: String, runtimeId: String? = nil, bearer: String? = nil) async throws {
        _ = try await send(path: "v1/computers/\(Self.escape(computerId))/human-done",
                           runtimeId: runtimeId, bearer: bearer)
    }

    /// Answers a detected block (`POST .../block-resolve`): "continue",
    /// "stop" or "rerun" (with the target computer id).
    func resolveBlock(computerId: String, action: String, rerunTarget: String? = nil,
                      runtimeId: String? = nil, bearer: String? = nil) async throws {
        var body: [String: String] = ["action": action]
        if let rerunTarget { body["rerun_target"] = rerunTarget }
        _ = try await send(path: "v1/computers/\(Self.escape(computerId))/block-resolve",
                           runtimeId: runtimeId, bearer: bearer,
                           body: try JSONSerialization.data(withJSONObject: body))
    }

    // MARK: - Agent browser: take over, secret, re-run, live view

    /// A person takes control of the computer (`POST .../control/take`). While
    /// they hold it, the bot's calls are refused.
    func takeControl(computerId: String, runtimeId: String? = nil) async throws {
        _ = try await send(path: "v1/computers/\(Self.escape(computerId))/control/take", runtimeId: runtimeId, bearer: nil)
    }

    /// Hands it back (`POST .../control/release`).
    func releaseControl(computerId: String, runtimeId: String? = nil) async throws {
        _ = try await send(path: "v1/computers/\(Self.escape(computerId))/control/release", runtimeId: runtimeId, bearer: nil)
    }

    /// Mouse on a computer you hold control of (`POST .../mouse`), screen pixels.
    /// `action`: click | doubleclick | rightclick | move | scroll (with `button` + `amount`).
    func mouse(computerId: String, action: String, x: Int, y: Int,
               button: String? = nil, amount: Int? = nil, runtimeId: String? = nil) async throws {
        var body: [String: Any] = ["action": action, "x": x, "y": y]
        if let button { body["button"] = button }
        if let amount { body["amount"] = amount }
        _ = try await send(path: "v1/computers/\(Self.escape(computerId))/mouse", runtimeId: runtimeId, bearer: nil,
                           body: try JSONSerialization.data(withJSONObject: body))
    }

    /// Keyboard on a computer you hold control of (`POST .../keyboard`):
    /// `type` with `text`, or `key` with a key name.
    func keyboard(computerId: String, text: String? = nil, key: String? = nil, runtimeId: String? = nil) async throws {
        let body: [String: String] = text.map { ["action": "type", "text": $0] } ?? ["action": "key", "key": key ?? "Return"]
        _ = try await send(path: "v1/computers/\(Self.escape(computerId))/keyboard", runtimeId: runtimeId, bearer: nil,
                           body: try JSONSerialization.data(withJSONObject: body))
    }

    /// The masked secret box (`POST .../secret`): the value goes in this one
    /// request body and nowhere else (never logged, never stored). `saveAs`
    /// also saves it to the vault under that name. 409 = nothing pending.
    func submitSecret(computerId: String, text: String, saveAs: String? = nil, runtimeId: String? = nil) async throws {
        var body: [String: String] = ["text": text]
        if let saveAs, !saveAs.isEmpty { body["save_as"] = saveAs }
        _ = try await send(path: "v1/computers/\(Self.escape(computerId))/secret", runtimeId: runtimeId, bearer: nil,
                           body: try JSONSerialization.data(withJSONObject: body))
    }

    func cancelSecret(computerId: String, runtimeId: String? = nil) async throws {
        _ = try await send(path: "v1/computers/\(Self.escape(computerId))/secret", runtimeId: runtimeId, bearer: nil,
                           body: try JSONSerialization.data(withJSONObject: ["cancel": true]))
    }

    /// Starts a run on a computer (`POST /api/v1/computers/:id/runs`), the
    /// route "Re-run on my Mac" uses with `rerun_of`. Returns the new run id.
    func startComputerRun(computerId: String, task: String, rerunOf: String? = nil) async throws -> String? {
        var body: [String: String] = ["task": task]
        if let rerunOf { body["rerun_of"] = rerunOf }
        let (data, _) = try await send(path: "v1/computers/\(Self.escape(computerId))/runs",
                                       runtimeId: computerId, bearer: nil,
                                       body: try JSONSerialization.data(withJSONObject: body))
        let json = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        return (json?["run_id"] as? String) ?? (json?["id"] as? String) ?? (json?["sessionId"] as? String)
    }

    /// One frame of the computer's screen (`GET .../screenshot`), or of one
    /// run's own display on Linux (`?run_id=`). Accepts raw image bytes or
    /// JSON holding a base64 image.
    func screenshot(computerId: String, runId: String? = nil, runtimeId: String? = nil) async throws -> Data? {
        let (data, _) = try await send(path: "v1/computers/\(Self.escape(computerId))/screenshot",
                                       runtimeId: runtimeId, bearer: nil, method: "GET",
                                       query: runId.map { [URLQueryItem(name: "run_id", value: $0)] } ?? [])
        if let json = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] {
            let b64 = (json["image"] as? String) ?? (json["screenshot"] as? String) ?? (json["data"] as? String)
            guard let b64 else { return nil }
            return Data(base64Encoded: b64.components(separatedBy: ",").last ?? b64)
        }
        return data
    }

    /// Live agent events for one computer: SSE
    /// `/api/v1/computers/:id/toolset/events`. Every frame is named
    /// `computer.action`; the real kind is `type` in the JSON. Finishes when
    /// the connection ends or the consumer is cancelled; reconnecting is the
    /// caller's job. Undecodable frames are skipped.
    func computerEvents(computerId: String, runtimeId: String? = nil) -> AsyncThrowingStream<ACIComputerEvent, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    let path = "v1/computers/\(Self.escape(computerId))/toolset/events"
                    var request: URLRequest
                    if let runtimeId, !runtimeId.isEmpty {
                        request = URLRequest(url: RuntimeRelayClient.relayBaseURL(runtimeId: runtimeId)
                            .appendingPathComponent("api/\(path)"))
                    } else {
                        request = try await client.authorizedRequest(
                            url: url(baseURL.deletingLastPathComponent(), path))
                    }
                    request.setValue("text/event-stream", forHTTPHeaderField: "Accept")
                    request.timeoutInterval = 600
                    let (bytes, response): (URLSession.AsyncBytes, URLResponse)
                    if let runtimeId, !runtimeId.isEmpty {
                        (bytes, response) = try await RuntimeRelayClient.shared.stream(request, runtimeId: runtimeId)
                    } else {
                        (bytes, response) = try await InstanceConnection.runtimeAPIBytes(for: request, session: client.session)
                    }
                    try client.validate(response)
                    for try await line in bytes.lines {
                        try Task.checkCancellation()
                        guard line.hasPrefix("data:") else { continue }
                        let payload = line.dropFirst(5).trimmingCharacters(in: .whitespaces)
                        guard let data = payload.data(using: .utf8),
                              let event = ACIComputerEvent(json: data) else { continue }
                        continuation.yield(event)
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }
}

// MARK: - Computer toolset events (agent browser contract, PR #3112)

/// The bot driving the browser: `actor` on `computer.action`.
struct ACIAgentActor: Sendable, Equatable {
    let agentId: String?
    let name: String
    let avatarURL: String?
    /// Hex like "#A855F7"; nil falls back to the accent.
    let color: String?
}

/// One pointer step: where the bot clicked or typed, in the screen's own
/// pixels (`screenW` x `screenH`).
struct ACIAgentAction: Sendable, Equatable {
    let runId: String?
    let toolset: String?
    let member: String?
    let x: Double
    let y: Double
    let screenW: Double
    let screenH: Double
    let ok: Bool
    let actor: ACIAgentActor?
    let step: Int?
    let rerunOf: String?
}

/// The bot wants to fill a login or take a gated action: ask the owner.
struct ACIApprovalRequest: Sendable, Equatable, Identifiable {
    var id: String { approvalId }
    let runId: String?
    let computerId: String?
    let approvalId: String
    /// "action" or "credential".
    let kind: String
    let credentialName: String?
    let credentialDomain: String?
    let message: String?

    var isCredential: Bool { kind == "credential" && credentialName != nil }
}

/// The bot needs the owner to do something (sign in, solve a puzzle, type a secret).
struct ACIHumanRequest: Sendable, Equatable {
    let runId: String?
    let reason: String
    /// Show a masked field: the owner types a secret that never leaves the device log.
    let secret: Bool
}

struct ACIRerunTarget: Sendable, Equatable, Identifiable {
    var id: String { computerId }
    let computerId: String
    let name: String
}

/// A site stopped the bot (login wall, 2FA, captcha, block page, rate limit).
struct ACIBlock: Sendable, Equatable {
    let runId: String?
    let domain: String
    /// login | two_factor | captcha | block_page | rate_limited
    let kind: String
    let rerunTargets: [ACIRerunTarget]
    let timeoutMs: Int?
}

enum ACIComputerEvent: Sendable, Equatable {
    case action(ACIAgentAction)
    case confirmationRequired(ACIApprovalRequest)
    case humanRequested(ACIHumanRequest)
    case humanResumed(runId: String?)
    case blockDetected(ACIBlock)
    /// Linux: this run has its own display; follow `screenshot?run_id=`.
    case agentDisplay(runId: String?)
    /// `action`: continue | stop | rerun | timed_out.
    case blockResolved(runId: String?, domain: String?, action: String?)

    /// Tolerant decode of one SSE `data:` payload; nil for frames this client
    /// does not use. Fields are read from `data` when the frame nests them,
    /// else from the top level.
    init?(json: Data) {
        guard let root = (try? JSONSerialization.jsonObject(with: json)) as? [String: Any] else { return nil }
        let d = (root["data"] as? [String: Any]) ?? root
        func str(_ k: String) -> String? { (d[k] as? String).flatMap { $0.isEmpty ? nil : $0 } }
        func num(_ k: String) -> Double? { (d[k] as? NSNumber)?.doubleValue }
        let type = (root["type"] as? String) ?? (d["type"] as? String)
            ?? (num("x") != nil && num("y") != nil ? "computer.action" : "")
        switch type {
        case "computer.action":
            guard let x = num("x"), let y = num("y") else { return nil }
            var actor: ACIAgentActor?
            if let a = d["actor"] as? [String: Any], let name = a["name"] as? String {
                actor = ACIAgentActor(agentId: a["agent_id"] as? String, name: name,
                                      avatarURL: a["avatar_url"] as? String, color: a["color"] as? String)
            }
            self = .action(ACIAgentAction(
                runId: str("run_id"), toolset: str("toolset"), member: str("member"), x: x, y: y,
                screenW: max(num("screen_w") ?? 0, 1), screenH: max(num("screen_h") ?? 0, 1),
                ok: (d["ok"] as? Bool) ?? true, actor: actor,
                step: (d["step"] as? NSNumber)?.intValue, rerunOf: str("rerun_of")))
        case "computer.confirmation_required":
            guard let approvalId = str("approval_id") else { return nil }
            let cred = d["credential"] as? [String: Any]
            self = .confirmationRequired(ACIApprovalRequest(
                runId: str("run_id"), computerId: str("computer_id"), approvalId: approvalId,
                kind: str("kind") ?? "action",
                credentialName: cred?["name"] as? String, credentialDomain: cred?["domain"] as? String,
                message: str("message")))
        case "computer.human_requested":
            let secret = (d["secret"] as? Bool) ?? (str("secret") != nil)
            self = .humanRequested(ACIHumanRequest(
                runId: str("run_id"),
                reason: str("reason") ?? str("message") ?? str("label") ?? str("secret") ?? "Your bot needs you.",
                secret: secret))
        case "computer.human_resumed":
            self = .humanResumed(runId: str("run_id"))
        case "computer.block_detected":
            guard let domain = str("domain") else { return nil }
            let targets = (d["rerun_targets"] as? [[String: Any]] ?? []).compactMap { t -> ACIRerunTarget? in
                guard let id = t["computer_id"] as? String else { return nil }
                return ACIRerunTarget(computerId: id, name: (t["name"] as? String) ?? id)
            }
            self = .blockDetected(ACIBlock(
                runId: str("run_id"), domain: domain, kind: str("kind") ?? "block_page",
                rerunTargets: targets, timeoutMs: (d["timeout_ms"] as? NSNumber)?.intValue))
        case "computer.agent_display":
            self = .agentDisplay(runId: str("run_id"))
        case "computer.block_resolved":
            self = .blockResolved(runId: str("run_id"), domain: str("domain"), action: str("action"))
        default:
            return nil
        }
    }
}
