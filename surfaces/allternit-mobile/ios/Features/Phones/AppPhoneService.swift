import CryptoKit
import Foundation
import UIKit

/// "Let your bots use this iPhone" (SPEC-iphone-app-screen §3): pairing this
/// phone with one of the owner's Desktops, and the control socket the app
/// keeps open while it is in the foreground so bots can drive the ACI
/// browser and ask the person to share the screen.
///
/// Everything goes out through api.allternit.com; the phone never listens.
/// The cloud only relays sealed bytes: the session key is derived on the
/// phone and on the Desktop and never leaves either.
@MainActor
final class AppPhoneService: ObservableObject {
    static let shared = AppPhoneService()

    enum PairStep: Equatable {
        case idle
        /// `POST /api/v1/app-phones` + opening the pair socket.
        case registering
        /// Waiting for the person to approve on the Desktop. `code` is nil
        /// until the Desktop's public key arrives.
        case waitingForDesktop(code: String?)
        case failed(String)
    }

    @Published private(set) var credentials: AppPhoneCredentials? = AppPhoneKeychain.load()
    @Published private(set) var pairStep: PairStep = .idle
    /// The control socket is open (bots can reach the app right now).
    @Published private(set) var isControlConnected = false
    /// The broadcast extension is live (heartbeat fresh).
    @Published private(set) var isScreenLive = AppPhoneSharedState.isLive
    /// A viewer is attached to the live screen.
    @Published private(set) var isScreenWatched = AppPhoneSharedState.isWatched
    /// A bot asked the person to start sharing (`screen.prompt`).
    @Published var isScreenPromptPresented = false

    private var pairTask: Task<Void, Never>?
    private var pairSocket: URLSessionWebSocketTask?

    private var isForeground = false
    private var controlTask: Task<Void, Never>?
    private var controlSocket: URLSessionWebSocketTask?
    /// Nonces of node messages already accepted, so a replayed request inside
    /// the ±120 s window is refused (the stream cipher must never see a
    /// nonce twice either).
    private var seenNonces: [String: Date] = [:]
    private var screenPoll: Timer?

    var isPaired: Bool { credentials != nil }

    private init() {
        CFNotificationCenterAddObserver(
            CFNotificationCenterGetDarwinNotifyCenter(), nil,
            { _, _, _, _, _ in Task { @MainActor in AppPhoneService.shared.refreshScreenState() } },
            AppPhoneSharedState.changedNotification as CFString, nil, .deliverImmediately)
    }

    // MARK: - Pairing

    /// Registers this phone for `runtime` and runs the pair handshake: the
    /// person sees a 6-digit code here and the same code on the Desktop, and
    /// approves there.
    func pair(with runtime: RuntimeDevice, name: String) {
        cancelPairing()
        pairStep = .registering
        pairTask = Task { [weak self] in
            do {
                let creds = try await Self.runPairing(runtime: runtime, name: name) { [weak self] socket in
                    self?.pairSocket = socket
                } onCode: { [weak self] code in
                    self?.pairStep = .waitingForDesktop(code: code)
                }
                guard let self, !Task.isCancelled else { return }
                AppPhoneKeychain.save(creds)
                self.credentials = creds
                self.pairStep = .idle
                self.pairSocket = nil
                self.restartControl()
            } catch is CancellationError {
                // cancelPairing() already reset the step.
            } catch {
                guard let self, !Task.isCancelled else { return }
                self.pairStep = .failed(error.localizedDescription)
                self.pairSocket = nil
            }
        }
    }

    func cancelPairing() {
        pairTask?.cancel()
        pairTask = nil
        pairSocket?.cancel(with: .goingAway, reason: nil)
        pairSocket = nil
        pairStep = .idle
    }

    /// Revokes the app phone key on the cloud and forgets the session key.
    /// The phone id stays, so pairing again reuses `app:<id>`.
    func unpair() async {
        let creds = credentials
        DeviceAccessStore.shared.disableAll()
        stopControl()
        AppPhoneSharedState.post(AppPhoneSharedState.stopNotification)
        AppPhoneKeychain.clear()
        credentials = nil
        guard let creds else { return }
        let id = creds.phoneId.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? creds.phoneId
        if var request = try? await Self.clerkRequest(path: "api/v1/app-phones/\(id)") {
            request.httpMethod = "DELETE"
            _ = try? await URLSession.shared.data(for: request)
        }
    }

