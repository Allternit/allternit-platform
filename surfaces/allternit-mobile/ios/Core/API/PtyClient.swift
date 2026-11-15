import Foundation

/// Pty session info returned by gizzi-code's `POST /v1/pty/create`
/// (integrations/pty/index.ts `Info`).
struct PtyInfo: Decodable, Sendable {
    let id: String
    let title: String
    let command: String
    let args: [String]
    let cwd: String
    let status: String
    let pid: Int
}

/// Client for the standalone gizzi-code server's pty routes — REST plus the
/// WebSocket attach (cmd/gizzi-code/docs/pty-websocket-protocol.md).
///
/// The target comes from `InstanceConnection`: an explicit/tunnel URL or the
/// mesh proxy is dialled directly; the relay to the user's computer carries
/// REST in the proxy envelope and the attach over a relay socket ticket
/// (`RuntimeRelayClient.webSocket`), as on m.allternit.com.
final class PtyClient: @unchecked Sendable {
    static let shared = PtyClient()

    let baseURL: URL
    private let endpointBaseURL: URL

    init(baseURL: URL = AppConfig.gizziCodeBaseURL) {
        self.baseURL = baseURL
        self.endpointBaseURL = baseURL
    }

    /// Per-call target: the given base URL, or — for the static default —
    /// whatever `InstanceConnection.resolve()` picks now (explicit URL →
    /// mesh → the relay to the user's computer).
    private func client() async throws -> APIClient {
        try await InstanceConnection.gizziClient(for: endpointBaseURL)
    }

    // MARK: - REST

    /// `POST /v1/pty/create`. The server's `CreateInput` has no size field —
    /// mux panes are born 80x24 (integrations/pty/index.ts:335-339) and are
    /// resized afterwards via `resize(id:cols:rows:)`.
    func createPty(title: String? = nil) async throws -> PtyInfo {
        try await client().post(path: "v1/pty/create", body: CreatePtyRequest(title: title))
    }

    /// `PUT /v1/pty/:id` — `{ "size": { "rows": R, "cols": C } }`. Resize is
    /// REST-only; it does NOT go over the socket (protocol doc §Client→server).
    func resize(id: String, cols: Int, rows: Int) async throws {
        try await client().put(
            path: "v1/pty/\(Self.escape(id))",
            body: UpdatePtyRequest(size: .init(rows: rows, cols: cols))
        )
    }

    /// `DELETE /v1/pty/:id` — kills the pty server-side. Detaching (closing
    /// the WebSocket) leaves it alive; only call this to end the shell.
    func deletePty(id: String) async throws {
        try await client().delete(path: "v1/pty/\(Self.escape(id))")
    }

    // MARK: - WebSocket

    /// Builds the authorized WebSocket task for `GET /v1/pty/:id/connect`
    /// (not yet resumed). The upgrade is an ordinary HTTP request under the
    /// same auth middleware as REST (protocol doc §Auth): Clerk Bearer, plus
    /// the DEBUG `-skip-auth` dev headers mirroring APIClient:118-125.
    func connectWebSocket(ptyId: String) async throws -> URLSessionWebSocketTask {
        let base = try await InstanceConnection.gizziBaseURL(for: endpointBaseURL)
        let path = "v1/pty/\(Self.escape(ptyId))/connect"
        if RuntimeRelayClient.isRelayURL(base) {
            // Relayed: the ticket carries auth; the task comes back resumed
            // and past the tunnel's ready frame.
            return try await InstanceConnection.webSocketTask(
                for: URLRequest(url: base.appendingPathComponent(path))
            )
        }
        guard var components = URLComponents(
            url: base.appendingPathComponent(path),
            resolvingAgainstBaseURL: false
        ) else {
            throw APIError.invalidResponse
        }
        components.scheme = components.scheme == "https" ? "wss" : "ws"
        guard let url = components.url else {
            throw APIError.invalidResponse
        }

        var request = URLRequest(url: url)
        if let token = try await AuthManager.shared.getToken(), !token.isEmpty {
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        #if DEBUG
        if launchArgumentEnabled("skip-auth") {
            request.setValue("dev-ios-tester", forHTTPHeaderField: "x-allternit-user-id")
            request.setValue("dev", forHTTPHeaderField: "x-allternit-desktop-access-token")
            // Dev bearer comes from the environment (see APIClient.swift) —
            // it is no longer hardcoded (B1).
            if request.value(forHTTPHeaderField: "Authorization") == nil,
               let devToken = ProcessInfo.processInfo.environment["ALLTERNIT_DEV_API_TOKEN"] {
                request.setValue("Bearer \(devToken)", forHTTPHeaderField: "Authorization")
            }
        }
        #endif
        return URLSession.shared.webSocketTask(with: request)
    }

    /// Web uses `encodeURIComponent` on path ids (parity with AgentChatClient).
    private static func escape(_ ptyId: String) -> String {
        ptyId.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? ptyId
    }
}

private struct CreatePtyRequest: Encodable {
    let title: String?
}

private struct UpdatePtyRequest: Encodable {
    struct Size: Encodable {
        let rows: Int
        let cols: Int
    }
    let size: Size
}

/// Lifecycle owner for one pty attachment: creates the pty, pumps the
/// WebSocket, classifies frames, and gates input on the meta frame.
///
/// UI-facing state is main-actor published; the receive loop suspends at
/// `socket.receive()` and never blocks the main thread. The pty is never
/// deleted here — sessions are owned by the server's mux daemon and survive
/// detaches, so the chat↔terminal flip keeps the shell alive.
@MainActor
final class PtySession: ObservableObject {
    enum State: Equatable {
        case connecting
        /// Meta frame received: replay done, input hooked up.
        case live
        /// Server closed the socket (the pty's process exited).
        case exited
        case failed(String)
    }

