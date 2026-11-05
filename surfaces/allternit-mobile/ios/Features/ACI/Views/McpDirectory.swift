import SwiftUI

// The connector directory, one for MCP Apps › Discover and Customize ›
// Connectors, as on Desktop (views/customize/ConnectorsDirectory.tsx,
// lib/connectors/connector-directory.ts).

// MARK: - Entries

/// DirectoryConnector: one app in the directory.
struct DirectoryEntry: Identifiable, Sendable, Equatable {
    enum Shelf: Sendable { case connector, official }

    let id: String
    let name: String
    var description: String
    var category: String
    var baseURL: String? = nil
    var authType: String = "oauth2"
    var connectable: Bool = true
    var connected: Bool = false
    var account: String? = nil
    var toolNames: [String] = []
    var shelf: Shelf = .connector
    /// The MCP server address (official servers).
    var serverURL: String? = nil
    /// The vendor publishes this server itself: the blue verified mark.
    var official: Bool = false
    var docsURL: String? = nil
    /// Why this can't connect from here, and what to do instead.
    var setupMessage: String? = nil

    /// The site its logo and maker come from.
    var site: String? { baseURL ?? serverURL }

    init(connector: Connector) {
        id = connector.id
        name = connector.name
        description = connector.description ?? ""
        category = connector.category ?? "Other"
        baseURL = connector.baseURL
        authType = connector.authType ?? "oauth2"
        connectable = connector.connectable
        connected = connector.isConnected
        account = connector.connection.account
        toolNames = connector.toolNames
    }

    init(server: McpOfficialServers.Server) {
        id = "mcp:\(server.id)"
        name = server.name
        description = "\(server.name)'s own MCP server. Sign in with your \(server.name) account."
        category = server.category
        shelf = .official
        serverURL = server.url
        official = true
    }

    init(guide: McpOfficialServers.Guide) {
        id = "mcp:\(guide.id)"
        name = guide.name
        description = guide.description
        category = "Data"
        connectable = false
        shelf = .official
        official = true
        docsURL = guide.docsURL
        setupMessage = guide.setup
    }
}

enum AppDirectory {
    /// TOP_CONNECTOR_IDS.
    static let topIds = [
        "google_drive", "gmail", "google_calendar", "notion", "slack", "github", "figma", "linear",
        "hubspot", "asana", "jira", "confluence", "canva", "dropbox", "airtable", "stripe",
        "salesforce", "zoom", "microsoft_teams", "outlook", "onedrive", "sentry", "vercel", "shopify",
    ]

    struct Collection: Identifiable, Sendable {
        let id: String
        let title: String
        let blurb: String
        /// COLLECTION_ACCENT.
        let accent: String
        let ids: [String]
    }

    /// CONNECTOR_COLLECTIONS.
    static let collections = [
        Collection(id: "developers", title: "Allternit for developers",
                   blurb: "Ship from chat: repos, issues, deploys, and errors in one place.", accent: "#2F9E6F",
                   ids: ["github", "gitlab", "linear", "sentry", "vercel", "jira", "bitbucket", "supabase"]),
        Collection(id: "design", title: "Allternit for design",
                   blurb: "Sketch, diagram, and iterate with your creative tools.", accent: "#6366F1",
                   ids: ["figma", "canva", "miro", "adobe", "dribbble", "webflow", "framer", "notion"]),
        Collection(id: "operations", title: "Allternit for operations",
                   blurb: "Money, customers, and pipeline, read straight from the source.", accent: "#2B7BB9",
                   ids: ["stripe", "hubspot", "salesforce", "quickbooks", "xero", "shopify", "intercom", "zendesk"]),
    ]

    static func rankKey(_ id: String) -> String {
        id.replacingOccurrences(of: "^mcp:", with: "", options: .regularExpression).replacingOccurrences(of: "-", with: "_")
    }