    private struct RegisterBody: Encodable {
        let phoneId: String
        let name: String
        let runtimeDeviceId: String
    }

    private struct RegisterResponse: Decodable {
        let appPhoneKey: String
    }

    private enum PairFailure: LocalizedError {
        case denied, timedOut, closed, keyMismatch, notSignedIn

        var errorDescription: String? {
            switch self {
            case .denied: return "The Desktop declined this iPhone."
            case .timedOut: return "Nobody approved on the Desktop in time. Try again."
            case .closed: return "The Desktop closed the connection. Check that Allternit Desktop is open."
            case .keyMismatch: return "The Desktop's key changed during pairing. Try again."
            case .notSignedIn: return "Sign in with your Allternit account first."
            }
        }
    }

    private static let approvalTimeout: TimeInterval = 120

    private static func runPairing(runtime: RuntimeDevice, name: String,
                                   onSocket: @escaping @MainActor (URLSessionWebSocketTask) -> Void,
                                   onCode: @escaping @MainActor (String?) -> Void) async throws -> AppPhoneCredentials {
        let phoneId = AppPhoneKeychain.phoneId()
        let cloud = AppConfig.cloudAPIBaseURL

        // 1. Register (or rotate) this phone's key with the cloud (Clerk).
        var request = try await clerkRequest(path: "api/v1/app-phones")
        request.httpMethod = "POST"
        request.httpBody = try JSONEncoder().encode(RegisterBody(phoneId: phoneId, name: name, runtimeDeviceId: runtime.id))
        let (data, response) = try await URLSession.shared.data(for: request)
        try APIClient.shared.validate(response, data: data)
        let appPhoneKey = try JSONDecoder().decode(RegisterResponse.self, from: data).appPhoneKey

        // 2. Pair socket to the Desktop through the relay.
        let url = try await AppPhoneRelay.socketURL(.pair, phoneId: phoneId, runtimeDeviceId: runtime.id,
                                                    appPhoneKey: appPhoneKey, cloudBaseURL: cloud)
        let socket = URLSession.shared.webSocketTask(with: url)
        onSocket(socket)
        socket.resume()
        defer { socket.cancel(with: .normalClosure, reason: nil) }

        let privateKey = Curve25519.KeyAgreement.PrivateKey()
        let appPub = privateKey.publicKey.rawRepresentation
        let hello: [String: Any] = [
            "type": "hello",
            "phoneId": phoneId,
            "name": name,
            "model": deviceModel(),
            "pub": appPub.base64EncodedString(),
        ]
        let helloData = try JSONSerialization.data(withJSONObject: hello)
        try await socket.send(.string(String(decoding: helloData, as: UTF8.self)))
        onCode(nil)

        // 3. Wait for the Desktop. A message carrying the node's key before
        // the verdict lets the code show while the person is still deciding.
        let deadline = Date().addingTimeInterval(approvalTimeout)
        var nodePub: Data?
        while true {
            try Task.checkCancellation()
            guard Date() < deadline else { throw PairFailure.timedOut }
            let message: URLSessionWebSocketTask.Message
            do {
                message = try await withTimeout(deadline.timeIntervalSinceNow) { try await socket.receive() }
            } catch is TimeoutError {
                throw PairFailure.timedOut
            } catch {
                throw Task.isCancelled ? CancellationError() : PairFailure.closed
            }
            let raw: Data
            switch message {
            case .string(let text): raw = Data(text.utf8)
            case .data(let bytes): raw = bytes
            @unknown default: continue
            }
            guard let obj = try? JSONSerialization.jsonObject(with: raw) as? [String: Any],
                  let type = obj["type"] as? String else { continue }
            if let b64 = obj["pub"] as? String, let pub = Data(base64Encoded: b64), pub.count == 32 {
                if let earlier = nodePub, earlier != pub { throw PairFailure.keyMismatch }
                if nodePub == nil {
                    nodePub = pub
                    onCode(SecureEnvelope.pairCode(appPublicKey: appPub, nodePublicKey: pub))
                }
            }
            switch type {
            case "denied":
                throw PairFailure.denied
            case "approved":
                guard let pub = nodePub else { throw PairFailure.closed }
                let key = try SecureEnvelope.sessionKey(privateKey: privateKey, nodePublicKey: pub, phoneId: phoneId)
                return AppPhoneCredentials(
                    phoneId: phoneId,
                    name: name,
                    runtimeDeviceId: runtime.id,
                    runtimeName: runtime.name,
                    appPhoneKey: appPhoneKey,
                    sessionKeyHex: key.hexString,
                    cloudBaseURL: cloud.absoluteString,
                    pairedAt: Date()
                )
            default:
                continue
            }
        }
    }

