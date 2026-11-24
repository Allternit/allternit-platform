import Foundation

/// The cloud + relay calls behind ACI › Computers on the web phone:
///
/// - `GET  {cloud}/api/v1/phones`                 → linked phones (lib/phones-cloud.ts `listCloudPhones`)
/// - `GET  {cloud}/api/v1/api-keys`               → operator keys (FabricOperatorKeys.tsx)
/// - `POST {cloud}/api/v1/api-keys`               → mint one (`{ name, scopes }`; the token comes back once)
/// - `DELETE {cloud}/api/v1/api-keys/:id`         → revoke
/// - `GET  /v1/browser-skills` on a paired computer, through the relay
///   (browser-skills-api.ts `listWorkflowSpecs` over `createFabricWorkflowsFetch`)
///
/// Cloud routes carry the Clerk session, like the PWA's `cloudApiFetch`.
final class ACIComputersClient: Sendable {
    static let shared = ACIComputersClient()

    enum Failure: LocalizedError {
        case signedOut
        case auth(Int)
        case server(String)
        case notAvailable
        /// 409 from the host relay: the computer is asleep and is never woken to be read.
        case asleep(ConsoleSnapshot?)
        /// 403 `needs_elevation` from `POST host/actions`: the one-time admin helper is not set up on that computer.
        case needsElevation(String)
        /// 403 `whole_disk_off` from a Files route: the path is outside the Files drive and whole-disk
        /// browsing is off. Carries the server's plain message.
        case wholeDiskOff(String)

        var errorDescription: String? {
            switch self {
            case .signedOut: return "Sign in required"
            case .auth(let status):
                return status == 401
                    ? "Sign in to Allternit to see your phones."
                    : "This Allternit account can’t manage phones."
            case .server(let message): return message
            case .notAvailable: return "Not available on your account yet."
            case .asleep: return "Asleep, not woken."
            case .needsElevation: return "This change needs the one-time admin helper on that computer."
            case .wholeDiskOff(let message): return message
            }
        }
    }

    // MARK: Linked phones

    struct CallBridge: Decodable, Sendable, Equatable {
        let enabled: Bool
        let botName: String?
    }

    /// One linked phone as the relay lists it; `online` = a Desktop session is attached.
    struct CloudPhone: Decodable, Sendable, Identifiable, Equatable {
        let serial: String
        let model: String
        let name: String
        let online: Bool
        let callBridge: CallBridge?
        let route: String?
        let screen: Bool?
        /// Android app phones: `platform` "android", route `android-app`, taps on when Accessibility is on.
        let platform: String?
        let canTap: Bool?
        let consent: String?
        /// Phone Use: set on a second iPhone or iPad in keyboard mode (the
        /// name of the iPhone it types for, "" when not known). Wire name kept.
        let keyboard_for: String?
        /// Phone Use routes the node lists for an app iPhone (contract §9.3).
        let routes: [RouteRow]?

        struct RouteRow: Decodable, Sendable, Equatable {
            let route: String?
            let healthy: Bool?
        }

        var id: String { serial }

        /// A second iPhone or iPad acting as a Bluetooth keyboard: not a phone to act on.
        var isKeyboard: Bool { keyboard_for != nil }

        /// "Keyboard for Sam's iPhone" (the web's keyboardLabel).
        var keyboardLabel: String {
            let name = (keyboard_for ?? "").trimmingCharacters(in: .whitespaces)
            return name.isEmpty ? "Keyboard for your iPhone" : "Keyboard for \(name)"
        }

        /// A way to tap is ready for this iPhone (a Bluetooth keyboard the node can see).
        var tapRouteReady: Bool { routes?.contains { $0.healthy == true && $0.route != "official" } == true }

        /// The Allternit Android app sharing its screen (live view opens; no cable bridge).
        var isAndroidApp: Bool {
            serial.hasPrefix("app:") && (route == "android-app" || platform == "android")
        }

        /// app-phones.ts `appPhoneFromCloud`: the Allternit iPhone app (`app:…`).
        var isAppPhone: Bool {
            serial.hasPrefix("app:") && !isAndroidApp && (route == nil || route == "iphone-app-store")
        }

        /// The app phone is live while it shares its screen; others while Desktop is attached.
        var isLive: Bool { isAppPhone ? (screen ?? online) && online : online }

