import Foundation

/// One provider/engine row from `GET /api/v1/providers` (provider_routes.rs).
struct ProviderInfo: Decodable, Sendable, Identifiable {
    let id: String
    let name: String
    let providerType: String
    let baseUrl: String?
    let apiKeySet: Bool
    let models: [String]
    let status: String

    private enum CodingKeys: String, CodingKey {
        case id, name, models, status
        case providerType = "provider_type"
        case baseUrl = "base_url"
        case apiKeySet = "api_key_set"
    }

    /// `GET /api/v1/providers` sends `models` as `[{id, name}]` objects
    /// (provider_routes.rs `ModelInfo`); older servers sent plain strings.
    /// Accept both and keep the model ids, so one shape change never fails
    /// the whole provider list.
    private struct ModelEntry: Decodable {
        let id: String
        init(from decoder: Decoder) throws {
            if let plain = try? decoder.singleValueContainer().decode(String.self) {
                id = plain
                return
            }
            let keyed = try decoder.container(keyedBy: Keys.self)
            id = try keyed.decode(String.self, forKey: .id)
        }
        private enum Keys: String, CodingKey { case id }
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        name = try c.decode(String.self, forKey: .name)
        providerType = try c.decode(String.self, forKey: .providerType)
        baseUrl = try c.decodeIfPresent(String.self, forKey: .baseUrl)
        apiKeySet = try c.decodeIfPresent(Bool.self, forKey: .apiKeySet) ?? false
        models = (try c.decodeIfPresent([ModelEntry].self, forKey: .models) ?? []).map(\.id)
        status = try c.decode(String.self, forKey: .status)
    }

    var isLocal: Bool { providerType == "local" }
    var isSubprocess: Bool { providerType == "subprocess" }

    var kindLabel: String {
        if isLocal { return "Local" }
        if isSubprocess { return "CLI" }
        return "Cloud"
    }

    var isReady: Bool {
        status == "active" || status == "ok" || status == "online"
    }
}

/// Auth status row from `GET /api/v1/providers/auth/status`.
struct ProviderAuthStatus: Decodable, Sendable, Identifiable {
    var id: String { providerId }
    let providerId: String
    let status: String
    let authenticated: Bool

    private enum CodingKeys: String, CodingKey {
        case providerId = "provider_id"
        case status
        case authenticated
    }
}

/// Ollama live probe response from `GET /api/v1/provider/ollama/status`.
struct OllamaLiveStatus: Decodable, Sendable {
    let running: Bool
    let models: [String]
}
