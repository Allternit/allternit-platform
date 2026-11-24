import Foundation
import SwiftUI

// The live view of an Android phone that shares its screen with the Allternit
// Android app. Mirrors the web viewer (allternit-ai
// `src/lib/phone-stream/cloud-ws-transport.ts`, `control-token.ts`): the relay
// socket carries iCTRL packets, taps go back as `{type:"input", events}` on a
// full-control socket only.

// MARK: - Packets

/// One iCTRL packet (`LiveScreenPacket` framing), decoded for the viewer.
enum LivePacket: Equatable {
    case status
    case config(width: Int, height: Int, rotation: Int)
    case frame(Data)
    case ended(reason: String)
    case audioConfig
    case audio

    /// A whole packet, or nil when the bytes are not exactly one.
    static func read(_ data: Data) -> LivePacket? {
        let bytes = [UInt8](data)
        guard bytes.count >= LiveScreenPacket.headerLength,
              let kind = LiveScreenPacket.Kind(rawValue: bytes[0]) else { return nil }
        let length = Int(bytes[1]) << 24 | Int(bytes[2]) << 16 | Int(bytes[3]) << 8 | Int(bytes[4])
        guard bytes.count == LiveScreenPacket.headerLength + length else { return nil }
        let payload = Data(bytes[LiveScreenPacket.headerLength...])
        func object() -> [String: Any] {
            (try? JSONSerialization.jsonObject(with: payload)) as? [String: Any] ?? [:]
        }
        switch kind {
        case .status: return .status
        case .config:
            let o = object()
            guard let w = (o["width"] as? NSNumber)?.intValue, let h = (o["height"] as? NSNumber)?.intValue else { return nil }
            return .config(width: w, height: h, rotation: (o["rotation"] as? NSNumber)?.intValue ?? 0)
        case .frame: return .frame(payload)
        case .ended: return .ended(reason: object()["reason"] as? String ?? "")
        case .audioConfig: return .audioConfig
        case .audio: return .audio
        // Phone Use answers to ingest-socket requests (contract §9.1); viewers skip them.
        case .reply: return nil
        }
    }
}

// MARK: - Tap mapping

enum PhoneLiveGeometry {
    /// Where the aspect-fit picture sits inside the view (letterboxing).
    static func fittedRect(video: CGSize, in view: CGSize) -> CGRect {
        guard video.width > 0, video.height > 0, view.width > 0, view.height > 0 else { return .zero }
        let scale = min(view.width / video.width, view.height / video.height)
        let size = CGSize(width: video.width * scale, height: video.height * scale)
        return CGRect(x: (view.width - size.width) / 2, y: (view.height - size.height) / 2, width: size.width, height: size.height)
    }

    /// A point in the view as a point in video pixels. Outside the picture it
    /// is nil, or clamped to the edge when `clamp` (so a swipe can end off it).
    static func videoPoint(_ point: CGPoint, view: CGSize, video: CGSize, clamp: Bool) -> CGPoint? {
        let rect = fittedRect(video: video, in: view)
        guard rect.width > 0, rect.height > 0 else { return nil }
        if !clamp && !rect.contains(point) { return nil }
        let x = min(max((point.x - rect.minX) / rect.width, 0), 1) * video.width
        let y = min(max((point.y - rect.minY) / rect.height, 0), 1) * video.height
        return CGPoint(x: x.rounded(), y: y.rounded())
    }
}

/// `PhoneInputEvent` (electron-api.d.ts), the subset the viewer sends.
enum PhoneInput: Equatable {
    enum Action: String { case down, move, up }
    case touch(Action, x: Int, y: Int)
    case text(String)
    case back, home, appSwitch

    var json: [String: Any] {
        switch self {
        case .touch(let action, let x, let y):
            return ["type": "touch", "action": action.rawValue, "pointerId": 0, "x": x, "y": y, "pressure": action == .up ? 0 : 1]
        case .text(let s): return ["type": "text", "text": s]
        case .back: return ["type": "back"]
        case .home: return ["type": "home"]
        case .appSwitch: return ["type": "app_switch"]
        }
    }
}

// MARK: - Copy (same words as the web)

enum PhoneLiveCopy {
    static func waiting(_ name: String) -> String { "Tap Start on \(name) to share its screen." }
    static func tapsOff(_ name: String) -> String { "Turn on taps in the Allternit app on \(name)." }
    static let shareStopped = "Sharing stopped on the phone."
    static let relayOffline = "The computer that relays your phone went offline. Open Allternit Desktop or start your cloud computer, then Retry."
    static let signedOut = "Sign in to Allternit to see your phones."

    static func ended(reason: String, name: String) -> String {
        if reason == "token_expired" { return "\(name) didn’t start sharing. Tap Start on \(name), then Retry." }
        if reason == "desktop_offline" { return relayOffline }
        return shareStopped
    }
}

// MARK: - Session

@MainActor
final class PhoneLiveSession: ObservableObject {
    enum State: Equatable {
        case connecting
        case waiting
        case live
        case ended(String)
        case error(String)
        case relayOffline
        case signedOut
    }

