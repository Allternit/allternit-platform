import Foundation

/// Registers this iPhone's APNs token with cloud-api
/// (`cmd/allternit-cloud-api/src/routes/apns.rs`):
/// - `POST   {cloud}/api/v1/device-tokens` `{token, platform:"ios", environment, deviceId}`
/// - `DELETE {cloud}/api/v1/device-tokens` `{token}` (sign-out)
///
/// Goes DIRECT to `AppConfig.cloudAPIBaseURL` with the Bearer (like
/// CoworkTasksClient) — never through the runtime relay, which would hand
/// the token to the user's computer instead of the cloud. A 503
/// `apns_not_configured` means the server's APNs key isn't set yet.
final class PushTokenClient: Sendable {
    static let shared = PushTokenClient()

    private init() {}

    private var url: URL { AppConfig.cloudAPIBaseURL.appendingPathComponent("api/v1/device-tokens") }

    /// Debug builds get sandbox tokens; TestFlight/App Store get production.
    static var environment: String {
        #if DEBUG
        "sandbox"
        #else
        "production"
        #endif
    }

    /// What this build can do with a push. `la_push`: Live Activity pushes
    /// (plan I3; tokens upload only while the Live Activities switch is on).
    /// `widget_push` joins with plan I6.
    static let capabilities = ["actions", "nse", "la_push", "widget_push"]

    /// Registers the token; returns the device read token the cloud hands
    /// out once (nil when it already did, or an older server).
    @discardableResult
    func register(token: String, deviceId: String?, osVersion: String) async throws -> String? {
        struct Body: Encodable {
            let token: String
            let platform: String
            let environment: String
            let deviceId: String?
            let osVersion: String
            let appBuild: String
            let capabilities: [String]
        }
        let build = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? ""
        let data = try await send(method: "POST", body: Body(
            token: token, platform: "ios", environment: Self.environment, deviceId: deviceId,
            osVersion: osVersion, appBuild: build, capabilities: Self.capabilities
        ))
        struct Answer: Decodable { let readToken: String? }
        return (try? JSONDecoder().decode(Answer.self, from: data))?.readToken
    }

    func unregister(token: String) async throws {
        struct Body: Encodable { let token: String }
        _ = try await send(method: "DELETE", body: Body(token: token))
    }

    private func send<B: Encodable>(method: String, body: B) async throws -> Data {
        let client = APIClient.shared
        var request = try await client.authorizedRequest(url: url, method: method)
        request.httpBody = try JSONEncoder().encode(body)
        let (data, response) = try await client.session.data(for: request)
        try client.validate(response, data: data)
        return data
    }
}

/// Small per-account UI flags — `GET/PUT {cloud}/api/v1/me/flags`
/// (cmd/allternit-cloud-api/src/routes/me_flags.rs). The web writes the same
/// keys into Clerk `unsafeMetadata`; this endpoint reads/writes that store,
/// so "Meet your bots" seen on the web stays seen on the iPhone.
final class AccountFlagsClient: Sendable {
    static let shared = AccountFlagsClient()

    private init() {}

    private var url: URL { AppConfig.cloudAPIBaseURL.appendingPathComponent("api/v1/me/flags") }

    /// The flag's string value, nil when unset. Throws when unreachable.
    func string(_ key: String) async throws -> String? {
        struct Response: Decodable { let flags: [String: JSONValue]? }
        let client = APIClient.shared
        let request = try await client.authorizedRequest(url: url)
        let (data, response) = try await client.session.data(for: request)
        try client.validate(response, data: data)
        let decoded = try JSONDecoder().decode(Response.self, from: data)
        if case .string(let value)? = decoded.flags?[key], !value.isEmpty { return value }
        return nil
    }

    func set(_ key: String, to value: String) async throws {
        let client = APIClient.shared
        var request = try await client.authorizedRequest(url: url, method: "PUT")
        request.httpBody = try JSONEncoder().encode([key: value])
        let (data, response) = try await client.session.data(for: request)
        try client.validate(response, data: data)
    }
}