        /// Desktop relays a linked iPhone with model "iPhone" (no platform field).
        var isIphone: Bool { !isAndroidApp && (model == "iPhone" || serial.hasPrefix("app:")) }

        /// phones-cloud.ts `isStaleWirelessRow`: an offline `ip:port` row never comes back.
        var isStaleWireless: Bool {
            !online && serial.range(of: #"^\d{1,3}(\.\d{1,3}){3}:\d{1,5}$"#, options: .regularExpression) != nil
        }
    }

    private struct PhonesEnvelope: Decodable {
        let phones: [ACILossy<CloudPhone>]?
    }

    func listPhones() async throws -> [CloudPhone] {
        let data = try await cloud("api/v1/phones", authFailure: true)
        let envelope = (try? JSONDecoder().decode(PhonesEnvelope.self, from: data))
        return (envelope?.phones ?? []).compactMap(\.value).filter { !$0.isStaleWireless }
    }

    // MARK: Operator keys

    struct OperatorKey: Decodable, Sendable, Identifiable, Equatable {
        let id: String
        let name: String
        let prefix: String
        let scopes: [String]?
        let token: String?
    }

    func listOperatorKeys() async throws -> [OperatorKey] {
        let data = try await cloud("api/v1/api-keys")
        return (try? JSONDecoder().decode([OperatorKey].self, from: data)) ?? []
    }

    private struct MintBody: Encodable {
        let name: String
        let scopes: [String]
    }

    func mintOperatorKey(name: String, canRunModels: Bool) async throws -> OperatorKey {
        let body = try JSONEncoder().encode(MintBody(name: name, scopes: canRunModels ? ["compute", "inference"] : ["compute"]))
        let data = try await cloud("api/v1/api-keys", method: "POST", body: body)
        do {
            return try JSONDecoder().decode(OperatorKey.self, from: data)
        } catch {
            throw Failure.server("The key response was unreadable.")
        }
    }

    func revokeOperatorKey(id: String) async throws {
        let escaped = id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? id
        _ = try await cloud("api/v1/api-keys/\(escaped)", method: "DELETE")
    }

    // MARK: Workflow specs (on a paired computer)

    struct WorkflowSpec: Decodable, Sendable, Identifiable, Equatable {
        let skillId: String
        let title: String?
        let hasNetworkTrace: Bool?

        var id: String { skillId }

        enum CodingKeys: String, CodingKey {
            case skillId = "skill_id"
            case title
            case hasNetworkTrace
        }
    }

    private struct SpecsEnvelope: Decodable {
        let specs: [ACILossy<WorkflowSpec>]?
    }

    func listWorkflowSpecs(runtimeId: String) async throws -> [WorkflowSpec] {
        var request = URLRequest(url: RuntimeRelayClient.relayBaseURL(runtimeId: runtimeId).appendingPathComponent("v1/browser-skills"))
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        let (data, response) = try await RuntimeRelayClient.shared.send(request, runtimeId: runtimeId)
        try Self.check(response, data: data)
        let envelope = try? JSONDecoder().decode(SpecsEnvelope.self, from: data)
        return (envelope?.specs ?? []).compactMap(\.value)
    }

    // MARK: Computers console (GET /api/v1/computers/console, camelCase)

    struct ConsoleSnapshot: Decodable, Sendable, Equatable {
        struct CPU: Decodable, Sendable, Equatable { let percent: Double }
        struct Memory: Decodable, Sendable, Equatable { let usedBytes: Double; let totalBytes: Double }
        struct Disk: Decodable, Sendable, Equatable { let mount: String; let usedBytes: Double; let totalBytes: Double }
        let sampledAt: String
        let cpu: CPU
        let memory: Memory
        let disk: [Disk]

        var memoryPercent: Double { memory.totalBytes > 0 ? memory.usedBytes / memory.totalBytes * 100 : 0 }
        /// The fullest disk, as a percent.
        var diskPercent: Double { disk.map { $0.totalBytes > 0 ? $0.usedBytes / $0.totalBytes * 100 : 0 }.max() ?? 0 }
    }

