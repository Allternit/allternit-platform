import Foundation
import Security

/// What pairing leaves behind for the app and the AllternitScreen extension:
/// who this phone is, which Desktop (runtime device) it talks to, the app
/// phone key that mints relay tickets without a Clerk session, and the
/// session key both sides derived during pairing.
///
/// Compiled into both targets (Shared/AppPhone). Pure Foundation + Security.
struct AppPhoneCredentials: Codable, Sendable, Equatable {
    /// Lowercased UUID. The serial everywhere else is `app:<phoneId>`.
    var phoneId: String
    var name: String
    var runtimeDeviceId: String
    var runtimeName: String
    /// From `POST /api/v1/app-phones`; sent as `x-allternit-app-phone-key`.
    var appPhoneKey: String
    /// 32-byte HKDF output, hex. Seals every socket message.
    var sessionKeyHex: String
    /// The cloud host this phone was paired against, so the extension (which
    /// can't read the app's settings or launch arguments) dials the same one.
    var cloudBaseURL: String
    var pairedAt: Date

    var serial: String { "app:\(phoneId)" }
    var sessionKey: Data? { Data(hexString: sessionKeyHex) }
}

/// The shared keychain item holding `AppPhoneCredentials`. Lives in the
/// `com.allternit.mobile.shared` access group so the broadcast extension can
/// read it; the group's full name (with the team prefix) comes from the
/// `AllternitSharedKeychainGroup` Info.plist key of whichever target runs.
enum AppPhoneKeychain {
    private static let service = "com.allternit.mobile.app-phone"
    private static let account = "credentials"
    /// The phone id outlives an unpair, so re-pairing rotates the key of the
    /// same `app:<id>` rather than leaving a dead phone on the Desktop.
    private static let phoneIdAccount = "phone-id"

    static var accessGroup: String? {
        guard let value = Bundle.main.object(forInfoDictionaryKey: "AllternitSharedKeychainGroup") as? String,
              !value.isEmpty, !value.hasPrefix("$(") else { return nil }
        return value
    }

    static func load() -> AppPhoneCredentials? {
        guard let data = read(account) else { return nil }
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        return try? decoder.decode(AppPhoneCredentials.self, from: data)
    }

    static func save(_ credentials: AppPhoneCredentials) {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        guard let data = try? encoder.encode(credentials) else { return }
        write(data, account: account)
    }

    static func clear() {
        SecItemDelete(query(account) as CFDictionary)
    }

    /// This phone's id, made on first use and kept across unpairs.
    static func phoneId() -> String {
        if let data = read(phoneIdAccount), let id = String(data: data, encoding: .utf8), !id.isEmpty {
            return id
        }
        let id = UUID().uuidString.lowercased()
        write(Data(id.utf8), account: phoneIdAccount)
        return id
    }

    private static func query(_ account: String) -> [CFString: Any] {
        var q: [CFString: Any] = [
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: service,
            kSecAttrAccount: account,
        ]
        if let accessGroup { q[kSecAttrAccessGroup] = accessGroup }
        return q
    }

    private static func read(_ account: String) -> Data? {
        var q = query(account)
        q[kSecReturnData] = true
        q[kSecMatchLimit] = kSecMatchLimitOne
        var result: CFTypeRef?
        guard SecItemCopyMatching(q as CFDictionary, &result) == errSecSuccess else { return nil }
        return result as? Data
    }

    private static func write(_ data: Data, account: String) {
        let q = query(account)
        let attributes: [CFString: Any] = [kSecValueData: data]
        if SecItemUpdate(q as CFDictionary, attributes as CFDictionary) == errSecItemNotFound {
            var item = q
            item[kSecValueData] = data
            // The extension runs while the phone may be locked (a broadcast
            // keeps going), so after-first-unlock, never synced off-device.
            item[kSecAttrAccessible] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
            SecItemAdd(item as CFDictionary, nil)
        }
    }
}

/// App Group state the app and the extension share: whether a broadcast is
/// live (for the "Your bot can see this screen" banner) and the Darwin
/// notifications that carry Stop from the app to the extension.
enum AppPhoneSharedState {
    static let appGroup = "group.com.allternit.mobile"
    static let extensionBundleId = "com.allternit.mobile.screen"
    /// Posted by the app's Stop button; the extension ends the broadcast.
    static let stopNotification = "com.allternit.mobile.screen.stop"
    /// Posted by the extension when it starts, finishes, or its viewers change.
    static let changedNotification = "com.allternit.mobile.screen.changed"

    private static let heartbeatKey = "screen.heartbeat"
    private static let watchedKey = "screen.watched"
    /// The extension refreshes its heartbeat this often while live…
    static let heartbeatInterval: TimeInterval = 5
    /// …so one older than this means it was killed without `broadcastFinished`.
    static let heartbeatStale: TimeInterval = 15

