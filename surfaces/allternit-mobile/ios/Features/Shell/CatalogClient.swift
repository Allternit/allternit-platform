import SwiftUI

// Catalog v2 (platform #3094): `GET /api/v1/connections/catalog` with the
// category / interactive / sort filters, and `/catalog/:id` for one entry.
// Fields are snake_case. Lenient like `ConnCatalogEntry`: a newer server can
// add or drop fields without breaking older builds.

/// One Catalog entry: a connector (group `apps`) or a skill (group `skills`).
struct CatalogEntry: Decodable, Identifiable, Equatable, Sendable {
    var id: String
    var group: String
    var name: String
    var blurb: String
    var author: String?
    var authorURL: String?
    var serverURL: String?
    var categories: [String]
    var interactive: Bool
    var screenPack: String?
    var tier: String?
    var iconURL: String?
    var available: Bool
    var unavailableReason: String?
    var appId: String?
    var connectorId: String?

    private enum CodingKeys: String, CodingKey {
        case id, group, name, blurb, author, categories, interactive, tier, available
        case authorURL = "author_url", serverURL = "server_url", screenPack = "screen_pack"
        case iconURL = "icon_url", unavailableReason = "unavailable_reason"
        case appId = "app_id", connectorId = "connector_id"
    }

    init(id: String, name: String, blurb: String = "", group: String = "apps") {
        self.id = id
        self.group = group
        self.name = name
        self.blurb = blurb
        categories = []
        interactive = false
        available = true
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        group = ((try? c.decodeIfPresent(String.self, forKey: .group)) ?? nil) ?? ""
        name = ((try? c.decodeIfPresent(String.self, forKey: .name)) ?? nil) ?? id
        blurb = ((try? c.decodeIfPresent(String.self, forKey: .blurb)) ?? nil) ?? ""
        author = (try? c.decodeIfPresent(String.self, forKey: .author)) ?? nil
        authorURL = (try? c.decodeIfPresent(String.self, forKey: .authorURL)) ?? nil
        serverURL = (try? c.decodeIfPresent(String.self, forKey: .serverURL)) ?? nil
        categories = ((try? c.decodeIfPresent([String].self, forKey: .categories)) ?? nil) ?? []
        interactive = ((try? c.decodeIfPresent(Bool.self, forKey: .interactive)) ?? nil) ?? false
        screenPack = (try? c.decodeIfPresent(String.self, forKey: .screenPack)) ?? nil
        tier = (try? c.decodeIfPresent(String.self, forKey: .tier)) ?? nil
        iconURL = (try? c.decodeIfPresent(String.self, forKey: .iconURL)) ?? nil
        available = ((try? c.decodeIfPresent(Bool.self, forKey: .available)) ?? nil) ?? true
        unavailableReason = (try? c.decodeIfPresent(String.self, forKey: .unavailableReason)) ?? nil
        appId = (try? c.decodeIfPresent(String.self, forKey: .appId)) ?? nil
        connectorId = (try? c.decodeIfPresent(String.self, forKey: .connectorId)) ?? nil
    }

    var isSkill: Bool { group == "skills" }
    /// Verified = a partner or Anthropic listing.
    var isVerified: Bool { tier == "partner" || tier == "anthropic" }
    /// The id the existing connector connect flow knows this entry by.
    var connectId: String { appId ?? connectorId ?? id }
    var kindLabel: String { isSkill ? "Skill" : "Connector" }
    /// "Connector · By Figma"
    var byline: String {
        if let author, !author.isEmpty { return "\(kindLabel) · By \(author)" }
        return kindLabel
    }
}

struct CatalogPage: Decodable, Sendable {
    var entries: [CatalogEntry]
    var nextCursor: String?

    private enum CodingKeys: String, CodingKey { case entries, nextCursor = "next_cursor" }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        entries = ((try? c.decodeIfPresent([CatalogEntry].self, forKey: .entries)) ?? nil) ?? []
        nextCursor = (try? c.decodeIfPresent(String.self, forKey: .nextCursor)) ?? nil
    }
}

enum CatalogSort: String { case popular, trending, new }

enum CatalogClient {
    private static var base: URL { AppConfig.cloudAPIBaseURL.appendingPathComponent("api/v1/connections/catalog") }

    private static func get(_ url: URL) async throws -> Data {
        let client = APIClient.shared
        var request = try await client.authorizedRequest(url: url, method: "GET")
        request.cachePolicy = .reloadIgnoringLocalCacheData
        let (data, response): (Data, URLResponse)
        if SampleURLProtocol.handles(request.url) {
            (data, response) = try await client.send(request)
        } else {
            // Session auth only: straight to cloud-api, like the connections list.
            (data, response) = try await client.session.data(for: request)
        }
        try client.validate(response, data: data)
        return data
    }

    /// One page of the Catalog. `group` is `apps` (Connectors) or `skills`.
    static func page(group: String, q: String = "", category: String? = nil, interactive: Bool = false,
                     sort: CatalogSort? = nil, limit: Int = 24, cursor: String? = nil) async throws -> CatalogPage {
        var query = [URLQueryItem(name: "group", value: group), URLQueryItem(name: "limit", value: String(limit))]
        let trimmed = String(q.trimmingCharacters(in: .whitespacesAndNewlines).lowercased().prefix(64))
        if !trimmed.isEmpty { query.append(URLQueryItem(name: "q", value: trimmed)) }
        if let category, !category.isEmpty { query.append(URLQueryItem(name: "category", value: category)) }
        if interactive { query.append(URLQueryItem(name: "interactive", value: "true")) }
        if let sort { query.append(URLQueryItem(name: "sort", value: sort.rawValue)) }
        if let cursor { query.append(URLQueryItem(name: "cursor", value: cursor)) }
        var components = URLComponents(url: base, resolvingAgainstBaseURL: false)
        components?.queryItems = query
        guard let url = components?.url else { throw URLError(.badURL) }
        do { return try JSONDecoder().decode(CatalogPage.self, from: try await get(url)) } catch let e as DecodingError { throw APIError.decoding(e) }
    }

    /// `GET /catalog/:id` answers `{version, entry}`.
    static func detail(id: String) async throws -> CatalogEntry {
        struct Envelope: Decodable { let entry: CatalogEntry }
        let url = base.appendingPathComponent(id)
        do { return try JSONDecoder().decode(Envelope.self, from: try await get(url)).entry } catch let e as DecodingError { throw APIError.decoding(e) }
    }
}

/// Catalog logo: the entry's `icon_url` on a 1 pt bordered tile, a monogram
/// while it loads or when there is none.
struct CatalogLogo: View {
    let name: String
    let url: String?
    var size: CGFloat = 32

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: size * 0.22, style: .continuous).fill(Color("BgSecondary"))
            if let url, let u = URL(string: url) {
                AsyncImage(url: u) { phase in
                    if let image = phase.image {
                        image.resizable().scaledToFit().padding(size * 0.12)
                    } else {
                        monogram
                    }
                }
            } else {
                monogram
            }
        }
        .frame(width: size, height: size)
        .clipShape(RoundedRectangle(cornerRadius: size * 0.22, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: size * 0.22, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
        .accessibilityHidden(true)
    }

    private var monogram: some View {
        Text(String(name.trimmingCharacters(in: .whitespaces).prefix(1)).uppercased())
            .font(.system(size: size * 0.45, weight: .semibold))
            .foregroundColor(Color("TextSecondary"))
    }
}