    /// One row of the console list (contract `ConsoleComputer`).
    struct ConsoleComputer: Decodable, Sendable, Identifiable, Equatable {
        let id: String
        let kind: String          // cloud | paired | desktop
        let group: String
        let name: String
        let provider: String
        let region: String?
        let os: String
        let osFamily: String
        let address: String?
        let reach: String
        let status: String
        let lastSample: ConsoleSnapshot?
        let servicesCount: Int?
        let lastBackupAt: String?
        /// The runtime device behind this row (console 2.3). Relay
        /// (`allternit-relay://`), Terminal and the Files drive are keyed by it;
        /// the row id stays the key for every `/computers/:id/*` route.
        let runtimeId: String?

        var relayId: String { runtimeId ?? id }
        /// Every id of this machine (one row per machine since #2825); empty from an older server.
        let aliases: [String]
        /// The paired-row id, when this machine has one (paired-only actions use it).
        let pairedId: String?

        enum CodingKeys: String, CodingKey {
            case id, kind, group, name, provider, region, os, osFamily, address, reach, status
            case lastSample, servicesCount, lastBackupAt, aliases, pairedId, runtimeId, wholeDisk
        }

        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            id = try c.decode(String.self, forKey: .id)
            kind = try c.decode(String.self, forKey: .kind)
            group = try c.decode(String.self, forKey: .group)
            name = try c.decode(String.self, forKey: .name)
            provider = try c.decode(String.self, forKey: .provider)
            region = try c.decodeIfPresent(String.self, forKey: .region)
            os = try c.decode(String.self, forKey: .os)
            osFamily = try c.decode(String.self, forKey: .osFamily)
            address = try c.decodeIfPresent(String.self, forKey: .address)
            reach = try c.decode(String.self, forKey: .reach)
            status = try c.decode(String.self, forKey: .status)
            lastSample = try c.decodeIfPresent(ConsoleSnapshot.self, forKey: .lastSample)
            servicesCount = try c.decodeIfPresent(Int.self, forKey: .servicesCount)
            lastBackupAt = try c.decodeIfPresent(String.self, forKey: .lastBackupAt)
            aliases = (try? c.decodeIfPresent([String].self, forKey: .aliases)) ?? []
            pairedId = try? c.decodeIfPresent(String.self, forKey: .pairedId)
            runtimeId = try c.decodeIfPresent(String.self, forKey: .runtimeId)
            wholeDisk = try c.decodeIfPresent(WholeDisk.self, forKey: .wholeDisk)
        }

        /// True when `other` is this machine's `id` or any of its aliases.
        func matches(_ other: String) -> Bool { id == other || aliases.contains(other) }
        /// Whole-disk Files (platform #2829). Missing from an older server = unavailable.
        var wholeDisk: WholeDisk? = nil

        /// Always a full value, so callers never branch on a missing field.
        var wholeDiskState: WholeDisk { wholeDisk ?? WholeDisk(enabled: false, available: false, reason: nil, roots: []) }

        var isOnline: Bool { !["offline", "asleep", "stopped"].contains(status.lowercased()) }
        var isAsleep: Bool { status.lowercased() == "asleep" }

        /// Plain reasons this computer needs a look (derived from the last sample).
        var attention: [String] {
            var out: [String] = []
            if status.lowercased() == "offline" { out.append("Offline") }
            if let s = lastSample {
                if s.cpu.percent >= 90 { out.append("CPU at \(Int(s.cpu.percent))%") }
                if s.memoryPercent >= 90 { out.append("Memory at \(Int(s.memoryPercent))%") }
                if s.diskPercent >= 90 { out.append("Disk \(Int(s.diskPercent))% full") }
            }
            return out
        }
    }

    /// One whole-disk root: `/` on Linux and macOS, one entry per drive letter on Windows.
    struct WholeDiskRoot: Decodable, Sendable, Equatable, Identifiable {
        let id: String
        let path: String
        let label: String
    }

    /// `wholeDisk` on a console row and in the PATCH answer. Lenient: a missing or odd field never drops the row.
    struct WholeDisk: Decodable, Sendable, Equatable {
        let enabled: Bool
        let available: Bool
        let reason: String?
        let roots: [WholeDiskRoot]

        init(enabled: Bool, available: Bool, reason: String?, roots: [WholeDiskRoot]) {
            self.enabled = enabled; self.available = available; self.reason = reason; self.roots = roots
        }