    /// rankConnectors: the well-known list first, then everything else by name.
    static func ranked(_ list: [DirectoryEntry]) -> [DirectoryEntry] {
        let rank = Dictionary(uniqueKeysWithValues: topIds.enumerated().map { ($1, $0) })
        return list.sorted { a, b in
            let ra = rank[rankKey(a.id)] ?? Int.max
            let rb = rank[rankKey(b.id)] ?? Int.max
            return ra != rb ? ra < rb : a.name.localizedCompare(b.name) == .orderedAscending
        }
    }

    /// categoryCounts: biggest first, then by name.
    static func categoryCounts(_ list: [DirectoryEntry]) -> [(name: String, count: Int)] {
        var counts: [String: Int] = [:]
        for entry in list { counts[entry.category, default: 0] += 1 }
        return counts.map { (name: $0.key, count: $0.value) }
            .sorted { $0.count != $1.count ? $0.count > $1.count : $0.name < $1.name }
    }

    private static func squash(_ name: String) -> String {
        name.lowercased().replacingOccurrences(of: "[^a-z0-9]", with: "", options: .regularExpression)
    }

    /// McpAppsDirectory `all`: a vendor's own server and the catalog connector
    /// for the same app are one app, the official one, with the catalog's
    /// description, tools and category.
    static func merged(catalog: [DirectoryEntry], installedIds: Set<String>) -> [DirectoryEntry] {
        let byName = Dictionary(catalog.map { (squash($0.name), $0) }, uniquingKeysWith: { first, _ in first })
        var absorbed = Set<String>()
        let official: [DirectoryEntry] = McpOfficialServers.servers.map { server in
            var entry = DirectoryEntry(server: server)
            if let twin = byName[squash(entry.name)] {
                absorbed.insert(twin.id)
                if !twin.description.isEmpty { entry.description = twin.description }
                entry.category = twin.category
                entry.toolNames = twin.toolNames
                entry.connected = twin.connected
                entry.account = twin.account
            }
            return entry
        }
        let guides = McpOfficialServers.guides.map(DirectoryEntry.init(guide:)).filter { byName[squash($0.name)] == nil }
        return (official + guides + catalog.filter { !absorbed.contains($0.id) }).map { entry in
            guard entry.shelf == .official, !entry.connected else { return entry }
            let bare = entry.id.replacingOccurrences(of: "^mcp:", with: "", options: .regularExpression)
            var next = entry
            next.connected = installedIds.contains(entry.id) || installedIds.contains(bare)
            return next
        }
    }

    /// makerOf: who made it, and its site.
    static func maker(of entry: DirectoryEntry) -> (name: String, site: String?)? {
        let origin = entry.site.flatMap { URL(string: $0) }.flatMap { url -> String? in
            guard let scheme = url.scheme, let host = url.host else { return nil }
            return "\(scheme)://\(host)"
        }
        let host = AppLogoCatalog.siteHost(entry.site)
        if host == "googleapis" { return ("Google", "https://www.google.com") }
        if host == "githubcopilot" { return ("GitHub", "https://github.com") }
        if entry.official { return (entry.name, origin) }
        guard let label = host else { return nil }
        let compact = entry.name.replacingOccurrences(of: "[^A-Za-z0-9]", with: "", options: .regularExpression)
        if compact.lowercased().hasPrefix(label) { return (String(compact.prefix(label.count)), origin) }
        return (label.prefix(1).uppercased() + label.dropFirst(), origin)
    }

    static func footer(of entry: DirectoryEntry) -> String? {
        guard let maker = maker(of: entry) else { return nil }
        if let site = maker.site {
            return site.replacingOccurrences(of: "^https?://", with: "", options: .regularExpression)
        }
        return "by \(maker.name)"
    }
}

// MARK: - Connecting

/// useConnectorConnect: connect a catalog connector or add an official server,
/// and hand the rest to the user (a sign-in page, an API key).
@MainActor
final class DirectoryConnectModel: ObservableObject {
    struct SignIn: Identifiable {
        let url: URL
        var id: String { url.absoluteString }
    }

