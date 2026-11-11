import Foundation
import Network
import NetworkExtension
import UIKit
import Vision

// MARK: - Lockdown over the loopback tunnel

/// Talks to this phone's own lockdownd through the loopback tunnel. Wire format:
/// a 4-byte big-endian length, then an XML property list.
enum LockdownProbe {
    struct Reply { let body: [String: Any]?; let ms: Double; let error: String? }

    static func request(_ body: [String: Any], timeout: Double = 3) async -> Reply {
        await withCheckedContinuation { (cont: CheckedContinuation<Reply, Never>) in
            let t0 = CFAbsoluteTimeGetCurrent()
            let lock = NSLock()
            var done = false
            let conn = NWConnection(host: NWEndpoint.Host(Spike.deviceIP),
                                    port: NWEndpoint.Port(rawValue: Spike.lockdownPort)!, using: .tcp)
            func finish(_ reply: [String: Any]?, _ error: String?) {
                lock.lock(); defer { lock.unlock() }
                if done { return }
                done = true
                conn.cancel()
                cont.resume(returning: Reply(body: reply, ms: (CFAbsoluteTimeGetCurrent() - t0) * 1000, error: error))
            }
            conn.stateUpdateHandler = { state in
                switch state {
                case .ready:
                    guard let plist = try? PropertyListSerialization.data(fromPropertyList: body, format: .xml, options: 0) else {
                        finish(nil, "encode"); return
                    }
                    var frame = Data()
                    var len = UInt32(plist.count).bigEndian
                    withUnsafeBytes(of: &len) { frame.append(contentsOf: $0) }
                    frame.append(plist)
                    conn.send(content: frame, completion: .contentProcessed { err in
                        if let err { finish(nil, "send: \(err)"); return }
                        conn.receive(minimumIncompleteLength: 4, maximumLength: 4) { head, _, _, err in
                            guard let head, head.count == 4 else { finish(nil, "head: \(String(describing: err))"); return }
                            let n = Int(head.reduce(UInt32(0)) { ($0 << 8) | UInt32($1) })
                            conn.receive(minimumIncompleteLength: n, maximumLength: n) { data, _, _, err in
                                guard let data,
                                      let obj = try? PropertyListSerialization.propertyList(from: data, format: nil) as? [String: Any] else {
                                    finish(nil, "body: \(String(describing: err))"); return
                                }
                                finish(obj, nil)
                            }
                        }
                    })
                case .failed(let err): finish(nil, "failed: \(err)")
                case .waiting(let err): finish(nil, "waiting: \(err)")
                default: break
                }
            }
            conn.start(queue: .global(qos: .userInitiated))
            DispatchQueue.global().asyncAfter(deadline: .now() + timeout) { finish(nil, "timeout after \(timeout)s") }
        }
    }

    static func queryType() async -> Reply {
        await request(["Label": "allternit-onphone-spike", "Request": "QueryType"])
    }

    /// On-phone pairing check: lockdownd answers StartSession only for a HostID it
    /// paired with. The spike stops before the TLS upgrade (see README).
    static func startSession(pairRecord: [String: Any]) async -> Reply {
        guard let host = pairRecord["HostID"] as? String, let buid = pairRecord["SystemBUID"] as? String else {
            return Reply(body: nil, ms: 0, error: "pair record has no HostID/SystemBUID")
        }
        return await request(["Label": "allternit-onphone-spike", "ProtocolVersion": "2",
                               "Request": "StartSession", "HostID": host, "SystemBUID": buid])
    }
}

// MARK: - Phone guard (Wave 1), unchanged

/// Every action the spike can take goes through the runtime's existing guard:
/// `POST /api/v1/phones/guard/check`. Only `allow` lets the action run. Anything
/// else (hold, denied, unreachable, not set up) blocks it. Fail closed.
enum GuardClient {
    struct Result { let decision: String; let raw: [String: Any]?; let error: String? }

    static var settings: UserDefaults { UserDefaults(suiteName: Spike.appGroup) ?? .standard }