    @Published private(set) var state: State = .connecting
    @Published private(set) var videoSize: CGSize = .zero
    @Published private(set) var hasControl = false
    @Published private(set) var controlBusy = false
    @Published var notice: String?

    let phone: ACIComputersClient.CloudPhone
    private weak var display: AnnexBDisplayView?
    private var socket: URLSessionWebSocketTask?
    private var generation = 0
    private var pingTask: Task<Void, Never>?
    private var receiveTask: Task<Void, Never>?
    private var closed = false
    private var waitRenewals = 0
    private var reconnectsUsed = 0
    private static let maxWaitRenewals = 3
    private static let tokenExpiredCode = 4401

    init(phone: ACIComputersClient.CloudPhone) { self.phone = phone }

    func attach(_ view: AnnexBDisplayView) { display = view }

    func start() {
        closed = false
        waitRenewals = 0
        reconnectsUsed = 0
        hasControl = false
        videoSize = .zero
        notice = nil
        state = .connecting
        Task { await connect(readOnly: true) }
    }

    func stop() {
        closed = true
        generation += 1
        pingTask?.cancel()
        receiveTask?.cancel()
        socket?.cancel(with: .goingAway, reason: nil)
        socket = nil
    }

    // MARK: Connect

    private func connect(readOnly: Bool) async {
        do {
            let token = try await mintToken(readOnly: readOnly, approvalId: nil)
            guard case .token(let value) = token else { return }
            try await open(token: value)
        } catch {
            fail(error)
        }
    }

    private func fail(_ error: Error) {
        if closed { return }
        stopSocketOnly()
        if let failure = error as? ACIComputersClient.Failure, case .signedOut = failure {
            state = .signedOut
        } else if let relay = error as? RuntimeRelayError, relay == .unreachable {
            state = .relayOffline
        } else if let failure = error as? ACIComputersClient.Failure, case .auth = failure {
            state = .signedOut
        } else {
            let text = error.localizedDescription
            state = .error(text.isEmpty ? "The phone stream couldn’t connect." : text)
        }
    }

    private func stopSocketOnly() {
        generation += 1
        pingTask?.cancel()
        receiveTask?.cancel()
        socket?.cancel(with: .goingAway, reason: nil)
        socket = nil
    }