    @Published private(set) var busy: Set<String> = []
    @Published var signIn: SignIn? = nil
    @Published var apiKeyFor: DirectoryEntry? = nil
    @Published var note: String? = nil

    /// Official servers: create the connector, record the install, start OAuth.
    var connectServer: (@MainActor (DirectoryEntry) async throws -> URL?)? = nil
    /// The list changed (after connecting, disconnecting or closing sign-in).
    var onChanged: (@MainActor () async -> Void)? = nil

    private let client = ConnectorsClient()

    func connect(_ entry: DirectoryEntry, apiKey: String? = nil) async {
        guard !busy.contains(entry.id) else { return }
        note = nil
        busy.insert(entry.id)
        defer { busy.remove(entry.id) }
        do {
            if entry.shelf == .official, let connectServer {
                if let url = try await connectServer(entry) {
                    signIn = SignIn(url: url)
                } else {
                    note = "Added \(entry.name)."
                }
                await onChanged?()
                return
            }
            let response = try await client.connect(id: entry.id, apiKey: apiKey)
            if let raw = response.authorizeURL, let url = URL(string: raw) {
                signIn = SignIn(url: url)
            } else if response.error == "api_key_required" {
                apiKeyFor = entry
            } else {
                note = response.message ?? response.error
                await onChanged?()
            }
        } catch {
            note = "Couldn’t connect \(entry.name). \(SettingsLoadFailure.from(error).message)"
        }
    }

    func disconnect(_ entry: DirectoryEntry) async {
        note = nil
        busy.insert(entry.id)
        defer { busy.remove(entry.id) }
        do {
            try await client.disconnect(id: entry.id)
            await onChanged?()
        } catch {
            note = "Couldn’t disconnect \(entry.name). \(SettingsLoadFailure.from(error).message)"
        }
    }
}

/// Everything a directory screen presents: the app's page, then its sign-in
/// page or API key prompt once that page has closed.
@MainActor
struct DirectoryFlow: ViewModifier {
    @Binding var selected: DirectoryEntry?
    @ObservedObject var model: DirectoryConnectModel
    /// MCP Apps offers "Add by URL" for guides.
    var onAddByURL: (() -> Void)? = nil
    /// The installed MCP connector behind a connected app (MCP Apps), for its vendor's own views.
    var installFor: ((DirectoryEntry) -> String?)? = nil

    @State private var pendingConnect: DirectoryEntry? = nil
    @State private var pendingDisconnect: DirectoryEntry? = nil
    @State private var pendingAddByURL = false
    @State private var apiKey = ""

    func body(content: Content) -> some View {
        content
            .sheet(item: $selected, onDismiss: runPending) { entry in
                DirectoryDetailSheet(
                    entry: entry,
                    onConnect: { pendingConnect = entry; selected = nil },
                    onDisconnect: { pendingDisconnect = entry; selected = nil },
                    onAddByURL: onAddByURL == nil ? nil : { pendingAddByURL = true; selected = nil },
                    install: installFor?(entry)
                )
            }
            .sheet(item: $model.signIn, onDismiss: { Task { await model.onChanged?() } }) { target in
                SafariView(url: target.url)
            }
            .alert("API key", isPresented: Binding(get: { model.apiKeyFor != nil }, set: { if !$0 { model.apiKeyFor = nil } }),
                   presenting: model.apiKeyFor) { entry in
                SecureField("API key", text: $apiKey)
                Button("Connect") {
                    let key = apiKey
                    apiKey = ""
                    Task { await model.connect(entry, apiKey: key) }
                }
                Button("Cancel", role: .cancel) { apiKey = "" }
            } message: { entry in
                Text("Enter an API key for \(entry.name).")
            }
    }

    private func runPending() {
        if let entry = pendingConnect {
            pendingConnect = nil
            Task { await model.connect(entry) }
        } else if let entry = pendingDisconnect {
            pendingDisconnect = nil
            Task { await model.disconnect(entry) }
        } else if pendingAddByURL {
            pendingAddByURL = false
            onAddByURL?()
        }
    }
}

