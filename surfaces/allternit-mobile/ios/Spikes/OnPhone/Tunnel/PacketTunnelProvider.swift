import Foundation
import NetworkExtension

/// Loopback packet tunnel. It routes only 10.7.0.1/32. Every IPv4 packet the
/// phone sends to 10.7.0.1 is written back with source and destination swapped,
/// so this phone's own services (lockdownd on 62078) answer the app as if from
/// another host. TCP/UDP checksums stay valid: the pseudo-header sum does not
/// depend on the order of the two addresses. Nothing leaves the phone.
final class PacketTunnelProvider: NEPacketTunnelProvider {
    private var running = false
    private var reflected = 0
    private var heartbeat: DispatchSourceTimer?

    override func startTunnel(options: [String: NSObject]?, completionHandler: @escaping (Error?) -> Void) {
        let settings = NEPacketTunnelNetworkSettings(tunnelRemoteAddress: Spike.deviceIP)
        let v4 = NEIPv4Settings(addresses: [Spike.tunnelLocalIP], subnetMasks: ["255.255.255.0"])
        v4.includedRoutes = [NEIPv4Route(destinationAddress: Spike.deviceIP, subnetMask: "255.255.255.255")]
        settings.ipv4Settings = v4
        settings.mtu = 1500
        let startedBy = (options?["startedBy"] as? String) ?? "system"
        setTunnelNetworkSettings(settings) { [weak self] error in
            guard let self else { return }
            if let error {
                Spike.appendLine(Spike.tunnelLog, ["event": "start_failed", "error": error.localizedDescription])
                completionHandler(error)
                return
            }
            Spike.appendLine(Spike.tunnelLog, ["event": "start", "startedBy": startedBy])
            self.running = true
            self.readLoop()
            self.startHeartbeat()
            completionHandler(nil)
        }
    }

    override func stopTunnel(with reason: NEProviderStopReason, completionHandler: @escaping () -> Void) {
        running = false
        heartbeat?.cancel()
        heartbeat = nil
        Spike.appendLine(Spike.tunnelLog, ["event": "stop", "reason": reason.rawValue, "reasonName": Self.name(reason), "reflected": reflected])
        completionHandler()
    }

    override func handleAppMessage(_ messageData: Data, completionHandler: ((Data?) -> Void)?) {
        let status: [String: Any] = ["running": running, "reflected": reflected]
        completionHandler?(try? JSONSerialization.data(withJSONObject: status))
    }

    override func sleep(completionHandler: @escaping () -> Void) {
        Spike.appendLine(Spike.tunnelLog, ["event": "sleep"])
        completionHandler()
    }

    override func wake() {
        Spike.appendLine(Spike.tunnelLog, ["event": "wake"])
    }

    private func readLoop() {
        packetFlow.readPackets { [weak self] packets, protocols in
            guard let self, self.running else { return }
            let out = packets.map(Self.reflect)
            self.reflected += out.count
            self.packetFlow.writePackets(out, withProtocols: protocols)
            self.readLoop()
        }
    }

    /// Swap IPv4 source (bytes 12-15) and destination (16-19). Other packets pass unchanged.
    static func reflect(_ packet: Data) -> Data {
        guard packet.count >= 20, packet[packet.startIndex] >> 4 == 4 else { return packet }
        var p = Data(packet)
        p.withUnsafeMutableBytes { raw in
            guard let b = raw.baseAddress?.assumingMemoryBound(to: UInt8.self) else { return }
            for i in 0..<4 {
                let t = b[12 + i]
                b[12 + i] = b[16 + i]
                b[16 + i] = t
            }
        }
        return p
    }

    private func startHeartbeat() {
        let t = DispatchSource.makeTimerSource(queue: .global(qos: .utility))
        t.schedule(deadline: .now() + 5, repeating: 5)
        t.setEventHandler { [weak self] in
            Spike.appendLine(Spike.tunnelLog, ["event": "beat", "reflected": self?.reflected ?? 0])
        }
        t.resume()
        heartbeat = t
    }

    static func name(_ r: NEProviderStopReason) -> String {
        switch r {
        case .none: return "none"
        case .userInitiated: return "userInitiated"
        case .providerFailed: return "providerFailed"
        case .noNetworkAvailable: return "noNetworkAvailable"
        case .unrecoverableNetworkChange: return "unrecoverableNetworkChange"
        case .providerDisabled: return "providerDisabled"
        case .authenticationCanceled: return "authenticationCanceled"
        case .configurationFailed: return "configurationFailed"
        case .idleTimeout: return "idleTimeout"
        case .configurationDisabled: return "configurationDisabled"
        case .configurationRemoved: return "configurationRemoved"
        case .superceded: return "superceded"
        case .userLogout: return "userLogout"
        case .userSwitch: return "userSwitch"
        case .connectionFailed: return "connectionFailed"
        case .sleep: return "sleep"
        case .appUpdate: return "appUpdate"
        @unknown default: return "unknown(\(r.rawValue))"
        }
    }
}
