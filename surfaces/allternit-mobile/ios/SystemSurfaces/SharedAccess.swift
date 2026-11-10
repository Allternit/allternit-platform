import Foundation
import Security

/// The device read token, kept in the shared keychain group so extensions can
/// read it while the phone is locked (`AfterFirstUnlockThisDeviceOnly`).
/// No app-only imports.
enum SharedAccess {
    private static let service = "com.allternit.mobile.read-token"
    private static let account = "read-token"
    private static let groupSuffix = "com.allternit.mobile.shared"

    /// Full access-group name with the team prefix. Taken from the
    /// `AllternitSharedKeychainGroup` Info.plist key when the target has it,
    /// otherwise derived from the team prefix this process already carries.
    static var accessGroup: String? {
        if let v = Bundle.main.object(forInfoDictionaryKey: "AllternitSharedKeychainGroup") as? String,
           !v.isEmpty, !v.hasPrefix("$(") { return v }
        return derivedGroup
    }

    private static let derivedGroup: String? = {
        let probe: [CFString: Any] = [
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: "com.allternit.mobile.group-probe",
            kSecAttrAccount: "probe",
            kSecReturnAttributes: true,
        ]
        var result: CFTypeRef?
        var status = SecItemCopyMatching(probe as CFDictionary, &result)
        if status == errSecItemNotFound {
            var add = probe
            add[kSecValueData] = Data([0])
            add[kSecAttrAccessible] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
            status = SecItemAdd(add as CFDictionary, &result)
        }
        guard status == errSecSuccess,
              let attrs = result as? [String: Any],
              let group = attrs[kSecAttrAccessGroup as String] as? String,
              let dot = group.firstIndex(of: ".") else { return nil }
        return String(group[...dot]) + groupSuffix
    }()

    private static func query() -> [CFString: Any] {
        var q: [CFString: Any] = [
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: service,
            kSecAttrAccount: account,
        ]
        if let g = accessGroup { q[kSecAttrAccessGroup] = g }
        return q
    }

    /// Stores (or replaces) the read token.
    @discardableResult
    static func storeReadToken(_ token: String) -> Bool {
        let data = Data(token.utf8)
        let q = query()
        let status = SecItemUpdate(q as CFDictionary, [kSecValueData: data] as CFDictionary)
        if status == errSecSuccess { return true }
        guard status == errSecItemNotFound else { return false }
        var item = q
        item[kSecValueData] = data
        item[kSecAttrAccessible] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        return SecItemAdd(item as CFDictionary, nil) == errSecSuccess
    }

    static func readToken() -> String? {
        var q = query()
        q[kSecReturnData] = true
        q[kSecMatchLimit] = kSecMatchLimitOne
        var result: CFTypeRef?
        guard SecItemCopyMatching(q as CFDictionary, &result) == errSecSuccess,
              let data = result as? Data,
              let s = String(data: data, encoding: .utf8), !s.isEmpty else { return nil }
        return s
    }

    static func deleteReadToken() {
        SecItemDelete(query() as CFDictionary)
    }
}

/// `GET /api/v1/status/summary` with the read token. URLSession only.
struct StatusSummaryClient: Sendable {
    enum Failure: Error, Equatable {
        case signedOut
        case http(Int)
        case transport
    }

    var baseURL: URL
    var tokenProvider: @Sendable () -> String?
    var session: URLSession

    init(baseURL: URL = AppGroupStore.cloudBaseURL,
         tokenProvider: @escaping @Sendable () -> String? = { SharedAccess.readToken() },
         session: URLSession = .shared) {
        self.baseURL = baseURL
        self.tokenProvider = tokenProvider
        self.session = session
    }

    /// Fetches the newest snapshot (by `asOf` when the cloud returns several
    /// runtimes), stamps `asOf` with now when absent, and returns it.
    func fetch() async -> Result<StatusSnapshot, Failure> {
        guard let token = tokenProvider(), !token.isEmpty else { return .failure(.signedOut) }
        var req = URLRequest(url: baseURL.appendingPathComponent("api/v1/status/summary"))
        req.httpMethod = "GET"
        req.timeoutInterval = 10
        req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        req.setValue("application/json", forHTTPHeaderField: "Accept")
        let data: Data
        let response: URLResponse
        do { (data, response) = try await session.data(for: req) }
        catch { return .failure(.transport) }
        guard let http = response as? HTTPURLResponse else { return .failure(.transport) }
        guard (200..<300).contains(http.statusCode) else { return .failure(.http(http.statusCode)) }
        guard var snap = Self.decodeNewest(data) else { return .failure(.transport) }
        if snap.asOf == nil { snap.asOf = Date() }
        return .success(snap)
    }

    /// Accepts a single snapshot, an array, or `{snapshots|runtimes: [...]}`
    /// (the route returns the latest snapshot per runtime); picks the newest.
    static func decodeNewest(_ data: Data) -> StatusSnapshot? {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        if let one = try? decoder.decode(StatusSnapshot.self, from: data),
           let obj = try? JSONSerialization.jsonObject(with: data), obj is [String: Any],
           (obj as? [String: Any])?["snapshots"] == nil, (obj as? [String: Any])?["runtimes"] == nil {
            return one
        }
        var list: [StatusSnapshot] = []
        if let arr = try? decoder.decode([StatusSnapshot].self, from: data) { list = arr }
        else if let w = try? decoder.decode([String: [StatusSnapshot]].self, from: data) {
            list = w["snapshots"] ?? w["runtimes"] ?? []
        }
        return list.max { ($0.asOf ?? .distantPast) < ($1.asOf ?? .distantPast) }
    }
}
