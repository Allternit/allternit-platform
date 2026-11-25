import Foundation

/// Apps shelf + share list (Apps + MCP 2026, plan v3 lane 51). Routes (cloud-api):
///   GET  /api/v1/apps                       → { yours, servers }
///   POST /api/v1/apps/:kind/:ref/install    { granted_tier, pinned?, consent? }
///   POST /api/v1/apps/:kind/:ref/pin        { pinned }
///   DELETE /api/v1/apps/:kind/:ref
///   GET  /api/v1/share-links                → { links }
///   POST /api/v1/share-links/:id/pause      { paused }
///   DELETE /api/v1/share-links/:id
///
/// Feature flag `apps.shelf`, on by default. Turn off with the launch argument
/// `-allternit.flag.apps.shelf NO` or the `allternit.flag.apps.shelf` default.
enum AppsShelfFlag {
    static let key = "allternit.flag.apps.shelf"
    /// On by default (Eoj 2026-11-25: ships as a regular feature); a stored `NO` turns it off.
    static var isOn: Bool { (UserDefaults.standard.object(forKey: key) as? Bool) ?? true }
}

/// What an app may do. Raised tiers ask again; the server answers `needs_consent`.
enum AppsTier: String, Sendable, CaseIterable {
    case browser, app, full

    var label: String {
        switch self {
        case .browser: return "Browser"
        case .app: return "App"
        case .full: return "Full"
        }
    }

    var detail: String {
        switch self {
        case .browser: return "Runs in a sandbox. Can show pages and nothing else."
        case .app: return "Can save its own data and use the tools you list for it."
        case .full: return "Can use your connected apps and run tasks for you."
        }
    }

    /// Unknown tiers read as the strictest one.
    static func from(_ raw: String?) -> AppsTier { AppsTier(rawValue: (raw ?? "").lowercased()) ?? .browser }
}

struct AppsShelfRow: Decodable, Sendable, Equatable, Identifiable {
    let kind: String
    let provenance: String?
    let tier: String?
    let installed: Bool
    let pinned: Bool
    let link: String?
    let slug: String
    let icon: String?
    let name: String

    var id: String { "\(kind):\(slug)" }
    var appTier: AppsTier { AppsTier.from(tier) }

    private enum CodingKeys: String, CodingKey { case kind, provenance, tier, installed, pinned, link, slug, icon, name }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        kind = try c.decodeIfPresent(String.self, forKey: .kind) ?? "artifact"
        provenance = try c.decodeIfPresent(String.self, forKey: .provenance)
        tier = try c.decodeIfPresent(String.self, forKey: .tier)
        installed = try c.decodeIfPresent(Bool.self, forKey: .installed) ?? false
        pinned = try c.decodeIfPresent(Bool.self, forKey: .pinned) ?? false
        link = try c.decodeIfPresent(String.self, forKey: .link)
        slug = try c.decodeIfPresent(String.self, forKey: .slug) ?? ""
        icon = try c.decodeIfPresent(String.self, forKey: .icon)
        name = try c.decodeIfPresent(String.self, forKey: .name) ?? slug
    }

    init(kind: String, provenance: String?, tier: String?, installed: Bool, pinned: Bool,
         link: String?, slug: String, icon: String?, name: String) {
        self.kind = kind; self.provenance = provenance; self.tier = tier; self.installed = installed
        self.pinned = pinned; self.link = link; self.slug = slug; self.icon = icon; self.name = name
    }

    /// "you" and "org" rows are the person's own; "shared" came from someone's link.
    var provenanceLabel: String? {
        switch provenance {
        case "you": return "You"
        case "org": return "Your org"
        case "shared": return "Shared with you"
        default: return nil
        }
    }
}

struct AppsShelfResponse: Decodable, Sendable {
    let yours: [AppsShelfRow]
    let servers: [AppsShelfRow]

    private enum CodingKeys: String, CodingKey { case yours, servers }
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        yours = try c.decodeIfPresent([AppsShelfRow].self, forKey: .yours) ?? []
        servers = try c.decodeIfPresent([AppsShelfRow].self, forKey: .servers) ?? []
    }
}

struct AppsInstallResponse: Decodable, Sendable {
    let needsConsent: Bool
    let requestedTier: String?
    let addedCapabilities: [String]

    private enum CodingKeys: String, CodingKey {
        case needsConsent = "needs_consent", requestedTier = "requested_tier", addedCapabilities = "added_capabilities"
    }
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        needsConsent = try c.decodeIfPresent(Bool.self, forKey: .needsConsent) ?? false
        requestedTier = try c.decodeIfPresent(String.self, forKey: .requestedTier)
        addedCapabilities = try c.decodeIfPresent([String].self, forKey: .addedCapabilities) ?? []
    }
}

