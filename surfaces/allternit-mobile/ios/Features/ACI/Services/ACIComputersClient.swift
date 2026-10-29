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

        var errorDescription: String? {
            switch self {
            case .signedOut: return "Sign in required"
            case .auth(let status):
                return status == 401
                    ? "Sign in to Allternit to see your phones."
                    : "This Allternit account can’t manage phones."
            case .server(let message): return message
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

        var id: String { serial }

        /// app-phones.ts `appPhoneFromCloud`: the Allternit iPhone app (`app:…`).
        var isAppPhone: Bool {
            serial.hasPrefix("app:") && (route == nil || route == "iphone-app-store")
        }

        /// The app phone is live while it shares its screen; others while Desktop is attached.
        var isLive: Bool { isAppPhone ? (screen ?? online) && online : online }

        /// Desktop relays a linked iPhone with model "iPhone" (no platform field).
        var isIphone: Bool { model == "iPhone" || serial.hasPrefix("app:") }

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

    /// Non-2xx → the server's `detail` / `message` / `error`, else the status.
    private static func check(_ response: URLResponse, data: Data) throws {
        guard let http = response as? HTTPURLResponse else { throw Failure.server("Request failed") }
        guard (200...299).contains(http.statusCode) else {
            struct Body: Decodable { let detail: String?; let message: String?; let error: String? }
            let body = try? JSONDecoder().decode(Body.self, from: data)
            let text = body?.detail ?? body?.message ?? body?.error
            throw Failure.server(text ?? "Request failed (\(http.statusCode))")
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
