import Foundation
import UIKit

/// Live Activity and widget push tokens, sent to cloud-api
/// (`cmd/allternit-cloud-api/src/routes/apns.rs`):
/// - `PUT    {cloud}/api/v1/device-tokens/activity`
///   `{deviceId, kind, attributesType?, activityKey?, token, environment}` → `{ok, id}`
/// - `DELETE {cloud}/api/v1/device-tokens/activity` `{token}` → `{ok, removed}`
///
/// `kind` is `la_start` (push-to-start, one per device and attributes type),
/// `la_update` (one running activity, keyed by `activityKey`) or `widget`.
/// The device must already be registered through `PushTokenClient` (same
/// `deviceId`), otherwise the PUT answers 404 `unknown_device` and
/// `ActivityTokenRegistry.resync()` retries after registration.
///
/// Direct to `AppConfig.cloudAPIBaseURL` with the Bearer, like `PushTokenClient`.
enum ActivityTokenKind: String, Sendable {
    case laStart = "la_start"
    case laUpdate = "la_update"
    case widget
}

/// One PUT body. Field names are the cloud route's (`ActivityBody`, camelCase).
/// Nil optionals are left out of the JSON.
struct ActivityTokenUpload: Codable, Equatable, Sendable {
    let deviceId: String
    let kind: String
    let attributesType: String?
    let activityKey: String?
    let token: String
    let environment: String

    /// The server keeps one token per slot; a newer token replaces the older.
    var slot: String { ActivityTokenUpload.slot(kind: kind, attributesType: attributesType, activityKey: activityKey) }

    static func slot(kind: String, attributesType: String?, activityKey: String?) -> String {
        [kind, attributesType ?? "", activityKey ?? ""].joined(separator: "|")
    }
}

protocol ActivityTokenTransport: Sendable {
    func put(_ upload: ActivityTokenUpload) async throws
    func delete(token: String) async throws
}

final class ActivityTokenClient: ActivityTokenTransport {
    static let shared = ActivityTokenClient()

    private init() {}

    private var url: URL { AppConfig.cloudAPIBaseURL.appendingPathComponent("api/v1/device-tokens/activity") }

    /// Hex form APNs and the cloud route expect (lowercase, no spaces).
    static func hex(_ token: Data) -> String {
        token.map { String(format: "%02x", $0) }.joined()
    }

    static func body(_ upload: ActivityTokenUpload) throws -> Data {
        let encoder = JSONEncoder()
        encoder.outputFormatting = .sortedKeys
        return try encoder.encode(upload)
    }

    func put(_ upload: ActivityTokenUpload) async throws {
        _ = try await send(method: "PUT", body: try Self.body(upload))
    }

    func delete(token: String) async throws {
        struct Body: Encodable { let token: String }
        _ = try await send(method: "DELETE", body: try JSONEncoder().encode(Body(token: token)))
    }

    private func send(method: String, body: Data) async throws -> Data {
        let client = APIClient.shared
        var request = try await client.authorizedRequest(url: url, method: method)
        request.httpBody = body
        let (data, response) = try await client.session.data(for: request)
        try client.validate(response, data: data)
        return data
    }
}

/// The device-local Live Activities switch: consent to upload Live Activity
/// push tokens. On by default; the Settings switch (plan I9) writes it.
/// Local activities still run with it off; only the server copy stops.
enum LiveActivityPushSetting {
    static let key = "liveActivities.pushTokensEnabled"

    static func isOn(_ defaults: UserDefaults = .standard) -> Bool {
        defaults.object(forKey: key) as? Bool ?? true
    }

    /// For the Settings switch: off deletes the server copies, on re-sends them.
    @MainActor
    static func set(_ on: Bool, defaults: UserDefaults = .standard) async {
        defaults.set(on, forKey: key)
        if on {
            await ActivityTokenRegistry.shared.resync()
        } else {
            await ActivityTokenRegistry.shared.deleteAll()
        }
    }
}