struct AppsShareLink: Decodable, Sendable, Equatable, Identifiable {
    let id: String
    let name: String?
    let subdomain: String?
    let url: String?
    let paused: Bool
    let revoked: Bool
    let killed: Bool

    private enum CodingKeys: String, CodingKey {
        case id, name, subdomain, url, paused, revoked, killed
        case revokedAt = "revoked_at", killedAt = "killed_at"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        name = try c.decodeIfPresent(String.self, forKey: .name)
        subdomain = try c.decodeIfPresent(String.self, forKey: .subdomain)
        url = try c.decodeIfPresent(String.self, forKey: .url)
        paused = try c.decodeIfPresent(Bool.self, forKey: .paused) ?? false
        let revokedAt = try c.decodeIfPresent(String.self, forKey: .revokedAt)
        let killedAt = try c.decodeIfPresent(String.self, forKey: .killedAt)
        revoked = (try c.decodeIfPresent(Bool.self, forKey: .revoked) ?? false) || revokedAt != nil
        killed = (try c.decodeIfPresent(Bool.self, forKey: .killed) ?? false) || killedAt != nil
    }

    init(id: String, name: String?, subdomain: String?, url: String?, paused: Bool, revoked: Bool, killed: Bool) {
        self.id = id; self.name = name; self.subdomain = subdomain; self.url = url
        self.paused = paused; self.revoked = revoked; self.killed = killed
    }

    /// Live links run on the person's own computer (`<name>.live.<domain>`).
    var isLive: Bool { (url ?? "").contains(".live.") }
    var isActive: Bool { !revoked && !killed }
    var title: String { name ?? subdomain ?? url ?? "Link" }
}

struct AppsShareLinksResponse: Decodable, Sendable {
    let links: [AppsShareLink]
}

// MARK: - Links

enum AppsShelfLinks {
    static let appDomain = "allternitusercontent.com"

    /// The page an app opens: its hosted https link, or `/app/<slug>` on the web origin.
    /// Anything that is not https on our own domains is refused.
    @MainActor
    static func openURL(for link: String?) -> URL? {
        guard let link, !link.isEmpty else { return nil }
        var url: URL?
        if link.hasPrefix("/") {
            url = URL(string: link, relativeTo: ArtifactLiveWebView.webOrigin)?.absoluteURL
        } else {
            url = URL(string: link)
        }
        guard let url, url.scheme?.lowercased() == "https", let host = url.host?.lowercased() else { return nil }
        let ok = host == appDomain || host.hasSuffix("." + appDomain)
            || host == ArtifactLiveWebView.webOrigin.host?.lowercased()
        return ok ? url : nil
    }
}

// MARK: - Client

final class AppsShelfClient: @unchecked Sendable {
    static let shared = AppsShelfClient()
    private let client: APIClient
    init(client: APIClient = .shared) { self.client = client }

    private struct InstallBody: Encodable {
        let granted_tier: String
        let pinned: Bool?
        let consent: Bool?
    }
    private struct PinBody: Encodable { let pinned: Bool }
    private struct PauseBody: Encodable { let paused: Bool }

    func shelf() async throws -> AppsShelfResponse { try await client.get(path: "apps") }

    func install(_ row: AppsShelfRow, tier: AppsTier, consent: Bool) async throws -> AppsInstallResponse {
        try await client.post(
            path: "apps/\(Self.escape(row.kind))/\(Self.escape(row.slug))/install",
            body: InstallBody(granted_tier: tier.rawValue, pinned: nil, consent: consent ? true : nil)
        )
    }

    /// The URL to load on Open: the minted `open_url` for own/org/shared apps, else the plain link.
    /// Vendor apps have no handoff.
    func openLink(for row: AppsShelfRow) async -> String? {
        if row.kind.lowercased().contains("vendor") { return row.link }
        if let minted = await handoffURL(slug: row.slug), AppsShelfLinks.openURL(for: minted) != nil { return minted }
        return row.link
    }

    func uninstall(_ row: AppsShelfRow) async throws {
        try await client.delete(path: "apps/\(Self.escape(row.kind))/\(Self.escape(row.slug))")
    }

    func pin(_ row: AppsShelfRow, pinned: Bool) async throws {
        try await client.post(
            path: "apps/\(Self.escape(row.kind))/\(Self.escape(row.slug))/pin",
            body: PinBody(pinned: pinned)
        )
    }

    private struct EmptyBody: Encodable {}
    private struct HandoffResponse: Decodable { let open_url: String? }

    /// One-time sign-in handoff for a hosted app. Nil on any failure (caller falls back to the plain link).
    func handoffURL(slug: String) async -> String? {
        guard !slug.isEmpty else { return nil }
        let response: HandoffResponse? = try? await client.post(path: "apps/\(Self.escape(slug))/handoff", body: EmptyBody())
        return response?.open_url
    }