    @Published private(set) var state: State = .connecting

    /// Name of the registered gizzi instance this session attaches to
    /// (InstanceStore), shown by the terminal view while connecting; nil when
    /// attached to the static `AppConfig.gizziCodeBaseURL` default.
    let attachedInstanceName: String?

    /// Raw terminal output sink (ANSI included), invoked on the main actor.
    /// Assigned by the terminal view; cleared on `stop()`.
    var onOutput: (@MainActor (String) -> Void)?

    private let client: PtyClient
    /// Set for a relayed computer: the shell lives in its terminal service
    /// (`/terminal/*`, streamed over SSE) instead of a gizzi-code pty.
    private let service: TerminalServiceClient?
    private(set) var ptyId: String?
    private var readyForInput = false
    private var intentionalClose = false
    private var lastResize: String?
    /// Last grid size reported by the terminal view, kept so it can be sent
    /// (or re-sent) once the session is live — the first `sizeChanged` fires
    /// during layout, before `createPty` returns, so `resize` has no ptyId
    /// yet and would otherwise drop the only resize the view ever reports.
    private var pendingSize: (cols: Int, rows: Int)?

    // `nonisolated(unsafe)` so the (non-isolated, Swift 6.0) deinit can tear
    // the connection down; both are only mutated on the main actor otherwise.
    nonisolated(unsafe) private var webSocket: URLSessionWebSocketTask?
    nonisolated(unsafe) private var receiveTask: Task<Void, Never>?
    nonisolated(unsafe) private var streamTask: Task<Void, Never>?
    /// Keystrokes typed while an input POST is in flight; they go out
    /// together in the next one, so fast typing costs one round trip, not
    /// one per key queued behind each other.
    private var pendingInput = ""
    private var isSendingInput = false

    init(client: PtyClient = .shared, attachedInstanceName: String? = nil) {
        self.client = client
        self.service = nil
        self.attachedInstanceName = attachedInstanceName
    }

    init(service: TerminalServiceClient, attachedInstanceName: String? = nil) {
        self.client = .shared
        self.service = service
        self.attachedInstanceName = attachedInstanceName
    }

    deinit {
        receiveTask?.cancel()
        streamTask?.cancel()
        webSocket?.cancel(with: .goingAway, reason: nil)
    }