// MARK: - Cards

/// ConnectorCard: logo tile, status dot, name with its mark, description,
/// the site in the footer, a round + (or ✓) in the tile's corner.
struct DirectoryCard: View {
    let entry: DirectoryEntry
    var busy: Bool = false
    let onOpen: () -> Void
    let onAdd: () -> Void

    var body: some View {
        PolishCard(
            name: entry.name,
            description: entry.description,
            status: entry.connected ? .ok : .idle,
            statusLabel: entry.connected ? "Connected" : "Not added",
            footer: AppDirectory.footer(of: entry),
            footerIcon: "link-simple",
            onOpen: onOpen,
            logo: { AppLogoView(id: entry.id, name: entry.name, url: entry.site, size: 52) },
            action: {
                PolishCardAddButton(added: entry.connected, pending: busy,
                                    label: entry.connected ? "\(entry.name) connected" : "Connect \(entry.name)",
                                    action: onAdd)
            },
            extras: { DirectoryNameMark(entry: entry, size: 13) },
            controls: { EmptyView() }
        )
    }
}

/// NameMark: the blue seal for a vendor's own server, the quiet seal for the catalog's.
struct DirectoryNameMark: View {
    let entry: DirectoryEntry
    let size: CGFloat

    var body: some View {
        if entry.official {
            VerifiedMark(size: size + 1)
        } else {
            PhIcon("seal-check", size: size).foregroundColor(Color("TextTertiary"))
        }
    }
}

struct DirectoryGrid: View {
    let list: [DirectoryEntry]
    let busy: Set<String>
    let onOpen: (DirectoryEntry) -> Void
    let onAdd: (DirectoryEntry) -> Void

    var body: some View {
        PolishCardGrid {
            ForEach(list) { entry in
                DirectoryCard(entry: entry, busy: busy.contains(entry.id), onOpen: { onOpen(entry) }, onAdd: { onAdd(entry) })
            }
        }
    }
}

/// SectionHead: title, count, "Show all →".
struct DirectorySectionHead: View {
    let title: String
    var count: Int? = nil
    var onShowAll: (() -> Void)? = nil