        enum CodingKeys: String, CodingKey { case enabled, available, reason, roots }

        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            enabled = (try? c.decode(Bool.self, forKey: .enabled)) ?? false
            available = (try? c.decode(Bool.self, forKey: .available)) ?? false
            reason = try? c.decode(String.self, forKey: .reason)
            roots = ((try? c.decode([ACILossy<WholeDiskRoot>].self, forKey: .roots)) ?? []).compactMap(\.value)
        }

        /// The roots switcher shows only while the setting is on and the server sent roots.
        var showsRootsSwitcher: Bool { enabled && !roots.isEmpty }

        /// Plain words for `reason`, shown as text when the switch is not available.
        var reasonCopy: String {
            switch reason {
            case "not_owner": return "Only the owner of this computer can turn this on."
            case "not_a_server": return "Allternit Desktop already reaches its own files, so this doesn’t apply."
            case "no_runtime": return "Pair this computer first, then you can turn this on."
            default: return "Not available on this computer."
            }
        }
    }

    static let wholeDiskWarning = "Anyone using this computer from Allternit can open, change or delete any file on it, not only the Files drive."

    private struct WholeDiskAnswer: Decodable { let wholeDisk: WholeDisk? }

    /// PATCH /api/v1/computers/:id/file-roots. Turning on sends `confirm: true`; the caller held to confirm first.
    /// Owner only; 403/409 carry a plain `message` the user sees as is.
    func setWholeDisk(id: String, on: Bool) async throws -> WholeDisk {
        let enc = id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? id
        let payload: [String: Any] = on ? ["wholeDisk": true, "confirm": true] : ["wholeDisk": false]
        let body = try JSONSerialization.data(withJSONObject: payload)
        let data = try await cloud("api/v1/computers/\(enc)/file-roots", method: "PATCH", body: body)
        let answer = try? JSONDecoder().decode(WholeDiskAnswer.self, from: data)
        return answer?.wholeDisk ?? WholeDisk(enabled: on, available: true, reason: nil, roots: [])
    }

    private struct ConsoleEnvelope: Decodable {
        let computers: [ACILossy<ConsoleComputer>]?
    }

    /// Throws `Failure.notAvailable` while the backend route does not exist for this account.
    func listConsole() async throws -> [ConsoleComputer] {
        let data = try await cloud("api/v1/computers/console")
        let decoder = JSONDecoder()
        if let rows = try? decoder.decode([ACILossy<ConsoleComputer>].self, from: data) {
            return rows.compactMap(\.value)
        }
        let envelope = try? decoder.decode(ConsoleEnvelope.self, from: data)
        return (envelope?.computers ?? []).compactMap(\.value)
    }

    // MARK: Console detail (GET /api/v1/computers/:id/host/* and /metrics)

    /// Contract `HostSnapshot`: the live read behind the detail Overview.
    struct HostSnapshot: Decodable, Sendable, Equatable {
        struct OS: Decodable, Sendable, Equatable { let name: String; let version: String?; let family: String?; let arch: String? }
        struct CPU: Decodable, Sendable, Equatable { let percent: Double; let cores: Int? }
        struct Memory: Decodable, Sendable, Equatable { let usedBytes: Double; let totalBytes: Double }
        struct Disk: Decodable, Sendable, Equatable { let mount: String; let usedBytes: Double; let totalBytes: Double }
        struct Net: Decodable, Sendable, Equatable { let rxBytesPerSec: Double; let txBytesPerSec: Double }
        let sampledAt: String?
        let uptimeSec: Double?
        let os: OS?
        let cpu: CPU
        let memory: Memory
        let disk: [Disk]
        let net: Net?

        var memoryPercent: Double { memory.totalBytes > 0 ? memory.usedBytes / memory.totalBytes * 100 : 0 }
        var diskPercent: Double { disk.map { $0.totalBytes > 0 ? $0.usedBytes / $0.totalBytes * 100 : 0 }.max() ?? 0 }
    }

    struct HostService: Decodable, Sendable, Identifiable, Equatable {
        let name: String
        let unit: String
        let state: String          // running | stopped | failed | unknown
        let enabled: Bool?
        let description: String?
        let memoryBytes: Double?
        var id: String { unit }
    }

    struct HostPort: Decodable, Sendable, Identifiable, Equatable {
        let port: Int
        let `protocol`: String
        let address: String?
        let process: String?
        let exposure: String       // loopback | lan | public
        var id: String { "\(`protocol`)-\(port)-\(address ?? "")" }
    }

    struct LogLine: Decodable, Sendable, Identifiable, Equatable {
        let ts: String
        let unit: String
        let level: String          // debug | info | warn | error
        let message: String
        var id: String { "\(ts)-\(unit)-\(message.hashValue)" }
    }

    /// Contract `MetricSeries`; `points` are `[ISO timestamp, value]` pairs, oldest first.
    struct MetricSeries: Decodable, Sendable, Equatable {
        let metric: String         // cpu | memory | disk | net_rx | net_tx
        let values: [Double]

        private enum CodingKeys: String, CodingKey { case metric, points }
        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            metric = try c.decode(String.self, forKey: .metric)
            var out: [Double] = []
            var list = try c.nestedUnkeyedContainer(forKey: .points)
            while !list.isAtEnd {
                var pair = try list.nestedUnkeyedContainer()
                _ = try? pair.decode(String.self)
                if let v = try? pair.decode(Double.self) { out.append(v) }
            }
            values = out
        }
    }

    private struct ServicesEnvelope: Decodable { let services: [ACILossy<HostService>]? }
    private struct PortsEnvelope: Decodable { let ports: [ACILossy<HostPort>]? }
    private struct MetricsEnvelope: Decodable { let series: [ACILossy<MetricSeries>]? }

    private func hostPath(_ id: String, _ tail: String) -> String {
        "api/v1/computers/\(id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? id)/\(tail)"
    }

    func hostSnapshot(id: String) async throws -> HostSnapshot {
        try JSONDecoder().decode(HostSnapshot.self, from: try await cloud(hostPath(id, "host/snapshot")))
    }

    func hostServices(id: String) async throws -> [HostService] {
        let env = try? JSONDecoder().decode(ServicesEnvelope.self, from: try await cloud(hostPath(id, "host/services")))
        return (env?.services ?? []).compactMap(\.value)
    }

    func hostPorts(id: String) async throws -> [HostPort] {
        let env = try? JSONDecoder().decode(PortsEnvelope.self, from: try await cloud(hostPath(id, "host/ports")))
        return (env?.ports ?? []).compactMap(\.value)
    }

    func metrics(id: String) async throws -> [MetricSeries] {
        let data = try await cloud(hostPath(id, "metrics?window=24h"))
        let decoder = JSONDecoder()
        if let rows = try? decoder.decode([ACILossy<MetricSeries>].self, from: data) { return rows.compactMap(\.value) }
        let env = try? decoder.decode(MetricsEnvelope.self, from: data)
        return (env?.series ?? []).compactMap(\.value)
    }

    /// Live log lines for one unit over SSE (`GET host/logs/stream?unit=`). The
    /// stream ends when the caller's task is cancelled; lines are never stored.
    func logStream(id: String, unit: String) -> AsyncThrowingStream<LogLine, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    var comps = URLComponents(url: AppConfig.cloudAPIBaseURL.appendingPathComponent(hostPath(id, "host/logs/stream")), resolvingAgainstBaseURL: false)
                    comps?.queryItems = [URLQueryItem(name: "unit", value: unit)]
                    guard let url = comps?.url else { throw Failure.server("Couldn't load this. Try again.") }
                    var request = URLRequest(url: url)
                    request.setValue("text/event-stream", forHTTPHeaderField: "Accept")
                    request.timeoutInterval = 3600
                    let token: String?
                    if let clerk = try await AuthManager.shared.getToken() { token = clerk } else { token = try await AuthManager.shared.effectiveToken() }
                    guard let token, !token.isEmpty else { throw Failure.signedOut }
                    request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
                    let (bytes, response) = try await URLSession.shared.bytes(for: request)
                    if let http = response as? HTTPURLResponse, !(200...299).contains(http.statusCode) {
                        var body = Data()
                        for try await b in bytes { body.append(b); if body.count > 4096 { break } }
                        try Self.check(response, data: body)
                    }
                    for try await line in bytes.lines {
                        guard line.hasPrefix("data:") else { continue }
                        let payload = line.dropFirst(5).trimmingCharacters(in: .whitespaces)
                        if let d = payload.data(using: .utf8), let l = try? JSONDecoder().decode(LogLine.self, from: d) {
                            continuation.yield(l)
                        }
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    // MARK: Console manage (findings, approvals, SSH keys, backups, settings)

    /// Contract `HostAction` (`{ type, target }`); `label` is the plain sentence the node may add.
    struct HostAction: Decodable, Sendable, Equatable {
        let type: String
        let target: String
        let label: String?
    }

    struct HostFinding: Decodable, Sendable, Identifiable, Equatable {
        let id: String
        let severity: String      // info | warning | critical
        let category: String      // disk | security | service | update | config
        let title: String
        let detail: String
        let suggestedAction: HostAction?
    }

    struct HostActionResult: Decodable, Sendable, Equatable {
        let changeId: String
        let undo: HostAction?
    }

    struct SSHKey: Decodable, Sendable, Identifiable, Equatable {
        let id: String
        let label: String
        let fingerprint: String?
        let createdAt: String?
        let kind: String?         // "agent" keys are short-lived
    }

    struct CloudBackup: Decodable, Sendable, Identifiable, Equatable {
        let id: String
        let createdAt: String
        let sizeBytes: Double?
        let kind: String?         // optional; absent on older servers
    }

    /// The backups list plus whether the server allows restoring (`canRestore`; absent means yes).
    struct CloudBackupList: Sendable {
        let backups: [CloudBackup]
        let canRestore: Bool
    }

    private struct FindingsEnvelope: Decodable { let findings: [ACILossy<HostFinding>]? }
    private struct KeysEnvelope: Decodable { let keys: [ACILossy<SSHKey>]? }
    private struct BackupsEnvelope: Decodable { let backups: [ACILossy<CloudBackup>]?; let canRestore: Bool? }
    private struct ApprovalEnvelope: Decodable { let approvalId: String?; let approval_id: String? }

    private func jsonBody(_ dict: [String: Any]) -> Data? { try? JSONSerialization.data(withJSONObject: dict) }

    func hostFindings(id: String) async throws -> [HostFinding] {
        let env = try? JSONDecoder().decode(FindingsEnvelope.self, from: try await cloud(hostPath(id, "host/findings")))
        return (env?.findings ?? []).compactMap(\.value)
    }

    /// The cloud issues one approval id bound to this action, this computer and this user.
    func requestApproval(id: String, type: String, target: String) async throws -> String {
        let data = try await cloud(hostPath(id, "approvals"), method: "POST", body: jsonBody(["type": type, "target": target]))
        let env = try? JSONDecoder().decode(ApprovalEnvelope.self, from: data)
        guard let approval = env?.approvalId ?? env?.approval_id, !approval.isEmpty else {
            throw Failure.server("Couldn't get an approval for this change. Nothing was changed.")
        }
        return approval
    }

    func runHostAction(id: String, type: String, target: String, approvalId: String) async throws -> HostActionResult {
        let data = try await cloud(hostPath(id, "host/actions"), method: "POST",
                                   body: jsonBody(["type": type, "target": target, "approval_id": approvalId]))
        guard let result = try? JSONDecoder().decode(HostActionResult.self, from: data) else {
            throw Failure.server("The change ran but the answer was unreadable. Check Activity.")
        }
        return result
    }

    /// A saved command (contract: `GET /api/v1/computers/snippets` → `{ snippets: [...] }`).
    struct Snippet: Decodable, Sendable, Identifiable, Equatable {
        let id: String
        let name: String
        let command: String
        let description: String?
    }

    private struct SnippetsEnvelope: Decodable { let snippets: [ACILossy<Snippet>]? }

    func snippets() async throws -> [Snippet] {
        let data = try await cloud("api/v1/computers/snippets")
        let decoder = JSONDecoder()
        if let rows = try? decoder.decode([ACILossy<Snippet>].self, from: data) { return rows.compactMap(\.value) }
        let env = try? decoder.decode(SnippetsEnvelope.self, from: data)
        return (env?.snippets ?? []).compactMap(\.value)
    }

    func sshKeys(id: String) async throws -> [SSHKey] {
        let data = try await cloud(hostPath(id, "ssh-keys"))
        let decoder = JSONDecoder()
        if let rows = try? decoder.decode([ACILossy<SSHKey>].self, from: data) { return rows.compactMap(\.value) }
        return ((try? decoder.decode(KeysEnvelope.self, from: data))?.keys ?? []).compactMap(\.value)
    }

    func addSSHKey(id: String, label: String, publicKey: String) async throws {
        _ = try await cloud(hostPath(id, "ssh-keys"), method: "POST", body: jsonBody(["label": label, "publicKey": publicKey]))
    }

    func removeSSHKey(id: String, keyId: String) async throws {
        let key = keyId.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? keyId
        _ = try await cloud(hostPath(id, "ssh-keys/\(key)"), method: "DELETE")
    }

    /// Orgo daily backups (cloud computers only).
    func cloudBackups(id: String) async throws -> [CloudBackup] {
        try await cloudBackupList(id: id).backups
    }

    func cloudBackupList(id: String) async throws -> CloudBackupList {
        let data = try await cloud(hostPath(id, "backups"))
        let decoder = JSONDecoder()
        if let rows = try? decoder.decode([ACILossy<CloudBackup>].self, from: data) {
            return CloudBackupList(backups: rows.compactMap(\.value), canRestore: true)
        }
        let env = try? decoder.decode(BackupsEnvelope.self, from: data)
        return CloudBackupList(backups: (env?.backups ?? []).compactMap(\.value), canRestore: env?.canRestore ?? true)
    }

    /// Restore is in place: changes since the backup are erased, so the server needs `confirm: true`.
    func restoreCloudBackup(id: String, backupId: String) async throws {
        _ = try await cloud(hostPath(id, "backups/restore"), method: "POST", body: jsonBody(["backupId": backupId, "confirm": true]))
    }

    /// Cloud settings: restart, move (`location` = nyc | chi | lon | syd), rebuild.
    /// Move and rebuild erase the disk, so both send `confirm: true` (the
    /// caller shows a hold-to-confirm first); without it the server answers 400.
    func restartCloud(id: String) async throws { _ = try await cloud(hostPath(id, "restart"), method: "POST") }
    func moveCloud(id: String, location: String) async throws {
        let body = try? JSONSerialization.data(withJSONObject: ["location": location, "confirm": true] as [String: Any])
        _ = try await cloud(hostPath(id, "move"), method: "POST", body: body)
    }
    func rebuildCloud(id: String) async throws {
        let body = try? JSONSerialization.data(withJSONObject: ["confirm": true])
        _ = try await cloud(hostPath(id, "rebuild"), method: "POST", body: body)
    }

    /// Paired computers: rename. `removePaired` only drops the paired row; Remove in the UI uses `unpair`.
    func renamePaired(id: String, name: String) async throws {
        let enc = id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? id
        _ = try await cloud("api/v1/computers/paired/\(enc)", method: "PATCH", body: jsonBody(["name": name]))
    }
    /// Remove a machine: `POST /api/v1/computers/:id/unpair {confirm:true}` revokes every
    /// alias of the machine (a bare `DELETE paired/:id` would leave its runtimes paired).
    func unpair(id: String) async throws {
        let enc = id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? id
        let body = try? JSONSerialization.data(withJSONObject: ["confirm": true])
        _ = try await cloud("api/v1/computers/\(enc)/unpair", method: "POST", body: body)
    }

    /// Plain sentence for each unpair error code (400 confirm_required, 403 not_owner,
    /// 404 not found, 409 cloud_computer).
    static func unpairMessage(_ code: String) -> String {
        switch code {
        case "confirm_required": return "Confirm to remove this computer. Nothing was changed."
        case "not_owner": return "Only the person who paired this computer can remove it."
        case "cloud_computer": return "This is an Allternit cloud computer. It can't be unpaired. Remove it from your cloud settings."
        case "not_found", "computer_not_found": return "This computer is already gone. Pull to refresh."
        default: return "Couldn't remove this computer. Nothing was changed."
        }
    }

    func removePaired(id: String) async throws {
        let enc = id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? id
        _ = try await cloud("api/v1/computers/paired/\(enc)", method: "DELETE")
    }

    // MARK: Plumbing

    private func cloud(_ path: String, method: String = "GET", body: Data? = nil, authFailure: Bool = false) async throws -> Data {
        var request = URLRequest(url: AppConfig.cloudAPIBaseURL.appendingPathComponent(path))
        request.httpMethod = method
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        if let body {
            request.httpBody = body
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }
        let token: String?
        if let clerk = try await AuthManager.shared.getToken() {
            token = clerk
        } else {
            token = try await AuthManager.shared.effectiveToken()
        }
        guard let token, !token.isEmpty else {
            throw authFailure ? Failure.auth(401) : Failure.signedOut
        }
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        let (data, response) = try await URLSession.shared.data(for: request)
        if authFailure, let http = response as? HTTPURLResponse, http.statusCode == 401 || http.statusCode == 403 {
            throw Failure.auth(http.statusCode)
        }
        try Self.check(response, data: data)
        return data
    }

    /// Non-2xx → the server's `detail` / `message` / `error`, else a plain
    /// sentence (never a raw status code): a 502/503/504 with no body is the
    /// computer or relay not answering.
    static func check(_ response: URLResponse, data: Data) throws {
        guard let http = response as? HTTPURLResponse else { throw Failure.server("Couldn't load this. Try again.") }
        guard (200...299).contains(http.statusCode) else {
            let path = http.url?.path ?? ""
            if path.hasSuffix("/unpair") {
                struct UnpairError: Decodable { let error: String?; let message: String? }
                let body = try? JSONDecoder().decode(UnpairError.self, from: data)
                if let code = body?.error,
                   ["confirm_required", "not_owner", "cloud_computer", "not_found", "computer_not_found"].contains(code) {
                    throw Failure.server(unpairMessage(code))
                }
                if [404, 405, 501].contains(http.statusCode) { throw Failure.notAvailable }
                if let message = body?.message, !message.isEmpty { throw Failure.server(message) }
            }
            let consolePath = path.hasSuffix("computers/console") || path.contains("/host/") || path.hasSuffix("/metrics")
                || path.contains("/ssh-keys") || path.hasSuffix("/backups") || path.hasSuffix("/backups/restore")
                || path.hasSuffix("/approvals") || path.hasSuffix("/restart") || path.hasSuffix("/move")
                || path.hasSuffix("/rebuild") || path.contains("/computers/paired/") || path.hasSuffix("/computers/snippets")
            if http.statusCode == 404, path.hasSuffix("/file-roots") {
                throw Failure.server("This computer isn't on your account any more.")
            }
            // Restore answers with its own plain `message` (501 while the server gate is off,
            // 404 backup_not_found, 409 restore_unavailable / busy): show it, never "not available".
            let isRestore = path.hasSuffix("/backups/restore")
            if [404, 405, 501].contains(http.statusCode), consolePath, !isRestore {
                throw Failure.notAvailable
            }
            if http.statusCode == 409, consolePath {
                struct Asleep: Decodable { let error: String?; let lastSample: ConsoleSnapshot? }
                let body = try? JSONDecoder().decode(Asleep.self, from: data)
                // A 409 with another reason (for example a refused action) is not "asleep".
                if body?.error == nil || body?.error == "asleep" { throw Failure.asleep(body?.lastSample) }
            }
            if http.statusCode == 403 {
                struct Elevation: Decodable { let error: String?; let message: String? }
                if let e = try? JSONDecoder().decode(Elevation.self, from: data) {
                    if e.error == "needs_elevation" { throw Failure.needsElevation(e.message ?? "") }
                    if e.error == "whole_disk_off" {
                        throw Failure.wholeDiskOff(e.message ?? "Turn on whole-disk browsing for this computer in its Settings first.")
                    }
                }
            }
            struct Body: Decodable { let detail: String?; let message: String?; let error: String? }
            let body = try? JSONDecoder().decode(Body.self, from: data)
            if let text = body?.detail ?? body?.message ?? body?.error, !text.isEmpty {
                throw Failure.server(text)
            }
            throw Failure.server((502...504).contains(http.statusCode)
                ? "This computer isn't responding."
                : "Couldn't load this. Try again.")
        }
    }
}

/// Decodes one array element, or nil when it is malformed (the web filters
/// rows that fail `isSummary` instead of failing the whole list).
struct ACILossy<T: Decodable & Sendable>: Decodable, Sendable {
    let value: T?

    init(from decoder: Decoder) throws {
        value = try? T(from: decoder)
    }
}