    private static var defaults: UserDefaults? { UserDefaults(suiteName: appGroup) }

    static func markLive(watched: Bool) {
        defaults?.set(Date().timeIntervalSince1970, forKey: heartbeatKey)
        defaults?.set(watched, forKey: watchedKey)
    }

    static func markEnded() {
        defaults?.removeObject(forKey: heartbeatKey)
        defaults?.removeObject(forKey: watchedKey)
    }

    static var isLive: Bool {
        guard let beat = defaults?.double(forKey: heartbeatKey), beat > 0 else { return false }
        return Date().timeIntervalSince1970 - beat < heartbeatStale
    }

    /// A viewer is attached (the node said resume and not pause since).
    static var isWatched: Bool { isLive && (defaults?.bool(forKey: watchedKey) ?? false) }

    static func post(_ name: String) {
        CFNotificationCenterPostNotification(CFNotificationCenterGetDarwinNotifyCenter(),
                                             CFNotificationName(name as CFString), nil, nil, true)
    }
}

/// Opens the app phone's sockets to its Desktop through the cloud relay:
/// mint a ticket with the app phone key (`POST /api/v1/app-phones/:id/
/// socket-ticket`), then dial the runtime socket tunnel with `?ticket=`.
/// The same path for the app and the extension, so there is one way in.
enum AppPhoneRelay {
    enum Endpoint: String {
        case pair, ingest, control

        func path(phoneId: String) -> String {
            let id = phoneId.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? phoneId
            return "/api/v1/phones/app/\(rawValue)?phone=\(id)"
        }
    }

    struct Failure: Error, LocalizedError, Sendable {
        let status: Int
        let message: String
        var errorDescription: String? { message }
        /// 401/404: the key was revoked or the phone removed; retrying won't
        /// help. (403 is a runtime capability refusal, not a dead pairing.)
        var isFatal: Bool { status == 401 || status == 404 }
    }

    private struct TicketBody: Encodable { let path: String }
    private struct TicketResponse: Decodable {
        let ticket: String
        /// Optional full socket URL, in case the cloud ever hands one back.
        let url: String?
    }

    /// The `wss://…/socket?ticket=…` URL for one socket. Takes the parts
    /// separately because pairing needs a socket before credentials exist.
    static func socketURL(_ endpoint: Endpoint, phoneId: String, runtimeDeviceId: String,
                          appPhoneKey: String, cloudBaseURL: URL,
                          session: URLSession = .shared) async throws -> URL {
        let path = endpoint.path(phoneId: phoneId)
        let escapedPhone = phoneId.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? phoneId
        var request = URLRequest(url: cloudBaseURL.appendingPathComponent("api/v1/app-phones/\(escapedPhone)/socket-ticket"))
        request.httpMethod = "POST"
        request.timeoutInterval = 20
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        request.setValue(appPhoneKey, forHTTPHeaderField: "x-allternit-app-phone-key")
        request.httpBody = try JSONEncoder().encode(TicketBody(path: path))

        let (data, response) = try await session.data(for: request)
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        guard (200...299).contains(status) else {
            throw Failure(status: status, message: Self.message(from: data) ?? "Ticket request failed (\(status))")
        }
        let ticket = try JSONDecoder().decode(TicketResponse.self, from: data)
        if let raw = ticket.url, let url = URL(string: raw) { return url }

        let escapedRuntime = runtimeDeviceId.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? runtimeDeviceId
        guard var components = URLComponents(
            url: cloudBaseURL.appendingPathComponent("api/v1/runtime-devices/\(escapedRuntime)/socket"),
            resolvingAgainstBaseURL: false
        ) else { throw Failure(status: 0, message: "Bad cloud URL") }
        components.scheme = components.scheme == "http" ? "ws" : "wss"
        components.queryItems = [URLQueryItem(name: "ticket", value: ticket.ticket)]
        guard let url = components.url else { throw Failure(status: 0, message: "Bad cloud URL") }
        return url
    }

    static func socketURL(_ endpoint: Endpoint, credentials c: AppPhoneCredentials,
                          session: URLSession = .shared) async throws -> URL {
        guard let base = URL(string: c.cloudBaseURL) else { throw Failure(status: 0, message: "Bad cloud URL") }
        return try await socketURL(endpoint, phoneId: c.phoneId, runtimeDeviceId: c.runtimeDeviceId,
                                   appPhoneKey: c.appPhoneKey, cloudBaseURL: base, session: session)
    }

    private static func message(from data: Data) -> String? {
        guard let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return nil }
        return (obj["message"] as? String) ?? (obj["error"] as? String)
    }
}