    private func open(token: String) async throws {
        guard let runtimeId = await RuntimeRelayStore.shared.resolveActiveRuntimeId() else {
            throw RuntimeRelayError.unreachable
        }
        let path = "/ws/phones/\(Self.component(phone.serial))/stream?token=\(Self.component(token))"
        let task = try await RuntimeRelayClient.shared.webSocket(path: path, runtimeId: runtimeId)
        if closed {
            task.cancel(with: .goingAway, reason: nil)
            return
        }
        generation += 1
        let gen = generation
        let old = socket
        pingTask?.cancel()
        receiveTask?.cancel()
        socket = task
        old?.cancel(with: .goingAway, reason: nil)
        if videoSize == .zero { state = .waiting }
        receiveTask = Task { [weak self] in await self?.receiveLoop(task, gen) }
        pingTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(20))
                if Task.isCancelled { return }
                guard let self, self.generation == gen else { return }
                task.send(.string(#"{"type":"ping"}"#)) { _ in }
            }
        }
    }

    // MARK: Receive

    private func receiveLoop(_ task: URLSessionWebSocketTask, _ gen: Int) async {
        while !Task.isCancelled {
            let message: URLSessionWebSocketTask.Message
            do {
                message = try await task.receive()
            } catch {
                handleClose(task, gen)
                return
            }
            guard gen == generation else { return }
            switch message {
            case .data(let data): handle(packet: LivePacket.read(data))
            case .string(let text): handleText(text)
            @unknown default: break
            }
        }
    }

    private func handleClose(_ task: URLSessionWebSocketTask, _ gen: Int) {
        guard gen == generation, !closed else { return }
        if case .ended = state { return }
        if task.closeCode.rawValue == Self.tokenExpiredCode, reconnectsUsed == 0 {
            reconnectsUsed += 1
            reconnectReadOnly()
            return
        }
        state = .ended("The phone stream connection closed.")
    }

    private func reconnectReadOnly() {
        // A fresh read-only token: a full-control grant is single use.
        hasControl = false
        Task { await connect(readOnly: true) }
    }

    func handle(packet: LivePacket?) {
        guard let packet else { return }
        switch packet {
        case .config(let width, let height, _):
            videoSize = CGSize(width: width, height: height)
            state = .live
            display?.apply(LiveLayerAction.decide(for: packet, hasFormat: display?.hasFormat ?? false))
        case .frame:
            guard videoSize != .zero, let display else { return }
            display.apply(LiveLayerAction.decide(for: packet, hasFormat: display.hasFormat))
        case .ended(let reason):
            end(reason)
        default:
            break
        }
    }

    private func handleText(_ text: String) {
        guard let o = (try? JSONSerialization.jsonObject(with: Data(text.utf8))) as? [String: Any],
              o["type"] as? String == "end" else { return }
        let reason = (o["reason"] as? String) ?? ""
        // The relay's wait outlives one token: renew it (a few times) instead of ending.
        if reason == "token_expired", videoSize == .zero, waitRenewals < Self.maxWaitRenewals {
            waitRenewals += 1
            reconnectReadOnly()
            return
        }
        end(reason)
    }

    private func end(_ reason: String) {
        if case .ended = state { return }
        state = .ended(reason)
        stopSocketOnly()
    }

    // MARK: Taps

    /// "Allow taps": a full-control token (approving the confirmation grant
    /// the person just asked for), then a new socket in place of the old one.
    func requestControl() async {
        if hasControl || controlBusy { return }
        guard phone.canTap == true else {
            notice = PhoneLiveCopy.tapsOff(phone.name)
            return
        }
        controlBusy = true
        notice = nil
        defer { controlBusy = false }
        do {
            var result = try await mintToken(readOnly: false, approvalId: nil)
            if case .confirmation(let id) = result {
                try await approve(id)
                result = try await mintToken(readOnly: false, approvalId: id)
            }
            guard case .token(let value) = result else {
                notice = "Taps couldn’t be turned on."
                return
            }
            try await open(token: value)
            hasControl = true
        } catch {
            notice = (error as? RuntimeRelayError) == .unreachable
                ? PhoneLiveCopy.relayOffline
                : (error.localizedDescription.isEmpty ? "Couldn’t turn on taps." : error.localizedDescription)
        }
    }

    func send(_ events: [PhoneInput]) {
        guard hasControl, phone.canTap == true, let socket, !events.isEmpty else { return }
        let body: [String: Any] = ["type": "input", "events": events.map(\.json)]
        guard let data = try? JSONSerialization.data(withJSONObject: body),
              let text = String(data: data, encoding: .utf8) else { return }
        socket.send(.string(text)) { _ in }
    }

    // MARK: HTTP

    enum Minted: Equatable {
        case token(String)
        case confirmation(String)
    }

    private func mintToken(readOnly: Bool, approvalId: String?) async throws -> Minted {
        var path = "api/v1/phones/\(Self.component(phone.serial))/ws-token"
        if let approvalId { path += "?approval_id=\(Self.component(approvalId))" }
        let body = try JSONSerialization.data(withJSONObject: ["readOnly": readOnly])
        let (status, data) = try await Self.cloudPost(path, body: body)
        let json = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] ?? [:]
        if status == 403 {
            if json["error"] as? String == "confirmation_required", let id = json["approval_id"] as? String {
                return .confirmation(id)
            }
            throw ACIComputersClient.Failure.auth(403)
        }
        if status == 401 { throw ACIComputersClient.Failure.auth(401) }
        guard (200...299).contains(status) else {
            let text = (json["message"] as? String) ?? (json["reason"] as? String) ?? (json["error"] as? String)
            throw ACIComputersClient.Failure.server(text ?? "The phone stream couldn’t start.")
        }
        guard let token = json["token"] as? String, !token.isEmpty else {
            throw ACIComputersClient.Failure.server("The phone stream token response was unreadable.")
        }
        return .token(token)
    }

    /// `POST /api/aci/handoff/:id/approve` on the runtime: the "Allow taps"
    /// tap is the approval (control-token.ts).
    private func approve(_ approvalId: String) async throws {
        guard let runtimeId = await RuntimeRelayStore.shared.resolveActiveRuntimeId() else {
            throw RuntimeRelayError.unreachable
        }
        var request = URLRequest(url: RuntimeRelayClient.relayBaseURL(runtimeId: runtimeId)
            .appendingPathComponent("api/aci/handoff/\(approvalId)/approve"))
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = Data("{}".utf8)
        let (_, response) = try await RuntimeRelayClient.shared.send(request, runtimeId: runtimeId)
        if let http = response as? HTTPURLResponse, !(200...299).contains(http.statusCode) {
            throw ACIComputersClient.Failure.server("Taps couldn’t be approved. Try again.")
        }
    }

    static func cloudPost(_ path: String, body: Data) async throws -> (Int, Data) {
        // Built from a string: appendingPathComponent would escape the
        // percent-encoded serial and a query string.
        let base = AppConfig.cloudAPIBaseURL.absoluteString.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        guard let url = URL(string: base + "/" + path) else { throw ACIComputersClient.Failure.server("The phone stream couldn’t start.") }
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.httpBody = body
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        let token: String?
        if let clerk = try await AuthManager.shared.getToken() {
            token = clerk
        } else {
            token = try await AuthManager.shared.effectiveToken()
        }
        guard let token, !token.isEmpty else { throw ACIComputersClient.Failure.signedOut }
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        let (data, response) = try await URLSession.shared.data(for: request)
        return ((response as? HTTPURLResponse)?.statusCode ?? 0, data)
    }

    /// `encodeURIComponent`.
    static func component(_ value: String) -> String {
        var allowed = CharacterSet.alphanumerics
        allowed.insert(charactersIn: "-_.!~*'()")
        return value.addingPercentEncoding(withAllowedCharacters: allowed) ?? value
    }
}