/// Keeps the cloud's copy of this phone's activity tokens in step with
/// ActivityKit. Remembers the latest token per slot (so a later sign-in or
/// switch-on can send it) and the uploaded ones (persisted, so sign-out after
/// a relaunch still deletes them).
@MainActor
final class ActivityTokenRegistry {
    static let shared = ActivityTokenRegistry(
        transport: ActivityTokenClient.shared,
        defaults: .standard,
        environment: PushTokenClient.environment,
        deviceId: { UIDevice.current.identifierForVendor?.uuidString },
        isSignedIn: { AuthManager.shared.isSignedIn }
    )

    static let uploadedKey = "liveActivities.uploadedTokens"

    private let transport: any ActivityTokenTransport
    private let defaults: UserDefaults
    private let environment: String
    private let deviceId: @MainActor () -> String?
    private let isSignedIn: @MainActor () -> Bool

    /// Latest token per slot, uploaded or not.
    private(set) var latest: [String: ActivityTokenUpload] = [:]
    /// What the server holds, as far as this phone knows.
    private(set) var uploaded: [String: ActivityTokenUpload] = [:]

    init(transport: any ActivityTokenTransport, defaults: UserDefaults, environment: String,
         deviceId: @escaping @MainActor () -> String?, isSignedIn: @escaping @MainActor () -> Bool) {
        self.transport = transport
        self.defaults = defaults
        self.environment = environment
        self.deviceId = deviceId
        self.isSignedIn = isSignedIn
        if let data = defaults.data(forKey: Self.uploadedKey),
           let saved = try? JSONDecoder().decode([ActivityTokenUpload].self, from: data) {
            uploaded = Dictionary(saved.map { ($0.slot, $0) }, uniquingKeysWith: { _, new in new })
        }
    }

    /// A new token from ActivityKit. Uploads it when the switch is on and
    /// someone is signed in; otherwise keeps it for `resync()`.
    func update(kind: ActivityTokenKind, attributesType: String?, activityKey: String?, token: Data) async {
        guard let deviceId = deviceId(), !token.isEmpty else { return }
        let upload = ActivityTokenUpload(
            deviceId: deviceId, kind: kind.rawValue, attributesType: attributesType,
            activityKey: activityKey, token: ActivityTokenClient.hex(token), environment: environment
        )
        latest[upload.slot] = upload
        await send(upload)
    }

    /// The activity behind this slot ended: its update token is dead.
    func remove(kind: ActivityTokenKind, attributesType: String?, activityKey: String?) async {
        let slot = ActivityTokenUpload.slot(kind: kind.rawValue, attributesType: attributesType, activityKey: activityKey)
        latest.removeValue(forKey: slot)
        guard let sent = uploaded.removeValue(forKey: slot) else { return }
        save()
        try? await transport.delete(token: sent.token)
    }

    /// Re-sends every known token: after the device registers, after
    /// sign-in, after the switch turns on.
    func resync() async {
        for upload in latest.values.sorted(by: { $0.slot < $1.slot }) where uploaded[upload.slot] != upload {
            await send(upload)
        }
    }

    /// Sign-out or switch off: delete the server copies. The latest tokens
    /// stay in memory so the next sign-in can send them again.
    func deleteAll() async {
        let sent = uploaded.values.sorted(by: { $0.slot < $1.slot })
        uploaded.removeAll()
        save()
        for upload in sent {
            try? await transport.delete(token: upload.token)
        }
    }

    private func send(_ upload: ActivityTokenUpload) async {
        guard LiveActivityPushSetting.isOn(defaults), isSignedIn() else { return }
        do {
            try await transport.put(upload)
            // A token sent while this one was in flight wins.
            if latest[upload.slot] == upload || latest[upload.slot] == nil {
                uploaded[upload.slot] = upload
                save()
            }
        } catch {
            // 404 unknown_device before the device token is registered,
            // offline, 503 apns_not_configured: resync() retries.
        }
    }

    private func save() {
        if let data = try? JSONEncoder().encode(Array(uploaded.values)) {
            defaults.set(data, forKey: Self.uploadedKey)
        }
    }
}