    /// Creates the pty and attaches the WebSocket. No-op while attached.
    /// After a detach (e.g. flipping to chat and back), the same pty is
    /// re-attached and the server replays the full scrollback, restoring
    /// the terminal screen (protocol: reconnecting to a live ptyID replays).
    func start() async {
        if let service {
            await startService(service)
            return
        }
        guard webSocket == nil else { return }
        state = .connecting
        intentionalClose = false
        readyForInput = false
        lastResize = nil
        do {
            if ptyId == nil {
                let info = try await client.createPty(title: "iOS Terminal")
                ptyId = info.id
            }
            guard let ptyId else { return }
            let socket = try await client.connectWebSocket(ptyId: ptyId)
            webSocket = socket
            socket.resume()
            startReceiveLoop(socket)
        } catch {
            guard !Task.isCancelled else { return }
            state = .failed(error.localizedDescription)
        }
    }

    /// Tears down a failed/exited session and attaches a fresh pty.
    func retry() async {
        stop()
        ptyId = nil
        await start()
    }

    /// Writes user input to the pty stdin (text frames, verbatim). Gated on
    /// the meta frame per protocol — input is only hooked up server-side once
    /// the replay completed, so keystrokes arriving earlier are dropped here.
    func send(_ text: String) {
        if let service {
            guard readyForInput, let ptyId else { return }
            pendingInput += text
            flushServiceInput(service, ptyId: ptyId)
            return
        }
        guard readyForInput, let webSocket else { return }
        webSocket.send(.string(text)) { _ in }
    }

    /// Best-effort REST resize, deduped (SwiftTerm reports `sizeChanged` on
    /// every layout pass). The size is remembered even before the pty exists
    /// (see `pendingSize`), so the meta-frame handler can send it once live.
    /// Failures clear the dedupe key so a later identical report retries.
    func resize(cols: Int, rows: Int) {
        guard cols > 0, rows > 0 else { return }
        pendingSize = (cols, rows)
        sendResize(cols: cols, rows: rows)
    }

    private func sendResize(cols: Int, rows: Int) {
        guard let ptyId else { return }
        let key = "\(cols)x\(rows)"
        guard key != lastResize else { return }
        lastResize = key
        Task { [client, service, weak self] in
            do {
                if let service {
                    try await service.resize(sessionId: ptyId, cols: cols, rows: rows)
                } else {
                    try await client.resize(id: ptyId, cols: cols, rows: rows)
                }
            } catch {
                if self?.lastResize == key { self?.lastResize = nil }
            }
        }
    }

    /// Detaches from the pty. The pty itself keeps running server-side
    /// (protocol doc §Close semantics) — `PtyClient.deletePty` would kill it.
    func stop() {
        intentionalClose = true
        receiveTask?.cancel()
        receiveTask = nil
        streamTask?.cancel()
        streamTask = nil
        webSocket?.cancel(with: .normalClosure, reason: nil)
        webSocket = nil
        onOutput = nil
    }

    /// Ends the shell on the computer (closing a terminal tab).
    func close() {
        stop()
        guard let ptyId else { return }
        Task { [client, service] in
            if let service {
                try? await service.close(sessionId: ptyId)
            } else {
                try? await client.deletePty(id: ptyId)
            }
        }
    }

    // MARK: Terminal service (relayed computer)

    /// Creates the shell (sized to the view when known) and reads its SSE
    /// stream; input opens once the stream does (UnifiedTerminal onOpen).
    private func startService(_ service: TerminalServiceClient) async {
        guard streamTask == nil else { return }
        state = .connecting
        intentionalClose = false
        readyForInput = false
        lastResize = nil
        do {
            if ptyId == nil {
                ptyId = try await service.create(cols: pendingSize?.cols ?? 80, rows: pendingSize?.rows ?? 24)
            }
        } catch {
            guard !Task.isCancelled else { return }
            state = .failed(error.localizedDescription)
            return
        }
        guard let ptyId else { return }
        streamTask = Task { [weak self] in
            do {
                let bytes = try await service.stream(sessionId: ptyId)
                guard let self else { return }
                self.readyForInput = true
                self.state = .live
                if let pendingSize = self.pendingSize {
                    self.lastResize = nil
                    self.sendResize(cols: pendingSize.cols, rows: pendingSize.rows)
                }
                // Read and decode off the main thread; only printable text
                // hops back to the terminal.
                let reader = Task.detached { [weak self] in
                    for try await line in bytes.lines {
                        guard line.hasPrefix("data:") else { continue }
                        var payload = String(line.dropFirst(5))
                        if payload.hasPrefix(" ") { payload.removeFirst() }
                        guard let text = Self.serviceOutput(payload) else { continue }
                        await MainActor.run { self?.onOutput?(text) }
                    }
                }
                try await withTaskCancellationHandler {
                    try await reader.value
                } onCancel: {
                    reader.cancel()
                }
                guard !Task.isCancelled, !self.intentionalClose else { return }
                self.state = .failed("Terminal stream ended")
            } catch {
                guard !Task.isCancelled, let self, !self.intentionalClose else { return }
                self.state = .failed(error.localizedDescription)
            }
            self?.streamTask = nil
        }
    }

