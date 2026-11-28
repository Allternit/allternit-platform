import Foundation

// The server picker catalog (contract docs/contracts/picker-catalog):
// `GET {cloud}/api/v1/picker-catalog` says, per Allternit Cloud model, whether
// this account can use it (access.state ok | locked | hidden) and what to do
// about a lock (access.action). Only `cloudModels` is read here; the other
// sections (runtimes, brains, onDevice) are the registry's and the setup
// screens' business. An older server answers 404: callers then keep today's
// `{cloud}/v1/models` + plan-entitlement lock path.

struct PickerAccess: Codable, Equatable, Sendable {
    enum State: String, Codable, Sendable { case ok, locked, hidden }
    enum Action: String, Codable, Sendable { case signIn = "sign_in", addCredits = "add_credits", acceptTerms = "accept_terms", upgrade, retry }

    var state: State
    var reason: String?
    var action: Action?

    init(state: State, reason: String? = nil, action: Action? = nil) {
        self.state = state; self.reason = reason; self.action = action
    }

    // Unknown states/actions from a newer server degrade to ok / no action.
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Keys.self)
        state = (try? c.decode(String.self, forKey: .state)).flatMap(State.init(rawValue:)) ?? .ok
        reason = try? c.decode(String.self, forKey: .reason)
        action = (try? c.decode(String.self, forKey: .action)).flatMap(Action.init(rawValue:))
    }
    private enum Keys: String, CodingKey { case state, reason, action }
}

struct PickerCloudModel: Codable, Equatable, Sendable {
    let id: String
    let name: String
    let provider: String?
    let access: PickerAccess
}

struct PickerCatalogBody: Codable, Equatable, Sendable {
    var cloudModels: [PickerCloudModel]

    init(cloudModels: [PickerCloudModel]) { self.cloudModels = cloudModels }
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        cloudModels = (try? c.decode([PickerCloudModel].self, forKey: .cloudModels)) ?? []
    }
    private enum CodingKeys: String, CodingKey { case cloudModels }

    /// Access by model id (the contract ids already carry the `allternit/` prefix).
    var accessById: [String: PickerAccess] {
        Dictionary(cloudModels.map { ($0.id, $0.access) }, uniquingKeysWith: { _, last in last })
    }
}

enum PickerCatalogResult: Equatable, Sendable {
    case fresh(PickerCatalogBody, etag: String?)
    case notModified
    /// 404 or an unreadable body: an old server. Use the `/v1/models` path.
    case unsupported
    case failed
}

enum PickerCatalogClient {
    /// One revalidating fetch. The route is per account, so it carries the Bearer
    /// when signed in and goes anonymous (signed_out lock rows) when not.
    static func fetch(etag: String?, base: URL = AppConfig.cloudAPIBaseURL,
                      session: URLSession? = nil) async -> PickerCatalogResult {
        let url = base.appendingPathComponent("api/v1/picker-catalog")
        var request: URLRequest
        if let signed = try? await APIClient.shared.authorizedRequest(url: url) {
            request = signed
        } else {
            request = URLRequest(url: url)
        }
        request.cachePolicy = .reloadIgnoringLocalCacheData
        request.timeoutInterval = 15
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        if let etag { request.setValue(etag, forHTTPHeaderField: "If-None-Match") }
        do {
            let (data, response) = try await (session ?? URLSession.shared).data(for: request)
            guard let http = response as? HTTPURLResponse else { return .failed }
            return interpret(status: http.statusCode, data: data, etag: http.value(forHTTPHeaderField: "ETag"))
        } catch {
            return .failed
        }
    }

    /// Pure: what a response means. Tested against the contract fixtures.
    static func interpret(status: Int, data: Data, etag: String?) -> PickerCatalogResult {
        if status == 304 { return .notModified }
        if status == 404 || status == 405 || status == 501 { return .unsupported }
        guard (200...299).contains(status) else { return .failed }
        guard let body = try? JSONDecoder().decode(PickerCatalogBody.self, from: data), !body.cloudModels.isEmpty else {
            return .unsupported
        }
        return .fresh(body, etag: etag)
    }

    // Disk cache (Application Support/Allternit/picker-catalog-v1.json): last good
    // body + ETag, so the lock state shows at launch before the fetch returns.
    private struct CacheFile: Codable { var etag: String?; var body: PickerCatalogBody }

    private static var cacheURL: URL? {
        guard let support = try? FileManager.default.url(for: .applicationSupportDirectory, in: .userDomainMask,
                                                         appropriateFor: nil, create: true) else { return nil }
        let dir = support.appendingPathComponent("Allternit", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir.appendingPathComponent("picker-catalog-v1.json")
    }

    static func readCache() -> (body: PickerCatalogBody, etag: String?)? {
        guard let url = cacheURL, let data = try? Data(contentsOf: url),
              let file = try? JSONDecoder().decode(CacheFile.self, from: data) else { return nil }
        return (file.body, file.etag)
    }

    static func writeCache(body: PickerCatalogBody, etag: String?) {
        guard let url = cacheURL, let data = try? JSONEncoder().encode(CacheFile(etag: etag, body: body)) else { return }
        try? data.write(to: url, options: .atomic)
    }

    static func clearCache() {
        if let url = cacheURL { try? FileManager.default.removeItem(at: url) }
    }
}