    /// A cloud request carrying the Clerk session (the app phone routes are
    /// Clerk-authed; the runtime device token is for the gateway).
    private static func clerkRequest(path: String) async throws -> URLRequest {
        var request = URLRequest(url: AppConfig.cloudAPIBaseURL.appendingPathComponent(path))
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        let token: String?
        if let clerk = try await AuthManager.shared.getToken() {
            token = clerk
        } else {
            token = try await AuthManager.shared.effectiveToken()
        }
        guard let token, !token.isEmpty else { throw PairFailure.notSignedIn }
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        return request
    }

    /// "iPhone16,2"-style hardware id; the Desktop shows it beside the name.
    private static func deviceModel() -> String {
        var info = utsname()
        uname(&info)
        let machine = withUnsafeBytes(of: &info.machine) { raw in
            String(decoding: raw.prefix { $0 != 0 }, as: UTF8.self)
        }
        return machine.isEmpty ? UIDevice.current.model : machine
    }

    // MARK: - Foreground / control socket

    /// Called from the scene phase: the control socket lives only while the
    /// app is in the foreground (iOS suspends it otherwise anyway).
    func setForeground(_ foreground: Bool) {
        isForeground = foreground
        if foreground { DeviceAccessStore.shared.refresh() }
        refreshScreenState()
        if foreground {
            startControl()
            screenPoll?.invalidate()
            screenPoll = Timer.scheduledTimer(withTimeInterval: AppPhoneSharedState.heartbeatInterval, repeats: true) { _ in
                Task { @MainActor in AppPhoneService.shared.refreshScreenState() }
            }
        } else {
            stopControl()
            screenPoll?.invalidate()
            screenPoll = nil
        }
    }

    func refreshScreenState() {
        isScreenLive = AppPhoneSharedState.isLive
        isScreenWatched = AppPhoneSharedState.isWatched
        if isScreenLive { isScreenPromptPresented = false }
    }

    /// Stop from the "Your bot can see this screen" banner.
    func stopScreenShare() {
        AppPhoneSharedState.post(AppPhoneSharedState.stopNotification)
        AppPhoneSharedState.markEnded()
        refreshScreenState()
    }

    /// Device Access switches or iOS permissions changed: tell the node.
    func deviceAccessChanged() {
        sendState()
    }

    /// Sends the `state` event when the ACI browser opens or closes.
    func browserStateChanged() {
        sendState()
    }

    private func restartControl() {
        stopControl()
        if isForeground { startControl() }
    }

    private func startControl() {
        guard controlTask == nil, credentials != nil else { return }
        controlTask = Task { [weak self] in
            var delay: TimeInterval = 1
            while !Task.isCancelled {
                guard let self, let creds = self.credentials, let key = creds.sessionKey else { return }
                do {
                    let url = try await AppPhoneRelay.socketURL(.control, credentials: creds)
                    let socket = URLSession.shared.webSocketTask(with: url)
                    self.controlSocket = socket
                    socket.resume()
                    try await self.runControl(socket, key: key) { delay = 1 }
                } catch let failure as AppPhoneRelay.Failure where failure.isFatal {
                    // Revoked on the Desktop or the cloud: forget the pairing
                    // so Settings offers to pair again.
                    AppPhoneKeychain.clear()
                    self.credentials = nil
                    self.isControlConnected = false
                    self.controlSocket = nil
                    self.controlTask = nil
                    return
                } catch {
                    // Desktop offline, network gone: retry below.
                }
                self.isControlConnected = false
                self.controlSocket = nil
                try? await Task.sleep(for: .seconds(delay))
                delay = min(delay * 2, 30)
            }
        }
    }

    private func stopControl() {
        controlTask?.cancel()
        controlTask = nil
        controlSocket?.cancel(with: .goingAway, reason: nil)
        controlSocket = nil
        isControlConnected = false
    }