    /// Sends what was typed, in order, one POST at a time.
    private func flushServiceInput(_ service: TerminalServiceClient, ptyId: String) {
        guard !isSendingInput, !pendingInput.isEmpty else { return }
        let text = pendingInput
        pendingInput = ""
        isSendingInput = true
        Task { [weak self] in
            do {
                try await service.input(sessionId: ptyId, text)
            } catch {
                guard !Task.isCancelled else { return }
                self?.state = .failed(error.localizedDescription)
            }
            guard let self else { return }
            self.isSendingInput = false
            self.flushServiceInput(service, ptyId: ptyId)
        }
    }

    /// The text an SSE `data:` payload prints, or nil. Like the web: any JSON
    /// message is handled by its type (only `data` prints; keep-alives such
    /// as `{}` don't); text that isn't JSON prints as is.
    nonisolated private static func serviceOutput(_ payload: String) -> String? {
        if let data = payload.data(using: .utf8),
           let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] {
            if object["type"] as? String == "data", let text = object["data"] as? String, !text.isEmpty { return text }
            return nil
        }
        return payload.isEmpty ? nil : payload
    }

    private func startReceiveLoop(_ socket: URLSessionWebSocketTask) {
        receiveTask = Task { [weak self] in
            while !Task.isCancelled {
                let message: URLSessionWebSocketTask.Message
                do {
                    message = try await socket.receive()
                } catch {
                    // Peer closed (pty process exited) or the socket dropped.
                    if !Task.isCancelled {
                        self?.connectionEnded()
                    }
                    return
                }
                guard let self else { return }
                self.handle(message)
            }
        }
    }

    private func handle(_ message: URLSessionWebSocketTask.Message) {
        switch message {
        case .string(let text):
            // Scrollback replay and live output are both raw UTF-8 text frames.
            onOutput?(text)
        case .data(let data):
            // The only server→client binary frame is the meta frame: 0x00 +
            // UTF-8 JSON. It must never be rendered; it marks the end of the
            // replay. `cursor` is informational (server-side resume isn't
            // implemented) and unknown meta keys are ignored.
            guard data.first == 0x00 else { return }
            readyForInput = true
            state = .live
            // The first `sizeChanged` fired during layout, before the pty
            // existed, so its resize was dropped (no ptyId) and the size
            // never changes again to re-trigger it — leaving the pty at the
            // server's 80x24 default. Re-send the reported size now that
            // the attach completed; forcing past the dedupe key also covers
            // a resize whose REST call raced or failed mid-handshake.
            if let pendingSize {
                lastResize = nil
                sendResize(cols: pendingSize.cols, rows: pendingSize.rows)
            }
        @unknown default:
            break
        }
    }

    private func connectionEnded() {
        guard !intentionalClose else { return }
        
        let wasExitedCleanly = webSocket?.closeCode != .invalid && webSocket?.closeCode != .abnormalClosure
        
        if readyForInput, !wasExitedCleanly, let ptyId {
            state = .connecting
            readyForInput = false
            Task {
                var attempt = 0
                while attempt < 5 && !intentionalClose && !Task.isCancelled {
                    attempt += 1
                    do {
                        let socket = try await client.connectWebSocket(ptyId: ptyId)
                        webSocket = socket
                        socket.resume()
                        startReceiveLoop(socket)
                        return // Reconnection initiated!
                    } catch {
                        // Transport/network error, keep retrying
                    }
                    try? await Task.sleep(for: .seconds(Double(attempt) * 1.5))
                }
                
                state = .exited
            }
        } else {
            // A closed socket mid-replay means the attach failed; after the meta
            // frame it means the pty's process exited (protocol doc §Close).
            state = readyForInput ? .exited : .failed("Connection closed by the server.")
        }
    }
}
