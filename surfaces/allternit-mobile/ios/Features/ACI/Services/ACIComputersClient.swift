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

        var errorDescription: String? {
            switch self {
            case .signedOut: return "Sign in required"
            case .auth(let status):
                return status == 401
                    ? "Sign in to Allternit to see your phones."
                    : "This Allternit account can’t manage phones."
            case .server(let message): return message
            case .notAvailable: return "Not available on your account yet."
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
    private static func check(_ response: URLResponse, data: Data) throws {
        guard let http = response as? HTTPURLResponse else { throw Failure.server("Couldn't load this. Try again.") }
        guard (200...299).contains(http.statusCode) else {
            if [404, 501].contains(http.statusCode), http.url?.path.hasSuffix("computers/console") == true {
                throw Failure.notAvailable
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