    func links() async throws -> [AppsShareLink] {
        let response: AppsShareLinksResponse = try await client.get(path: "share-links")
        return response.links
    }

    func pause(_ link: AppsShareLink, paused: Bool) async throws {
        try await client.post(path: "share-links/\(Self.escape(link.id))/pause", body: PauseBody(paused: paused))
    }

    func revoke(_ link: AppsShareLink) async throws {
        try await client.delete(path: "share-links/\(Self.escape(link.id))")
    }

    static func escape(_ id: String) -> String {
        var allowed = CharacterSet.alphanumerics
        allowed.insert(charactersIn: "-._~")
        return id.addingPercentEncoding(withAllowedCharacters: allowed) ?? id
    }
}

// MARK: - Model

@MainActor
final class AppsShelfModel: ObservableObject {
    @Published private(set) var yours: [AppsShelfRow] = []
    @Published private(set) var servers: [AppsShelfRow] = []
    @Published private(set) var links: [AppsShareLink] = []
    @Published private(set) var loaded = false
    @Published private(set) var failure: String?
    @Published private(set) var linksFailure: String?
    @Published var notice: String?
    @Published private(set) var busy: Set<String> = []
    /// An install waiting on the person's yes (first install, or a raised tier).
    @Published var consent: ConsentRequest?

    struct ConsentRequest: Identifiable, Equatable {
        let row: AppsShelfRow
        let tier: AppsTier
        let added: [String]
        let raised: Bool
        var id: String { row.id }
    }

    private let api: AppsShelfClient
    init(api: AppsShelfClient = .shared) { self.api = api }

    /// The URL Open loads (a one-time handoff for hosted apps, else the plain link).
    func openLink(for row: AppsShelfRow) async -> String? { await api.openLink(for: row) }

    func refresh() async {
        do {
            let shelf = try await api.shelf()
            yours = shelf.yours
            servers = shelf.servers
            failure = nil
        } catch {
            failure = Self.message(error)
        }
        do {
            links = try await api.links().filter(\.isActive)
            linksFailure = nil
        } catch {
            linksFailure = Self.message(error)
        }
        loaded = true
    }

    /// Install button: a new install always shows the sheet first.
    func requestInstall(_ row: AppsShelfRow) {
        consent = ConsentRequest(row: row, tier: row.appTier, added: [], raised: false)
    }

    /// The person said yes on the sheet.
    func confirmInstall(_ request: ConsentRequest) async {
        consent = nil
        await install(request.row, tier: request.tier, consent: true)
    }

    private func install(_ row: AppsShelfRow, tier: AppsTier, consent: Bool) async {
        busy.insert(row.id)
        defer { busy.remove(row.id) }
        do {
            let result = try await api.install(row, tier: tier, consent: consent)
            if result.needsConsent {
                // The tier went up since the person last said yes: ask again.
                self.consent = ConsentRequest(row: row, tier: AppsTier.from(result.requestedTier ?? tier.rawValue),
                                              added: result.addedCapabilities, raised: true)
                return
            }
            Haptics.success()
            await refresh()
        } catch {
            notice = "Couldn’t install \(row.name). \(Self.message(error))"
        }
    }

    func uninstall(_ row: AppsShelfRow) async {
        busy.insert(row.id)
        defer { busy.remove(row.id) }
        do {
            try await api.uninstall(row)
            await refresh()
        } catch {
            notice = "Couldn’t remove \(row.name). \(Self.message(error))"
        }
    }

    func togglePin(_ row: AppsShelfRow) async {
        busy.insert(row.id)
        defer { busy.remove(row.id) }
        do {
            try await api.pin(row, pinned: !row.pinned)
            await refresh()
        } catch {
            notice = "Couldn’t change the pin on \(row.name). \(Self.message(error))"
        }
    }

    func togglePause(_ link: AppsShareLink) async {
        busy.insert(link.id)
        defer { busy.remove(link.id) }
        do {
            try await api.pause(link, paused: !link.paused)
            await refresh()
        } catch {
            notice = "Couldn’t \(link.paused ? "resume" : "pause") the link. \(Self.message(error))"
        }
    }

    func revoke(_ link: AppsShareLink) async {
        busy.insert(link.id)
        defer { busy.remove(link.id) }
        do {
            try await api.revoke(link)
            Haptics.medium()
            await refresh()
        } catch {
            notice = "Couldn’t stop the link. \(Self.message(error))"
        }
    }

    private static func message(_ error: Error) -> String {
        let text = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        return text.isEmpty ? "Try again in a moment." : text
    }
}
