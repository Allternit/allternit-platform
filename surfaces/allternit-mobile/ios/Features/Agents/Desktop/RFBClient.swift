import CoreGraphics
import Foundation

// -----------------------------------------------------------------------------
// Native RFB (VNC) client + UI-facing session for the bot desktop viewer.
//
// Server side: `GET /ws/bots/:bot_id/desktop/vnc?sandbox_id=…&user_id=…` on
// the allternit-api origin blindly proxies raw RFB 3.8 bytes over BINARY
// WebSocket frames to a sandbox `x11vnc -nopw`
// (cmd/allternit-api/src/bot_desktop_stream.rs). The web reference client is
// noVNC in surfaces/ai.allternit.com/src/views/bots/BotDesktopView.tsx.
//
// VIEW-ONLY ENFORCEMENT: the server forwards client input to the sandbox
// regardless of control_state (bot_desktop_stream.rs's ws→tcp task has no
// gating), so PointerEvent/KeyEvent are only sent while the store reports
// `human_controls` — enforced in `BotDesktopSession` (the view-model layer),
// which owns the latest BotDesktopStatus.ControlState.
// -----------------------------------------------------------------------------

/// The protocol client: an actor owning the WebSocket, the incremental
/// parser and the framebuffer. The receive loop suspends the actor at
/// `socket.receive()`, so `sendPointer`/`sendKey` interleave with parsing
/// without extra locking (repo concurrency idiom: one async receive loop,
/// cf. PtySession).
actor RFBClient {
    /// One published frame per processed FramebufferUpdate batch, plus the
    /// ServerInit geometry/name. Invoked on the client's actor context —
    /// handlers hop to the main actor themselves.
    struct Handlers: Sendable {
        var frame: @Sendable (CGImage) -> Void
        var serverInit: @Sendable (String, Int, Int) -> Void
        /// Any stream termination not requested via `disconnect()` — clean
        /// close, transport error, or protocol failure (`Error` nil on a
        /// clean peer close).
        var disconnected: @Sendable (Error?) -> Void
    }

    private let handlers: Handlers
    private let parser = RFBStreamParser()
    private var socket: URLSessionWebSocketTask?
    private var framebuffer: RFBFramebuffer?
    private var isIntentionallyClosed = false

    init(handlers: Handlers) {
        self.handlers = handlers
    }

    /// Absolutizes the PATH-ONLY `ws_url` from the desktop status response
    /// (`/ws/bots/...?sandbox_id=…&user_id=…`, bot_desktop_routes.rs:100-107)
    /// against the `AppConfig.apiBaseURL` origin, http→ws / https→wss —
    /// the same mapping the web's `wsUrlFromPath()` does against
    /// window.location (BotDesktopView.tsx:40-45). The query (including
    /// `user_id`, which the server checks against the Bearer identity,
    /// bot_desktop_stream.rs:42-44) is carried over verbatim.
    static func webSocketURL(wsPath: String) -> URL? {
        guard let base = URLComponents(url: AppConfig.apiBaseURL, resolvingAgainstBaseURL: false),
              let path = URLComponents(string: wsPath) else {
            return nil
        }
        var components = URLComponents()
        components.scheme = base.scheme == "https" ? "wss" : "ws"
        components.host = base.host
        components.port = base.port
        components.path = path.path
        // Keep the query percent-encoded verbatim so tokens with "+" or "/" survive.
        components.percentEncodedQuery = path.percentEncodedQuery
        return components.url
    }

    /// Connects and completes the RFB handshake: version (3.8), security
    /// (None for the sandbox's x11vnc `-nopw`; VNC password or Apple sign-in
    /// for a computer's own screen sharing, see `authenticate`), ClientInit(shared), ServerInit, then
    /// SetPixelFormat / SetEncodings / the initial full-screen
    /// FramebufferUpdateRequest. Throws a typed `RFBError` on any protocol
    /// violation; the caller (BotDesktopSession) owns retry.
    func connect(wsPath: String, credentials: RFBCredentials? = nil) async throws {
        guard let url = Self.webSocketURL(wsPath: wsPath) else {
            throw RFBError.invalidURL(wsPath)
        }
        var request = URLRequest(url: url)
        // The upgrade goes through the same auth middleware as REST — the
        // Bearer header is exactly why this is a native client and not a
        // WKWebView.
        if let token = try await AuthManager.shared.apiToken(), !token.isEmpty {
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        #if DEBUG
        // `-skip-auth` dev headers, mirroring APIClient:118-125 / PtyClient.
        if launchArgumentEnabled("skip-auth") {
            request.setValue("dev-ios-tester", forHTTPHeaderField: "x-allternit-user-id")
            request.setValue("dev", forHTTPHeaderField: "x-allternit-desktop-access-token")
            if request.value(forHTTPHeaderField: "Authorization") == nil {
                request.setValue("Bearer dev-api-token", forHTTPHeaderField: "Authorization")
            }
        }
        #endif
        let task = URLSession.shared.webSocketTask(with: request)
        socket = task
        task.resume()
        try await performHandshake(credentials: credentials)
    }

    /// The message loop: FramebufferUpdate → apply each rectangle → publish
    /// one CGImage per batch → request the next incremental update. Bell and
    /// ServerCutText carry nothing the viewer needs; SetColourMapEntries can
    /// never apply (true color is negotiated). Runs until the stream ends;
    /// any non-intentional termination is reported via `handlers.disconnected`.
    func run() async {
        do {
            while true {
                let message = try await nextMessage()
                switch message {
                case .framebufferUpdate(let rects):
                    guard let framebuffer else {
                        throw RFBError.unexpectedMessage("FramebufferUpdate before ServerInit")
                    }
                    for rect in rects {
                        try framebuffer.apply(rect)
                    }
                    if let image = framebuffer.makeImage() {
                        handlers.frame(image)
                    }
                    let request = RFBClientMessage.framebufferUpdateRequest(
                        incremental: true,
                        width: UInt16(framebuffer.width),
                        height: UInt16(framebuffer.height)
                    )
                    try await send(request)
                case .setColourMapEntries, .bell, .serverCutText:
                    continue
                case .protocolVersion, .securityTypes, .securityFailure, .serverInit:
                    throw RFBError.unexpectedMessage("handshake message after ServerInit")
                }
            }
        } catch {
            socket?.cancel(with: .normalClosure, reason: nil)
            socket = nil
            if !isIntentionallyClosed {
                handlers.disconnected(error)
            }
        }
    }

    /// RFB PointerEvent. Callable whenever connected — the VIEW-ONLY gate
    /// lives in BotDesktopSession (see the file header).
    func sendPointer(x: UInt16, y: UInt16, buttonMask: UInt8) async throws {
        try await send(RFBClientMessage.pointerEvent(buttonMask: buttonMask, x: x, y: y))
    }

    /// RFB KeyEvent (down and up are separate messages).
    func sendKey(keysym: UInt32, down: Bool) async throws {
        try await send(RFBClientMessage.keyEvent(keysym: keysym, down: down))
    }

    /// User-initiated close: no reconnect, no `disconnected` callback.
    func disconnect() {
        isIntentionallyClosed = true
        socket?.cancel(with: .normalClosure, reason: nil)
        socket = nil
    }

    // MARK: - Handshake

    private func performHandshake(credentials: RFBCredentials?) async throws {
        let versionMessage = try await nextMessage()
        guard case .protocolVersion(let line) = versionMessage else {
            throw RFBError.unexpectedMessage("expected ProtocolVersion, got \(versionMessage)")
        }
        try validateProtocolVersion(line)
        try await send(RFBClientMessage.protocolVersion())

        let securityMessage = try await nextMessage()
        switch securityMessage {
        case .securityFailure(let reason):
            throw RFBError.securityHandshakeFailed(reason)
        case .securityTypes(let types):
            guard let type = RFBAuth.supportedTypes.first(where: types.contains) else {
                throw RFBError.noSupportedSecurityType(types)
            }
            try await authenticate(type: type, credentials: credentials)
        default:
            throw RFBError.unexpectedMessage("expected Security, got \(securityMessage)")
        }
        try await send(RFBClientMessage.clientInit())

        let initMessage = try await nextMessage()
        guard case .serverInit(let info) = initMessage else {
            throw RFBError.unexpectedMessage("expected ServerInit, got \(initMessage)")
        }
        framebuffer = try RFBFramebuffer(width: info.width, height: info.height)
        handlers.serverInit(info.name, info.width, info.height)
        try await send(RFBClientMessage.setPixelFormat())
        try await send(RFBClientMessage.setEncodings())
        try await send(RFBClientMessage.framebufferUpdateRequest(
            incremental: false,
            width: UInt16(info.width),
            height: UInt16(info.height)
        ))
    }

    /// Answers the chosen security type, then reads SecurityResult (sent for
    /// every type in 3.8, None included). Without credentials for a type that
    /// needs them this throws `credentialsRequired` before selecting it, so
    /// the server never counts a failed attempt.
    private func authenticate(type: UInt8, credentials: RFBCredentials?) async throws {
        switch type {
        case 30:
            guard let credentials, !credentials.username.isEmpty else {
                throw RFBError.credentialsRequired(needsUsername: true)
            }
            try await send(RFBClientMessage.securitySelection(30))
            let header = try await nextBytes(4)
            let generator = Array(header[0..<2])
            let keyLength = Int(header[2]) << 8 | Int(header[3])
            let prime = try await nextBytes(keyLength)
            let serverKey = try await nextBytes(keyLength)
            guard let response = RFBAuth.ardResponse(
                generator: generator, prime: prime, serverPublicKey: serverKey, credentials: credentials
            ) else {
                throw RFBError.securityHandshakeFailed("the computer sent an unusable key")
            }
            try await send(Data(response))
        case 2:
            guard let credentials, !credentials.password.isEmpty else {
                throw RFBError.credentialsRequired(needsUsername: false)
            }
            try await send(RFBClientMessage.securitySelection(2))
            let challenge = try await nextBytes(16)
            guard let response = RFBAuth.vncResponse(challenge: challenge, password: credentials.password) else {
                throw RFBError.securityHandshakeFailed("couldn't answer the password challenge")
            }
            try await send(Data(response))
        default:
            try await send(RFBClientMessage.securitySelectionNone())
        }

        let result = try await nextBytes(4)
        guard result != [0, 0, 0, 0] else { return }
        // Failure carries a reason string in 3.8 (some servers close instead).
        var reason = type == 1 ? "The computer refused the connection." : "Wrong user name or password."
        if let lengthBytes = try? await nextBytes(4) {
            let length = lengthBytes.reduce(0) { $0 << 8 | Int($1) }
            if length > 0, length < 4096, let text = try? await nextBytes(length),
               let message = String(bytes: text, encoding: .utf8), !message.isEmpty {
                reason = message
            }
        }
        if type == 1 { throw RFBError.securityHandshakeFailed(reason) }
        throw RFBError.authenticationFailed(reason)
    }

    /// "RFB 003.008\n" — the sandbox's x11vnc always greets with 3.8; we
    /// answer 3.8 and refuse anything older rather than silently degrading.
    private func validateProtocolVersion(_ line: String) throws {
        let digits = line.dropFirst(4).dropLast(1) // "003.008"
        let parts = digits.split(separator: ".")
        guard parts.count == 2,
              let major = Int(parts[0]), let minor = Int(parts[1]) else {
            throw RFBError.invalidProtocolVersion(line)
        }
        guard major == 3, minor >= 8 else {
            throw RFBError.unsupportedProtocolVersion(line)
        }
    }

    // MARK: - Transport

    /// The next complete server message, receiving WebSocket frames until
    /// the parser has enough bytes (frame boundaries are arbitrary — the
    /// proxy forwards 16 KB TCP reads, bot_desktop_stream.rs:173).
    private func nextMessage() async throws -> RFBServerMessage {
        guard let socket else { throw RFBError.notConnected }
        while true {
            if let message = try parser.nextMessage() {
                return message
            }
            let wsMessage = try await socket.receive()
            switch wsMessage {
            case .data(let data):
                parser.append(data)
            case .string(let text):
                // The proxy only ever sends binary frames; a text frame is
                // still RFB bytes on the wire, so parse it rather than drop.
                parser.append(Data(text.utf8))
            @unknown default:
                continue
            }
        }
    }

    /// Exactly `count` raw handshake bytes (see `RFBStreamParser.takeBytes`).
    private func nextBytes(_ count: Int) async throws -> [UInt8] {
        guard let socket else { throw RFBError.notConnected }
        while true {
            if let bytes = parser.takeBytes(count) { return bytes }
            switch try await socket.receive() {
            case .data(let data): parser.append(data)
            case .string(let text): parser.append(Data(text.utf8))
            @unknown default: continue
            }
        }
    }

    /// One RFB client message per binary WebSocket frame — the proxy writes
    /// each frame to the TCP socket verbatim, preserving boundaries.
    private func send(_ data: Data) async throws {
        guard let socket else { throw RFBError.notConnected }
        try await socket.send(.data(data))
    }
}

// MARK: - UI-facing session

/// Main-actor owner of one `RFBClient`, in the PtySession idiom: published
/// state for the SwiftUI view, reconnect with capped exponential backoff,
/// and the VIEW-ONLY input gate (see the file header — the server forwards
/// input regardless of control_state, so nothing goes out unless the store
/// says `human_controls`).
@MainActor
final class BotDesktopSession: ObservableObject {
    enum State: Equatable {
        case connecting
        case live
        /// Mid-backoff; the attempt number drives the overlay copy.
        case reconnecting(attempt: Int)
        /// Backoff exhausted (or a handshake-level failure) — the view
        /// shows the error plus a manual Retry.
        case failed(String)
        /// The computer's screen sharing wants a sign-in: the view shows the
        /// form (`message` is set after a wrong attempt) and calls
        /// `submitCredentials`.
        case needsCredentials(needsUsername: Bool, message: String?)
    }

    @Published private(set) var state: State = .connecting
    /// Latest framebuffer snapshot; nil until the first FramebufferUpdate.
    @Published private(set) var frame: CGImage? = nil
    /// ServerInit desktop name (x11vnc's X display name) for the toolbar.
    @Published private(set) var desktopName: String = ""
    @Published private(set) var framebufferSize: CGSize = .zero

    /// Latest control state, pushed in by the view from BotDesktopStore.
    /// THE input gate: pointer/keyboard events leave the device only while
    /// this is `.humanControls`.
    var controlState: BotDesktopStatus.ControlState

    var allowsInput: Bool { controlState == .humanControls }

    /// Backoff: 1s → 2s → 4s → … capped at 30s, at most this many attempts
    /// before the session gives up and shows the error.
    static let maxReconnectAttempts = 8

    private let wsPath: String
    private var client: RFBClient?
    private var runTask: Task<Void, Never>?
    private var reconnectTask: Task<Void, Never>?
    private var reconnectAttempt = 0
    private var intentionalClose = false
    /// The last sign-in that worked (or is being tried), reused for
    /// reconnects; memory only.
    private var credentials: RFBCredentials?

    init(wsPath: String, controlState: BotDesktopStatus.ControlState) {
        self.wsPath = wsPath
        self.controlState = controlState
    }

    /// Connects (idempotent while a client exists). Called from the view's
    /// `.task`.
    func start() {
        guard client == nil else { return }
        intentionalClose = false
        reconnectAttempt = 0
        connect()
    }

    /// Tears down without reconnecting (sheet dismissed).
    func stop() {
        intentionalClose = true
        reconnectTask?.cancel()
        reconnectTask = nil
        runTask?.cancel()
        runTask = nil
        let client = self.client
        self.client = nil
        if let client {
            Task { await client.disconnect() }
        }
    }

    /// Manual retry from the failed overlay: full reset + reconnect.
    func retry() {
        stop()
        intentionalClose = false
        reconnectAttempt = 0
        state = .connecting
        connect()
    }

    /// Connects again with the sign-in from the credentials form.
    func submitCredentials(username: String, password: String) {
        credentials = RFBCredentials(username: username, password: password)
        retry()
    }

    // MARK: - Input (gated — see file header)

    /// RFB PointerEvent; `buttonMask` bit 0 = left, 1 = middle, 2 = right.
    /// No-op while observing or before the stream is live.
    func sendPointer(x: Int, y: Int, buttonMask: UInt8) {
        guard allowsInput, state == .live, let client else { return }
        let x = UInt16(clamping: x), y = UInt16(clamping: y)
        Task { [weak self] in
            do {
                try await client.sendPointer(x: x, y: y, buttonMask: buttonMask)
            } catch {
                self?.handleStreamFailure(error)
            }
        }
    }

    /// RFB KeyEvent (down and up sent separately by the caller).
    func sendKey(keysym: UInt32, down: Bool) {
        guard allowsInput, state == .live, let client else { return }
        Task { [weak self] in
            do {
                try await client.sendKey(keysym: keysym, down: down)
            } catch {
                self?.handleStreamFailure(error)
            }
        }
    }

    /// Tap convenience: full press (down+up) of one button at a point.
    func tap(x: Int, y: Int, buttonMask: UInt8) {
        sendPointer(x: x, y: y, buttonMask: buttonMask)
        sendPointer(x: x, y: y, buttonMask: 0)
    }

    // MARK: - Connection lifecycle

    private func connect() {
        state = reconnectAttempt > 0 ? .reconnecting(attempt: reconnectAttempt) : .connecting
        let client = RFBClient(handlers: RFBClient.Handlers(
            frame: { [weak self] image in
                Task { @MainActor in self?.frame = image }
            },
            serverInit: { [weak self] name, width, height in
                Task { @MainActor in
                    self?.desktopName = name
                    self?.framebufferSize = CGSize(width: width, height: height)
                }
            },
            disconnected: { [weak self] error in
                Task { @MainActor in
                    self?.handleStreamFailure(error ?? RFBError.notConnected)
                }
            }
        ))
        self.client = client
        let wsPath = self.wsPath
        let credentials = self.credentials
        Task { [weak self] in
            do {
                try await client.connect(wsPath: wsPath, credentials: credentials)
                guard let self, !Task.isCancelled, !self.intentionalClose else { return }
                self.reconnectAttempt = 0
                self.state = .live
                self.runTask = Task { await client.run() }
            } catch is CancellationError {
                // Sheet dismissed mid-handshake — stop() owns the teardown.
            } catch {
                self?.handleStreamFailure(error)
            }
        }
    }

    /// Any unexpected stream end: reconnect with capped exponential backoff
    /// (1s→2s→…→30s, `maxReconnectAttempts` tries) or surface the failure
    /// with a manual Retry. Intentional closes (sheet dismissed) never reach
    /// here — `disconnect()` suppresses the client's callback.
    private func handleStreamFailure(_ error: Error) {
        guard !intentionalClose else { return }
        runTask?.cancel()
        runTask = nil
        let oldClient = client
        client = nil
        if let oldClient {
            Task { await oldClient.disconnect() }
        }
        // A sign-in problem doesn't get better by retrying: ask the person.
        switch error as? RFBError {
        case .credentialsRequired(let needsUsername):
            state = .needsCredentials(needsUsername: needsUsername, message: nil)
            return
        case .authenticationFailed(let reason):
            let needsUsername = !(credentials?.username.isEmpty ?? true)
            credentials = nil
            state = .needsCredentials(needsUsername: needsUsername, message: reason)
            return
        default:
            break
        }
        guard reconnectAttempt < Self.maxReconnectAttempts else {
            state = .failed(error.localizedDescription)
            return
        }
        reconnectAttempt += 1
        state = .reconnecting(attempt: reconnectAttempt)
        let delay = min(30, pow(2.0, Double(reconnectAttempt - 1)))
        reconnectTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(delay))
            guard let self, !Task.isCancelled, !self.intentionalClose else { return }
            self.connect()
        }
    }
}