    var body: some View {
        HStack(alignment: .firstTextBaseline) {
            Text(title)
                .font(.system(size: 14, weight: .medium))
                .foregroundColor(Color("TextPrimary"))
            if let count {
                Text(count.formatted())
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextTertiary"))
            }
            Spacer()
            if let onShowAll {
                Button(action: onShowAll) {
                    HStack(spacing: 4) {
                        Text("Show all")
                        PhIcon("arrow-right", size: 13)
                    }
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextPrimary"))
                    .frame(minHeight: 32)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
        }
    }
}

// MARK: - Landing

/// ConnectorsLanding: curated hero, top apps, official servers, then the six
/// biggest categories.
struct DirectoryLanding: View {
    /// Not connected yet.
    let list: [DirectoryEntry]
    /// The hero draws from these, so it doesn't shrink as you connect.
    let pool: [DirectoryEntry]
    let total: Int
    /// "apps" or "connectors".
    let noun: String
    let busy: Set<String>
    let onOpen: (DirectoryEntry) -> Void
    let onAdd: (DirectoryEntry) -> Void
    let onCategory: (String) -> Void
    let onShowAll: () -> Void
    let onCollection: (String) -> Void
    var onShowOfficial: (() -> Void)? = nil
    var showCategoryCloud = true

    private struct Section: Identifiable {
        let name: String
        let count: Int
        let members: [DirectoryEntry]
        var id: String { name }
    }

    private struct Plan {
        var top: [DirectoryEntry] = []
        var official: [DirectoryEntry] = []
        var officialCount = 0
        var sections: [Section] = []
        var categories: [(name: String, count: Int)] = []
    }

    private func makePlan() -> Plan {
        let ranked = AppDirectory.ranked(list)
        var out = Plan()
        out.top = Array(ranked.prefix(10))
        var shown = Set(out.top.map(\.id))
        if onShowOfficial != nil {
            let official = ranked.filter(\.official)
            out.officialCount = official.count
            out.official = Array(official.filter { !shown.contains($0.id) }.prefix(8))
            out.official.forEach { shown.insert($0.id) }
        }
        out.categories = AppDirectory.categoryCounts(list)
        out.sections = out.categories.prefix(6).compactMap { category in
            let members = Array(ranked.filter { $0.category == category.name && !shown.contains($0.id) }.prefix(4))
            return members.isEmpty ? nil : Section(name: category.name, count: category.count, members: members)
        }
        return out
    }

    var body: some View {
        let plan = makePlan()
        VStack(alignment: .leading, spacing: 0) {
            DirectoryHero(pool: pool, onOpen: onOpen, onCollection: onCollection)
                .padding(.bottom, 32)
            DirectorySectionHead(title: "Top \(noun)", count: total, onShowAll: onShowAll)
                .padding(.bottom, 10)
            DirectoryGrid(list: plan.top, busy: busy, onOpen: onOpen, onAdd: onAdd)
                .padding(.bottom, 36)
            if !plan.official.isEmpty {
                DirectorySectionHead(title: "Official servers", count: plan.officialCount, onShowAll: onShowOfficial)
                    .padding(.bottom, 10)
                DirectoryGrid(list: plan.official, busy: busy, onOpen: onOpen, onAdd: onAdd)
                    .padding(.bottom, 36)
            }
            ForEach(plan.sections) { section in
                DirectorySectionHead(title: section.name, count: section.count, onShowAll: { onCategory(section.name) })
                    .padding(.bottom, 10)
                DirectoryGrid(list: section.members, busy: busy, onOpen: onOpen, onAdd: onAdd)
                    .padding(.bottom, 36)
            }
            if showCategoryCloud && !plan.categories.isEmpty {
                DirectorySectionHead(title: "Categories").padding(.bottom, 10)
                TeachChipFlow(spacing: 16) {
                    ForEach(plan.categories, id: \.name) { category in
                        Button { onCategory(category.name) } label: {
                            HStack(alignment: .firstTextBaseline, spacing: 6) {
                                Text(category.name)
                                    .font(.system(size: 13))
                                    .foregroundColor(Color("TextPrimary"))
                                Text("\(category.count)")
                                    .font(.system(size: 12))
                                    .foregroundColor(Color("TextTertiary"))
                            }
                            .frame(minHeight: 32)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
        }
    }
}

/// ConnectorHero: "Curated by Allternit", one swipeable card per collection,
/// tinted with the collection's accent and carrying its real app logos.
struct DirectoryHero: View {
    let pool: [DirectoryEntry]
    let onOpen: (DirectoryEntry) -> Void
    let onCollection: (String) -> Void

    private struct Slide: Identifiable {
        let collection: AppDirectory.Collection
        let members: [DirectoryEntry]
        var id: String { collection.id }
    }

    private func makeSlides() -> [Slide] {
        let byKey = Dictionary(pool.map { (AppDirectory.rankKey($0.id), $0) }, uniquingKeysWith: { first, _ in first })
        return AppDirectory.collections.compactMap { collection in
            let members = collection.ids.compactMap { byKey[$0] }
            return members.count >= 3 ? Slide(collection: collection, members: members) : nil
        }
    }

    var body: some View {
        let slides = makeSlides()
        if !slides.isEmpty {
            ScrollView(.horizontal, showsIndicators: false) {
                LazyHStack(spacing: 12) {
                    ForEach(slides) { slide in
                        card(slide).containerRelativeFrame(.horizontal)
                    }
                }
                .scrollTargetLayout()
            }
            .scrollTargetBehavior(.paging)
            .accessibilityLabel("Curated by Allternit")
        }
    }

    private func card(_ slide: Slide) -> some View {
        let accent = Color(hex: slide.collection.accent)
        return VStack(alignment: .leading, spacing: 0) {
            Text("CURATED BY ALLTERNIT")
                .font(.system(size: 11, weight: .medium))
                .tracking(0.9)
                .foregroundColor(Color("TextSecondary"))
            Text(slide.collection.title)
                .font(.system(size: 28, weight: .medium, design: .serif))
                .tracking(-0.3)
                .foregroundColor(Color("TextPrimary"))
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, 6)
            Text(slide.collection.blurb)
                .font(.system(size: 14))
                .lineSpacing(3)
                .foregroundColor(Color("TextSecondary"))
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, 8)
            HStack(spacing: 10) {
                ForEach(slide.members.prefix(5)) { member in
                    Button { onOpen(member) } label: {
                        AppLogoView(id: member.id, name: member.name, url: member.site, size: 40)
                            .shadow(color: .black.opacity(0.08), radius: 4, y: 2)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(member.name)
                }
            }
            .padding(.top, 16)
            Button { onCollection(slide.collection.id) } label: {
                Text("Explore")
                    .font(.system(size: 13, weight: .medium))
                    .foregroundColor(Color("BgPrimary"))
                    .padding(.horizontal, 14)
                    .frame(height: 36)
                    .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color("TextPrimary")))
            }
            .buttonStyle(.plain)
            .padding(.top, 18)
        }
        .padding(.horizontal, 24)
        .padding(.vertical, 28)
        .frame(maxWidth: .infinity, minHeight: 248, alignment: .leading)
        .background(
            ZStack {
                Color("BgPanel")
                LinearGradient(colors: [accent.opacity(0.26), accent.opacity(0.09)], startPoint: .topLeading, endPoint: .bottomTrailing)
            }
            .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
        )
    }
}

/// The back line on a directory sub-page (BackLink).
struct DirectoryBackLink: View {
    let label: String
    let onBack: () -> Void

    var body: some View {
        Button(action: onBack) {
            HStack(spacing: 4) {
                PhIcon("caret-left", size: 13)
                Text(label)
            }
            .font(.system(size: 13))
            .foregroundColor(Color("TextSecondary"))
            .frame(minHeight: 36)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

// MARK: - Detail

/// ConnectorDetail: logo, name and mark, who made it, what it does, the tools
/// it brings, and the one action that fits.
struct DirectoryDetailSheet: View {
    let entry: DirectoryEntry
    let onConnect: () -> Void
    let onDisconnect: () -> Void
    var onAddByURL: (() -> Void)? = nil
    /// The installed MCP connector (connected MCP apps), for "From <app>".
    var install: String? = nil

    @Environment(\.dismiss) private var dismiss
    @Environment(\.openURL) private var openURL

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 20) {
                    HStack(alignment: .top, spacing: 16) {
                        AppLogoView(id: entry.id, name: entry.name, url: entry.site, size: 64)
                        VStack(alignment: .leading, spacing: 4) {
                            HStack(spacing: 6) {
                                Text(entry.name)
                                    .font(.system(size: 20, weight: .semibold))
                                    .foregroundColor(Color("TextPrimary"))
                                    .lineLimit(2)
                                DirectoryNameMark(entry: entry, size: 15)
                            }
                            if let maker = AppDirectory.maker(of: entry) {
                                Text("by \(maker.name)")
                                    .font(.system(size: 13))
                                    .foregroundColor(Color("TextSecondary"))
                            }
                            statusLine
                        }
                    }
                    primaryAction
                    if !entry.description.isEmpty {
                        Text(entry.description)
                            .font(.system(size: 14))
                            .lineSpacing(4)
                            .foregroundColor(Color("TextSecondary"))
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    if let setup = entry.setupMessage { guideNote(setup) }
                    // The same screen Desktop and the web show, in the vendor's look (AppScreenWebView).
                    AppScreenSection(
                        target: AppScreenTarget(app: entry.id, name: entry.name, connected: entry.connected,
                                                account: entry.account, install: entry.connected ? install : nil),
                        onConnect: entry.connected || !entry.connectable ? nil : onConnect
                    )
                    if !entry.toolNames.isEmpty { tools }
                    links
                    if entry.official {
                        Text("Listed because the vendor publishes a public MCP server. Not a partnership or endorsement.")
                            .font(.system(size: 12))
                            .foregroundColor(Color("TextTertiary"))
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                .padding(20)
            }
            .background(Color("BgPrimary").ignoresSafeArea())
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button { dismiss() } label: { PhIcon("x", size: 16).foregroundColor(Color("TextSecondary")) }
                        .accessibilityLabel("Close")
                }
            }
        }
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
    }

    private var statusLine: some View {
        HStack(spacing: 6) {
            Circle().fill((entry.connected ? PolishStatus.ok : .idle).color).frame(width: 8, height: 8)
            Text(entry.connected ? (entry.account.map { "Connected as \($0)" } ?? "Connected") : "Not connected")
                .font(.system(size: 13))
                .foregroundColor(Color("TextSecondary"))
                .lineLimit(1)
        }
    }

    @ViewBuilder
    private var primaryAction: some View {
        if entry.connected {
            if entry.shelf == .connector {
                Button("Disconnect", action: onDisconnect).buttonStyle(DirectoryQuietButton())
            }
        } else if entry.connectable {
            Button(action: onConnect) {
                Text("Connect \(entry.name)").frame(maxWidth: .infinity)
            }
            .buttonStyle(DirectoryPrimaryButton())
        } else if entry.setupMessage != nil, let onAddByURL {
            Button("Add by URL", action: onAddByURL).buttonStyle(DirectoryPrimaryButton())
        } else {
            Text("This one can’t be connected from here yet.")
                .font(.system(size: 13))
                .foregroundColor(Color("TextSecondary"))
        }
    }

    private func guideNote(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 13))
            .lineSpacing(3)
            .foregroundColor(Color("TextPrimary"))
            .fixedSize(horizontal: false, vertical: true)
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color("FillNeutral")))
    }

    private var tools: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 4) {
                Text("What it can do")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                Text("· \(entry.toolNames.count)")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextTertiary"))
            }
            TeachChipFlow(spacing: 6) {
                ForEach(entry.toolNames.prefix(30), id: \.self) { tool in
                    Text(tool)
                        .font(.system(size: 12))
                        .foregroundColor(Color("TextSecondary"))
                        .padding(.horizontal, 8)
                        .padding(.vertical, 4)
                        .background(Capsule().fill(Color("FillNeutral")))
                }
            }
        }
    }

    @ViewBuilder
    private var links: some View {
        let site = AppDirectory.maker(of: entry)?.site
        if site != nil || entry.docsURL != nil {
            HStack(spacing: 16) {
                if let site, let url = URL(string: site) {
                    Button { openURL(url) } label: { linkLabel("Website") }.buttonStyle(.plain)
                }
                if let docs = entry.docsURL, let url = URL(string: docs) {
                    Button { openURL(url) } label: { linkLabel("Setup guide") }.buttonStyle(.plain)
                }
            }
        }
    }

    private func linkLabel(_ title: String) -> some View {
        HStack(spacing: 4) {
            Text(title).underline()
            PhIcon("arrow-square-out", size: 13)
        }
        .font(.system(size: 13))
        .foregroundColor(Color("TextPrimary"))
        .frame(minHeight: 36)
        .contentShape(Rectangle())
    }
}

/// `btnPrimary`: the filled button.
struct DirectoryPrimaryButton: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 14, weight: .medium))
            .foregroundColor(Color("BgPrimary"))
            .padding(.horizontal, 16)
            .frame(minHeight: 40)
            .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color("TextPrimary")))
            .opacity(configuration.isPressed ? 0.85 : 1)
    }
}

/// `btnQuiet`: the neutral button.
struct DirectoryQuietButton: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 14, weight: .medium))
            .foregroundColor(Color("TextPrimary"))
            .padding(.horizontal, 16)
            .frame(minHeight: 40)
            .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color("FillNeutral")))
            .opacity(configuration.isPressed ? 0.7 : 1)
    }
}