    static func check(tool: String, args: [String: Any], app: String?) async -> Result {
        let base = settings.string(forKey: "runtimeURL") ?? ""
        let token = settings.string(forKey: "runtimeToken") ?? ""
        let serial = settings.string(forKey: "phoneSerial") ?? ""
        guard let url = URL(string: base.trimmingCharacters(in: .whitespaces) + "/api/v1/phones/guard/check"),
              !base.isEmpty, !token.isEmpty, !serial.isEmpty else {
            return Result(decision: "unconfigured", raw: nil, error: "Set runtime URL, token and phone serial first.")
        }
        var req = URLRequest(url: url, timeoutInterval: 15)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        var body: [String: Any] = ["serial": serial, "tool": tool, "args": args, "platform": "ios",
                                   "ctx": ["source": "api"]]
        if let app { body["app"] = app }
        req.httpBody = try? JSONSerialization.data(withJSONObject: body)
        do {
            let (data, resp) = try await URLSession.shared.data(for: req)
            let code = (resp as? HTTPURLResponse)?.statusCode ?? 0
            let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
            guard code == 200, let decision = obj?["decision"] as? String else {
                return Result(decision: "error", raw: obj, error: "HTTP \(code)")
            }
            return Result(decision: decision, raw: obj, error: nil)
        } catch {
            return Result(decision: "error", raw: nil, error: error.localizedDescription)
        }
    }

    /// Queue a typing job for the keyboard, only after the guard allows `phone.type`.
    static func queueTyping(text: String, target: String) async -> Result {
        let r = await check(tool: "phone.type", args: ["text": text], app: target)
        guard r.decision == "allow" else { return r }
        Spike.remove(Spike.typeResult)
        Spike.writeJSON(Spike.typeJob, [
            "id": UUID().uuidString, "text": text, "target": target, "state": "queued",
            "guard": "allow", "holdId": r.raw?["holdId"] ?? NSNull(), "createdAt": Spike.now,
        ])
        return r
    }
}

// MARK: - Tunnel control

enum TunnelControl {
    static func manager() async throws -> NETunnelProviderManager {
        let all = try await NETunnelProviderManager.loadAllFromPreferences()
        if let m = all.first(where: { ($0.protocolConfiguration as? NETunnelProviderProtocol)?.providerBundleIdentifier == Spike.tunnelBundleID }) {
            return m
        }
        let m = NETunnelProviderManager()
        let proto = NETunnelProviderProtocol()
        proto.providerBundleIdentifier = Spike.tunnelBundleID
        proto.serverAddress = "On-phone loopback (\(Spike.deviceIP))"
        m.protocolConfiguration = proto
        m.localizedDescription = "Allternit OnPhone Spike"
        return m
    }

    /// Installs (first time asks the person to allow a VPN configuration) and starts.
    /// `onDemand` reconnects after restart and after another VPN lets go.
    static func start(onDemand: Bool) async throws {
        let m = try await manager()
        m.isEnabled = true
        let rule = NEOnDemandRuleConnect()
        rule.interfaceTypeMatch = .any
        m.onDemandRules = [rule]
        m.isOnDemandEnabled = onDemand
        try await m.saveToPreferences()
        try await m.loadFromPreferences()
        try m.connection.startVPNTunnel(options: ["startedBy": "app" as NSString])
    }

    static func stop() async throws {
        let m = try await manager()
        m.isOnDemandEnabled = false
        try await m.saveToPreferences()
        m.connection.stopVPNTunnel()
    }

    static func status() async -> String {
        guard let m = try? await manager() else { return "not installed" }
        switch m.connection.status {
        case .invalid: return "invalid (not installed)"
        case .disconnected: return "disconnected"
        case .connecting: return "connecting"
        case .connected: return "connected"
        case .reasserting: return "reasserting"
        case .disconnecting: return "disconnecting"
        @unknown default: return "unknown"
        }
    }
}

// MARK: - Own-window read (control measurement)

enum OwnRead {
    /// Vision text read of this app's own window: the same OCR screen capture runs,
    /// without the capture. A control for the cross-app numbers.
    @MainActor static func once() -> (ms: Double, lines: Int)? {
        guard let window = UIApplication.shared.connectedScenes
            .compactMap({ ($0 as? UIWindowScene)?.keyWindow }).first else { return nil }
        let image = UIGraphicsImageRenderer(bounds: window.bounds).image { _ in
            window.drawHierarchy(in: window.bounds, afterScreenUpdates: false)
        }
        guard let cg = image.cgImage else { return nil }
        let t0 = CFAbsoluteTimeGetCurrent()
        let req = VNRecognizeTextRequest()
        req.recognitionLevel = .accurate
        req.usesLanguageCorrection = false
        try? VNImageRequestHandler(cgImage: cg, options: [:]).perform([req])
        return ((CFAbsoluteTimeGetCurrent() - t0) * 1000, req.results?.count ?? 0)
    }
}