    /// One control socket's life: announce state, then answer requests until
    /// it closes. `onOpen` runs once the first frame went through.
    private func runControl(_ socket: URLSessionWebSocketTask, key: Data, onOpen: () -> Void) async throws {
        try await sendSealed(["type": "state", "foreground": true, "browserOpen": ACIBrowserBridge.shared.isBrowserOpen,
                              "deviceAccess": DeviceAccessStore.shared.allowedSnapshot],
                             on: socket, key: key)
        isControlConnected = true
        onOpen()
        let pinger = Task { [weak socket] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(20))
                socket?.sendPing { _ in }
            }
        }
        defer { pinger.cancel() }
        while !Task.isCancelled {
            let message = try await socket.receive()
            let raw: Data
            switch message {
            case .string(let text): raw = Data(text.utf8)
            case .data(let bytes): raw = bytes
            @unknown default: continue
            }
            guard let request = open(raw, key: key),
                  let id = request["id"],
                  let method = request["method"] as? String else { continue }
            let params = request["params"] as? [String: Any] ?? [:]
            // Each request runs on its own so a 10 s `web.wait` doesn't hold
            // up a `web.snapshot` behind it.
            Task { [weak self] in
                guard let self else { return }
                var reply: [String: Any] = ["id": id]
                do {
                    reply["result"] = try await self.handle(method: method, params: params)
                } catch let error as AppPhoneRPCError {
                    reply["error"] = ["code": error.code, "message": error.message]
                } catch {
                    reply["error"] = ["code": "internal", "message": error.localizedDescription]
                }
                try? await self.sendSealed(reply, on: socket, key: key)
            }
        }
    }

    private func handle(method: String, params: [String: Any]) async throws -> Any {
        switch method {
        case "screen.prompt":
            // Only a person can start a broadcast: show the picker sheet.
            if !isScreenLive { isScreenPromptPresented = true }
            return ["shown": !isScreenLive, "live": isScreenLive]
        default:
            if method.hasPrefix("device.") {
                return try await DeviceAccessService.handle(method: method, params: params)
            }
            if method.hasPrefix("web.") {
                return try await ACIBrowserBridge.shared.handle(method: method, params: params)
            }
            throw AppPhoneRPCError(code: "unknown_method", message: "Unknown method \(method)")
        }
    }

    private func open(_ raw: Data, key: Data) -> [String: Any]? {
        guard let opened = try? SecureEnvelope.open(raw, key: key) else { return nil }
        let now = Date()
        seenNonces = seenNonces.filter { now.timeIntervalSince($0.value) < SecureEnvelope.timestampWindow * 2 }
        guard seenNonces[opened.nonceHex] == nil else { return nil }
        seenNonces[opened.nonceHex] = now
        return try? JSONSerialization.jsonObject(with: opened.plaintext) as? [String: Any]
    }

    private func sendSealed(_ object: [String: Any], on socket: URLSessionWebSocketTask, key: Data) async throws {
        let plain = try JSONSerialization.data(withJSONObject: object)
        guard let sealed = SecureEnvelope.seal(plain, key: key) else {
            throw AppPhoneRPCError(code: "internal", message: "Could not seal the message")
        }
        try await socket.send(.string(String(decoding: sealed, as: UTF8.self)))
    }

    private func sendState() {
        guard isControlConnected, let socket = controlSocket, let key = credentials?.sessionKey else { return }
        let state: [String: Any] = ["type": "state", "foreground": isForeground,
                                    "browserOpen": ACIBrowserBridge.shared.isBrowserOpen,
                                    "deviceAccess": DeviceAccessStore.shared.allowedSnapshot]
        Task { try? await sendSealed(state, on: socket, key: key) }
    }
}

/// `{"id","error":{"code","message"}}` on the control socket.
struct AppPhoneRPCError: Error, Sendable {
    let code: String
    let message: String
}

private struct TimeoutError: Error {}

/// Runs `operation`, throwing `TimeoutError` after `seconds`.
private func withTimeout<T: Sendable>(_ seconds: TimeInterval,
                                      _ operation: @escaping @Sendable () async throws -> T) async throws -> T {
    try await withThrowingTaskGroup(of: T.self) { group in
        group.addTask { try await operation() }
        group.addTask {
            try await Task.sleep(for: .seconds(max(0, seconds)))
            throw TimeoutError()
        }
        defer { group.cancelAll() }
        guard let first = try await group.next() else { throw TimeoutError() }
        return first
    }
}
