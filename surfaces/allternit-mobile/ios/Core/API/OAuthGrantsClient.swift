import Foundation

/// One outside app a person approved to use their Allternit account
/// (CONTRACT-oauth-linked-billing-2026-11 "As built": `GET /api/v1/oauth/grants`).
struct OAuthGrant: Decodable, Identifiable, Equatable, Sendable {
    let id: String
    let appName: String
    let appLogoURL: String?
    let appHomepageURL: String?
    /// `testing | in_review | live | suspended`.
    let appState: String
    let scopes: [String]
    var monthlyCapCents: Int
    /// Null once the app is live: the person's own cap is then the only limit.
    let maxCapCents: Int?
    let spentCents: Int
    let createdAt: String?
    let lastUsedAt: String?

    private enum CodingKeys: String, CodingKey {
        case id, scopes
        case appName = "app_name", appLogoURL = "app_logo_url", appHomepageURL = "app_homepage_url", appState = "app_state"
        case monthlyCapCents = "monthly_cap_cents", maxCapCents = "max_cap_cents", spentCents = "spent_cents"
        case createdAt = "created_at", lastUsedAt = "last_used_at"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        appName = (try? c.decode(String.self, forKey: .appName)) ?? "An app"
        appLogoURL = try? c.decodeIfPresent(String.self, forKey: .appLogoURL)
        appHomepageURL = try? c.decodeIfPresent(String.self, forKey: .appHomepageURL)
        appState = (try? c.decode(String.self, forKey: .appState)) ?? "live"
        scopes = (try? c.decode([String].self, forKey: .scopes)) ?? []
        monthlyCapCents = (try? c.decode(Int.self, forKey: .monthlyCapCents)) ?? 0
        maxCapCents = try? c.decodeIfPresent(Int.self, forKey: .maxCapCents)
        spentCents = (try? c.decode(Int.self, forKey: .spentCents)) ?? 0
        createdAt = try? c.decodeIfPresent(String.self, forKey: .createdAt)
        lastUsedAt = try? c.decodeIfPresent(String.self, forKey: .lastUsedAt)
    }

    /// An app that is not live yet is a developer's test app.
    var isTesting: Bool { appState != "live" }
}

struct OAuthGrantsList: Decodable, Equatable, Sendable {
    let month: String?
    let resetsAt: String?
    let data: [OAuthGrant]

    private enum CodingKeys: String, CodingKey { case month, data, resetsAt = "resets_at" }
}

/// `GET /api/v1/claude-link` (billing contract, "As built" B steps 9-10).
struct ClaudeLinkStatus: Decodable, Equatable, Sendable {
    struct SpendLimit: Decodable, Equatable, Sendable { let reached: Bool }
    /// `unlinked | linked | relink_needed | disconnected`.
    let state: String
    let orgName: String?
    let linkedAt: String?
    let relinkBy: String?
    let spendLimit: SpendLimit?

    private enum CodingKeys: String, CodingKey {
        case state
        case orgName = "org_name", linkedAt = "linked_at", relinkBy = "relink_by", spendLimit = "spend_limit"
    }
}

/// A grant call the server refused, with its own words (a bad cap is a 400).
struct OAuthGrantsError: Error, LocalizedError, Equatable {
    let status: Int
    let message: String?
    var errorDescription: String? {
        if let message, !message.isEmpty { return message }
        switch status {
        case 401, 403: return "Sign in to Allternit on this phone, then try again."
        case 404: return "This app no longer has access."
        default: return "The server answered \(status). Try again."
        }
    }
}

/// Settings › Privacy › Connected apps: the account's OAuth grants on cloud-api
/// (Clerk session auth, the same token as the Connections calls).
struct OAuthGrantsClient {
    var baseURL: URL = AppConfig.cloudAPIBaseURL
    var session: URLSession = .shared
    var tokenProvider: @Sendable () async throws -> String? = { try await AuthManager.shared.getToken() }

    static let shared = OAuthGrantsClient()

    func list() async throws -> OAuthGrantsList {
        try JSONDecoder().decode(OAuthGrantsList.self, from: try await send("GET", "api/v1/oauth/grants"))
    }

    func setCap(id: String, cents: Int) async throws -> OAuthGrant {
        let body = try JSONSerialization.data(withJSONObject: ["monthly_cap_cents": cents])
        return try JSONDecoder().decode(OAuthGrant.self, from: try await send("PATCH", "api/v1/oauth/grants/\(Self.escape(id))", body: body))
    }

    /// DELETE answers 204. A 404 means the grant is already gone, which is the goal.
    func revoke(id: String) async throws {
        do { _ = try await send("DELETE", "api/v1/oauth/grants/\(Self.escape(id))") }
        catch let error as OAuthGrantsError where error.status == 404 { return }
    }

    private static func escape(_ id: String) -> String {
        id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? id
    }

    private func send(_ method: String, _ path: String, body: Data? = nil) async throws -> Data {
        guard let token = try await tokenProvider(), !token.isEmpty else { throw OAuthGrantsError(status: 401, message: nil) }
        var request = URLRequest(url: baseURL.appendingPathComponent(path))
        request.httpMethod = method
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        request.cachePolicy = .reloadIgnoringLocalCacheData
        request.timeoutInterval = 20
        if let body {
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = body
        }
        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse else { throw APIError.invalidResponse }
        guard (200...299).contains(http.statusCode) else {
            throw OAuthGrantsError(status: http.statusCode, message: APIErrorBody.parse(data).message)
        }
        return data
    }
}
