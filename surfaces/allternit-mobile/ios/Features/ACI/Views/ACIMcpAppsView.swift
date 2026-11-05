import SwiftUI

/// ACI › MCP Apps, the web's `McpAppsView compact` (views/mini-apps/
/// McpAppsView.tsx): "MCP Apps" header, one compact bar (Installed ·
/// Discover · Build, search, Add by URL), and the tab's body.
///
/// Discover is the directory Desktop shows (`McpAppsDirectory`): the connector
/// catalog plus vendor-published official servers, with their real logos.
///
/// Routes (allternit-api, through the app's API route):
/// - `GET  mcp-app-installs` / `PUT mcp-app-installs` / `PATCH` and `DELETE mcp-app-installs/:appId`
/// - `GET  mcp/connectors` / `POST mcp/connectors` / `DELETE mcp/connectors/:id`
/// - `POST mcp/connectors/:id/oauth/start` → `{ authorize_url }`
/// - `GET  connectors` (the catalog), `POST connectors/:id/connect`
/// - `POST /api/mcp/apps` `tools/list` (on the person's computer)
/// - `GET  miniapps/submissions` (Build › Your apps)
struct ACIMcpAppsView: View {
    enum Tab: String, CaseIterable {
        case installed, discover, build

        var label: String {
            switch self {
            case .installed: return "Installed"
            case .discover: return "Discover"
            case .build: return "Build"
            }
        }
    }

    /// McpAppsDirectory's routes.
    enum Route: Equatable {
        case landing, all, official
        case category(String)
        case collection(String)
    }

    /// DISCOVER_FILTERS that have anything behind them on the phone.
    enum SourceFilter: String, CaseIterable {
        case all, official, styled

        var label: String {
            switch self {
            case .all: return "All sources"
            case .official: return "Official servers"
            case .styled: return "Styled by Allternit"
            }
        }
    }

    private static let pageSize = 48

    @StateObject private var model = ACIMcpAppsModel()
    @StateObject private var connect = DirectoryConnectModel()
    @Environment(\.openURL) private var openURL
    @State private var tab: Tab = .installed
    @State private var query = ""
    @State private var isAddOpen = false
    @State private var selected: ACIMcpAppsModel.InstalledApp? = nil
    @State private var route: Route = .landing
    @State private var category: String? = nil
    @State private var filter: SourceFilter = .all
    @State private var limit = ACIMcpAppsView.pageSize
    @State private var detail: DirectoryEntry? = nil
    @State private var isConnectorsOpen = false

    var body: some View {
        VStack(spacing: 0) {
            ACIViewHeader(title: "MCP Apps", subtitle: "Connect apps and services the agent can use.", bottom: 16)
                .padding(.horizontal, 16)
            ACICompactBar(query: $query, searchLabel: "Search MCP Apps", placeholder: "Search apps") {
                ForEach(Tab.allCases, id: \.self) { item in
                    ACIBarChip(title: item.label, isOn: tab == item) { go(item) }
                }
            } actions: {
                ACIBarIcon(systemName: "plus", label: "Add by URL") { isAddOpen = true }
            }
            .onChange(of: query) { _, next in
                limit = Self.pageSize
                if !next.isEmpty {
                    route = .landing
                    if tab == .build { go(.discover) }
                }
            }
            if let notice = model.notice ?? connect.note {
                HStack(spacing: 12) {
                    Text(notice)
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextPrimary"))
                        .frame(maxWidth: .infinity, alignment: .leading)
                    Button("Dismiss") { model.notice = nil; connect.note = nil }
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                        .underline()
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 8)
                .background(Color("FillNeutral"))
                .overlay(alignment: .bottom) { Rectangle().fill(Color("BorderSubtle")).frame(height: 1) }
            }
            switch tab {
            case .installed: installed
            case .discover: discover
            case .build: ACIMcpBuildTab(model: model)
            }
        }
        .background(Color("BgPrimary"))
        .task {
            connect.connectServer = { [model] entry in
                try await model.installServer(appId: entry.id, name: entry.name, serverURL: entry.serverURL ?? "").authorizeURL
            }
            connect.onChanged = { [model] in await model.refresh() }
            await model.refresh()
            #if DEBUG
            // `-open-mcp-app <id>` (DEBUG only): Discover with that app's page open, for screenshots.
            if let id = UserDefaults.standard.string(forKey: "open-mcp-app"), detail == nil {
                tab = .discover
                detail = directory.first { $0.id == id || $0.id == "mcp:\(id)" }
            }
            #endif
        }
        .sheet(isPresented: $isAddOpen) {
            ACIAddByURLSheet { url in
                let result = try await model.addByURL(url)
                if let authorize = result.authorizeURL { connect.signIn = .init(url: authorize) }
                model.notice = result.note
            }
        }
        .sheet(item: $selected) { app in
            ACIInstalledAppSheet(app: app, model: model, connect: connect) { isConnectorsOpen = true }
        }
        .sheet(isPresented: $isConnectorsOpen, onDismiss: { Task { await model.refresh() } }) {
            ConnectorsListView()
        }
        .modifier(DirectoryFlow(selected: $detail, model: connect, onAddByURL: { isAddOpen = true }, installFor: { entry in
            // McpAppsDirectory's currentInstall: the connected install whose app id is this entry's.
            let id = entry.id.hasPrefix("mcp:") ? String(entry.id.dropFirst(4)) : entry.id
            return model.apps.first { app in
                guard app.connected, !app.readOnly else { return false }
                let bare = app.appId.replacingOccurrences(of: "^(connector:|mcp:)", with: "", options: .regularExpression)
                return [app.appId, bare].contains(entry.id) || [app.appId, bare].contains(id)
            }?.connectorId
        }))
    }

    private func go(_ next: Tab) {
        tab = next
        model.notice = nil
        connect.note = nil
    }

    // MARK: Installed

    private var q: String { query.trimmingCharacters(in: .whitespaces).lowercased() }

    @ViewBuilder
    private var installed: some View {
        if !model.loaded {
            ScrollView {
                PolishCardSkeletons(count: 6).padding(.horizontal, 16).padding(.top, 20)
            }
        } else if let failure = model.failure, model.apps.isEmpty {
            ScrollView {
                SettingsFailureView(failure: failure, subject: "your apps") { Task { await model.refresh() } }
                    .padding(16)
            }
        } else {
            let shown = q.isEmpty ? model.apps : model.apps.filter { $0.name.lowercased().contains(q) }
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    if let failure = model.failure {
                        HStack(spacing: 8) {
                            Text(failure.message).foregroundColor(Color("TextSecondary"))
                            Button("Retry") { Task { await model.refresh() } }
                                .fontWeight(.medium)
                                .foregroundColor(Color("TextPrimary"))
                                .underline()
                        }
                        .font(.system(size: 14))
                    }
                    if shown.isEmpty {
                        VStack(spacing: 12) {
                            Text(q.isEmpty ? "No apps connected yet. Add one and it shows up here." : "No installed apps match “\(query.trimmingCharacters(in: .whitespaces))”.")
                                .font(.system(size: 14))
                                .foregroundColor(Color("TextSecondary"))
                                .multilineTextAlignment(.center)
                            if q.isEmpty {
                                Button("Browse apps") { go(.discover) }
                                    .buttonStyle(DirectoryPrimaryButton())
                            }
                        }
                        .frame(maxWidth: .infinity)
                        .padding(.horizontal, 24)
                        .padding(.vertical, 40)
                        .background(Color("BgPanel"))
                        .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
                        .padding(.top, 16)
                    } else {
                        PolishCardGrid {
                            ForEach(shown) { app in installedCard(app) }
                        }
                    }
                }
                .padding(.horizontal, 16)
                .padding(.top, 20)
                .padding(.bottom, 40)
            }
            .refreshable { await model.refresh() }
        }
    }

    /// AppCard in its grid: logo, status, the one fact, the permission mode.
    private func installedCard(_ app: ACIMcpAppsModel.InstalledApp) -> some View {
        PolishCard(
            name: app.name,
            description: app.description,
            status: app.connected ? .ok : .attention,
            statusLabel: app.connected ? "Connected" : "Needs sign-in",
            footer: app.connected ? "Connected" : "Needs sign-in",
            onOpen: { selected = app },
            logo: { AppLogoView(id: app.logoKey ?? app.appId, name: app.name, url: app.url, size: 52) },
            action: {
                if !app.connected && !app.readOnly {
                    Button("Reconnect") { Task { await reconnect(app) } }
                        .font(.system(size: 12, weight: .medium))
                        .foregroundColor(Color("TextPrimary"))
                        .padding(.horizontal, 10)
                        .frame(height: 28)
                        .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color("BgPanel")))
                        .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
                        .buttonStyle(.plain)
                }
            },
            extras: {
                if app.official { VerifiedMark(size: 14) }
            },
            controls: {
                Group {
                    if app.readOnly {
                        Text("Managed in Connectors")
                            .font(.system(size: 12))
                            .foregroundColor(Color("TextTertiary"))
                            .padding(.vertical, 6)
                    } else {
                        ACIPermissionMenu(app: app, model: model)
                    }
                }
                .padding(.horizontal, 10)
                .padding(.bottom, 6)
            }
        )
    }

    private func reconnect(_ app: ACIMcpAppsModel.InstalledApp) async {
        model.notice = nil
        do {
            connect.signIn = .init(url: try await model.startOAuth(connectorId: app.connectorId))
        } catch {
            model.notice = ACIMcpAppsModel.friendly(error, "Couldn’t start sign-in for \(app.name). Try again.")
        }
    }

    // MARK: Discover

    private var directory: [DirectoryEntry] {
        AppDirectory.merged(catalog: model.catalog, installedIds: model.installedIds)
    }

    private func source(_ entry: DirectoryEntry) -> SourceFilter { entry.official ? .official : .styled }

    @ViewBuilder
    private var discover: some View {
        let all = directory
        VStack(spacing: 0) {
            if case .landing = route { categoryRow(all) }
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    if model.catalogState == .failed {
                        HStack(spacing: 8) {
                            Text(model.catalogMessage ?? "Couldn’t load apps.").foregroundColor(Color("TextSecondary"))
                            Button("Retry") { Task { await model.loadCatalog() } }
                                .fontWeight(.medium)
                                .foregroundColor(Color("TextPrimary"))
                                .underline()
                        }
                        .font(.system(size: 14))
                    }
                    if model.catalogState == .loading && model.catalog.isEmpty {
                        PolishCardSkeletons(count: 8)
                    } else {
                        discoverBody(all)
                    }
                }
                .padding(.horizontal, 16)
                .padding(.top, 16)
                .padding(.bottom, 40)
            }
            .refreshable { await model.refresh() }
        }
    }

    @ViewBuilder
    private func discoverBody(_ all: [DirectoryEntry]) -> some View {
        let available = all.filter { !$0.connected }
        let browsing = q.isEmpty && category == nil && filter == .all
        switch route {
        case .landing where browsing:
            DirectoryLanding(
                list: available, pool: all, total: all.count, noun: "apps", busy: connect.busy,
                onOpen: { detail = $0 }, onAdd: add,
                onCategory: { category = $0 },
                onShowAll: { route = .all },
                onCollection: { route = .collection($0) },
                onShowOfficial: { route = .official },
                showCategoryCloud: false
            )
        case .landing:
            results(all)
        case .all:
            page("All apps", list: AppDirectory.ranked(available))
        case .official:
            page("Official servers", list: AppDirectory.ranked(available.filter(\.official)))
        case .category(let name):
            page(name, list: AppDirectory.ranked(available.filter { $0.category == name }))
        case .collection(let id):
            let collection = AppDirectory.collections.first { $0.id == id }
            let byKey = Dictionary(available.map { (AppDirectory.rankKey($0.id), $0) }, uniquingKeysWith: { first, _ in first })
            page(collection?.title ?? "Apps", list: (collection?.ids ?? []).compactMap { byKey[$0] })
        }
    }

    /// ConnectorListPage.
    private func page(_ title: String, list: [DirectoryEntry]) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            DirectoryBackLink(label: "MCP Apps") { route = .landing; limit = Self.pageSize }
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(title)
                    .font(.system(size: 24, weight: .medium, design: .serif))
                    .foregroundColor(Color("TextPrimary"))
                Text(list.count.formatted())
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextTertiary"))
            }
            paged(list, emptyText: "Nothing here yet.")
        }
    }

    private func results(_ all: [DirectoryEntry]) -> some View {
        let list = AppDirectory.ranked(all).filter { entry in
            (filter == .all || source(entry) == filter)
                && (category == nil || entry.category == category)
                && (q.isEmpty || "\(entry.id)\n\(entry.name)\n\(entry.description)\n\(entry.category)".lowercased().contains(q))
        }
        return VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(category ?? (q.isEmpty ? "All apps" : "Results"))
                    .font(.system(size: 14, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                Text(list.count.formatted())
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextTertiary"))
            }
            paged(list, emptyText: q.isEmpty ? "Nothing here yet." : "Nothing matches “\(query.trimmingCharacters(in: .whitespaces))”.")
        }
    }

    @ViewBuilder
    private func paged(_ list: [DirectoryEntry], emptyText: String) -> some View {
        if list.isEmpty {
            Text(emptyText)
                .font(.system(size: 14))
                .foregroundColor(Color("TextSecondary"))
                .frame(maxWidth: .infinity)
                .padding(.vertical, 40)
        } else {
            DirectoryGrid(list: Array(list.prefix(limit)), busy: connect.busy, onOpen: { detail = $0 }, onAdd: add)
            if list.count > limit {
                Button("Show more") { limit += Self.pageSize }
                    .buttonStyle(DirectoryQuietButton())
                    .frame(maxWidth: .infinity)
                    .padding(.top, 8)
            }
        }
    }

    private func add(_ entry: DirectoryEntry) {
        if entry.connectable {
            Task { await connect.connect(entry) }
        } else {
            detail = entry
        }
    }

    /// The pills: All, the top categories, More, then the source filter.
    private func categoryRow(_ all: [DirectoryEntry]) -> some View {
        let categories = AppDirectory.categoryCounts(all)
        let top = categories.prefix(5)
        let rest = categories.dropFirst(5)
        let restActive = rest.contains { $0.name == category }
        return ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                pill("All", isOn: category == nil) { category = nil }
                ForEach(top, id: \.name) { item in
                    pill(item.name, isOn: category == item.name) { category = item.name }
                }
                if !rest.isEmpty {
                    Menu {
                        ForEach(rest, id: \.name) { item in
                            Button("\(item.name)  \(item.count)") { category = item.name }
                        }
                    } label: {
                        pillLabel(restActive ? (category ?? "More") : "More", isOn: restActive, caret: true)
                    }
                    .accessibilityLabel("More categories")
                }
                Menu {
                    ForEach(SourceFilter.allCases, id: \.self) { option in
                        Button { filter = option } label: {
                            if filter == option { Label(option.label, systemImage: "checkmark") } else { Text(option.label) }
                        }
                    }
                } label: {
                    HStack(spacing: 4) {
                        Text(filter.label)
                        PhIcon("caret-down", size: 12)
                    }
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextPrimary"))
                    .padding(.horizontal, 12)
                    .frame(height: 32)
                    .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color("BgPanel")))
                    .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
                }
                .accessibilityLabel("Filter by source")
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
        }
    }

    private func pill(_ title: String, isOn: Bool, action: @escaping () -> Void) -> some View {
        Button(action: { action(); limit = Self.pageSize }) { pillLabel(title, isOn: isOn, caret: false) }
            .buttonStyle(.plain)
            .accessibilityAddTraits(isOn ? .isSelected : [])
    }

    private func pillLabel(_ title: String, isOn: Bool, caret: Bool) -> some View {
        HStack(spacing: 4) {
            Text(title).lineLimit(1)
            if caret { PhIcon("caret-down", size: 12) }
        }
        .font(.system(size: 13, weight: .medium))
        .foregroundColor(isOn ? Color("BgPrimary") : Color("TextSecondary"))
        .padding(.horizontal, 14)
        .frame(height: 32)
        .background(Capsule().fill(isOn ? Color("TextPrimary") : Color("FillNeutral")))
        .fixedSize()
    }
}

// MARK: - Model

@MainActor
final class ACIMcpAppsModel: ObservableObject {
    struct InstallRecord: Decodable, Sendable {
        let appId: String
        let connectorId: String
        let permissionMode: String?
    }

    struct Connector: Decodable, Sendable {
        let id: String
        let name: String
        let name_id: String?
        let url: String?
        let enabled: Bool?
        let oauth_status: String?
        let needs_reauth: Bool?
    }

    struct InstalledApp: Identifiable, Sendable {
        let appId: String
        let connectorId: String
        let name: String
        let url: String?
        let connected: Bool
        var permissionMode: String = ACIMcpAppsModel.defaultMode
        /// Catalog id or `name_id`, for the logo.
        var logoKey: String? = nil
        var description: String? = nil
        /// A connected catalog service: shown here, managed in Connectors.
        var readOnly: Bool = false
        /// The vendor's own server (the blue mark).
        var official: Bool = false

        var id: String { appId }
        var host: String? { url.flatMap { URL(string: $0)?.host } }
    }

    /// One tool of an installed app (`tools/list`), read for "What it can do".
    struct Tool: Identifiable, Sendable {
        /// reads | changes | important
        let effect: String
        let name: String
        let description: String?
        var id: String { name }
    }

    enum CatalogState: Sendable { case loading, ready, failed }

    /// APP_PERMISSION_MODE_LABELS, in order.
    nonisolated static let defaultMode = "ask_before_changes"
    nonisolated static let permissionModes: [(id: String, label: String)] = [
        ("always_ask", "Always ask"),
        ("ask_before_changes", "Ask before changes"),
        ("ask_before_important_changes", "Ask before important changes"),
    ]

    static func modeLabel(_ id: String) -> String {
        permissionModes.first { $0.id == id }?.label ?? "Ask before changes"
    }

    struct Submission: Decodable, Identifiable, Sendable {
        struct Body: Decodable, Sendable { let name: String }
        let id: String
        let state: String
        let submission: Body
        let rejectionReason: String?
    }

    @Published private(set) var apps: [InstalledApp] = []
    @Published private(set) var loaded = false
    @Published private(set) var failure: SettingsLoadFailure? = nil
    @Published var notice: String? = nil
    @Published private(set) var catalog: [DirectoryEntry] = []
    @Published private(set) var catalogState: CatalogState = .loading
    @Published private(set) var catalogMessage: String? = nil
    /// Directory ids whose install has finished signing in.
    @Published private(set) var installedIds: Set<String> = []

    private struct InstallsEnvelope: Decodable { let items: [InstallRecord]? }
    private struct ConnectorsEnvelope: Decodable { let connectors: [Connector]? }
    private struct SubmissionsEnvelope: Decodable { let items: [Submission]? }
    private struct Created: Decodable { let id: String }
    private struct OAuthStart: Decodable { let authorize_url: String? }
    private struct EmptyBody: Encodable {}
    private struct CreateConnector: Encodable { let name: String; let name_id: String; let url: String; let type: String }
    private struct PutInstall: Encodable { let appId: String; let connectorId: String }
    private struct PatchMode: Encodable { let permissionMode: String }

    /// mini-apps-model.ts `REAUTH_STATES`-style statuses that need a new sign-in.
    private static let reauthStates: Set<String> = ["expired", "error", "revoked", "pending", "needs_reauth"]

    func refresh() async {
        async let installsResult: InstallsEnvelope = APIClient.shared.get(path: "mcp-app-installs")
        async let connectorsResult: ConnectorsEnvelope = APIClient.shared.get(path: "mcp/connectors")
        async let catalogLoad: Void = loadCatalog()
        var firstError: Error? = nil
        var installs: InstallsEnvelope? = nil
        var connectors: ConnectorsEnvelope? = nil
        do { installs = try await installsResult } catch { firstError = error }
        do { connectors = try await connectorsResult } catch { firstError = firstError ?? error }
        _ = await catalogLoad
        if Self.isCancelled(firstError) { return }
        failure = firstError.map(SettingsLoadFailure.from)

        // A connector added outside this tab has no install record yet: adopt
        // it so it shows here and its permission mode applies (connectorsToAdopt).
        if let known = installs?.items, let rows = connectors?.connectors {
            let installed = Set(known.map(\.connectorId))
            let orphans = rows.filter { !installed.contains($0.id) }
            if !orphans.isEmpty {
                for orphan in orphans {
                    try? await APIClient.shared.put(path: "mcp-app-installs",
                                                    body: PutInstall(appId: "mcp:\(orphan.name_id ?? orphan.id)", connectorId: orphan.id))
                }
                if let again: InstallsEnvelope = try? await APIClient.shared.get(path: "mcp-app-installs") { installs = again }
            }
        }

        let byId = Dictionary((connectors?.connectors ?? []).map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        let officialIds = Set(McpOfficialServers.servers.map { "mcp:\($0.id)" })
        var joined: [InstalledApp] = (installs?.items ?? []).map { record in
            let connector = byId[record.connectorId]
            return InstalledApp(
                appId: record.appId,
                connectorId: record.connectorId,
                name: connector?.name ?? record.appId.replacingOccurrences(of: #"^(mcp:|url:)"#, with: "", options: .regularExpression),
                url: connector?.url,
                connected: Self.isConnected(connector),
                permissionMode: record.permissionMode ?? Self.defaultMode,
                logoKey: connector?.name_id,
                official: officialIds.contains(record.appId)
            )
        }
        // Connected catalog services are MCP Apps too (joinInstalled).
        let names = Set(joined.map { $0.name.lowercased() })
        joined += catalog
            .filter { $0.connected && !names.contains($0.name.lowercased()) }
            .map { service in
                InstalledApp(appId: "connector:\(service.id)", connectorId: "", name: service.name, url: service.baseURL,
                             connected: true, logoKey: service.id, description: service.description, readOnly: true)
            }
        apps = joined
        var ids = Set<String>()
        for app in joined where app.connected {
            ids.insert(app.appId)
            ids.insert(app.appId.replacingOccurrences(of: #"^(connector:|mcp:)"#, with: "", options: .regularExpression))
        }
        installedIds = ids
        loaded = true
    }

    private static func isCancelled(_ error: Error?) -> Bool {
        if error is CancellationError { return true }
        if let urlError = error as? URLError, urlError.code == .cancelled { return true }
        return false
    }

    /// `GET connectors`: the catalog behind Discover. Failing leaves the
    /// official servers listed, as on Desktop.
    func loadCatalog() async {
        if catalog.isEmpty { catalogState = .loading }
        do {
            let rows = try await ConnectorsClient().listConnectors()
            catalog = rows.map(DirectoryEntry.init(connector:))
            catalogState = .ready
            catalogMessage = nil
        } catch is CancellationError {
        } catch {
            catalogState = .failed
            catalogMessage = SettingsLoadFailure.from(error).message
        }
    }

    /// `PATCH mcp-app-installs/:appId`, shown at once and put back if it fails.
    func setPermissionMode(_ app: InstalledApp, mode: String) async {
        guard let index = apps.firstIndex(where: { $0.appId == app.appId }) else { return }
        let previous = apps[index].permissionMode
        apps[index].permissionMode = mode
        do {
            try await APIClient.shared.patch(path: "mcp-app-installs/\(Self.escape(app.appId))", body: PatchMode(permissionMode: mode))
        } catch {
            if let again = apps.firstIndex(where: { $0.appId == app.appId }) { apps[again].permissionMode = previous }
            notice = Self.friendly(error, "Couldn’t save that change. Try again.")
        }
    }

    /// `POST /api/mcp/apps` `tools/list`, on the person's computer.
    func tools(connectorId: String) async throws -> [Tool] {
        struct Request: Encodable { let action: String; let connectorId: String }
        struct Response: Decodable {
            struct Row: Decodable {
                struct Hints: Decodable { let readOnlyHint: Bool?; let destructiveHint: Bool? }
                let name: String
                let title: String?
                let description: String?
                let annotations: Hints?
            }
            let tools: [Row]?
        }
        let client = APIClient.shared
        let base = client.baseURL.lastPathComponent == "v1" ? client.baseURL.deletingLastPathComponent() : client.baseURL
        var request = try await client.authorizedRequest(url: base.appendingPathComponent("mcp/apps"), method: "POST")
        request.httpBody = try JSONEncoder().encode(Request(action: "tools/list", connectorId: connectorId))
        let (data, response) = try await InstanceConnection.runtimeAPIData(for: request, session: client.session)
        try client.validate(response, data: data)
        let rows = try JSONDecoder().decode(Response.self, from: data).tools ?? []
        return rows.map { row in
            let effect = row.annotations?.readOnlyHint == true ? "reads" : row.annotations?.destructiveHint == true ? "important" : "changes"
            return Tool(effect: effect, name: row.title ?? row.name, description: row.description)
        }
    }

    private static func isConnected(_ connector: Connector?) -> Bool {
        guard let connector else { return false }
        if connector.enabled == false || connector.needs_reauth == true { return false }
        if let status = connector.oauth_status, reauthStates.contains(status) { return false }
        return true
    }

    func startOAuth(connectorId: String) async throws -> URL {
        let body: OAuthStart = try await APIClient.shared.post(path: "mcp/connectors/\(Self.escape(connectorId))/oauth/start", body: EmptyBody())
        guard let raw = body.authorize_url, let url = URL(string: raw) else {
            throw ACIComputersClient.Failure.server("Connector OAuth start returned no authorize URL.")
        }
        return url
    }

    /// install-server.ts: create the connector (or reuse one from an earlier
    /// try), record the install, start OAuth.
    func installServer(appId: String, name: String, serverURL: String) async throws -> (note: String, authorizeURL: URL?) {
        let nameId = name.lowercased()
            .replacingOccurrences(of: "[^a-z0-9]+", with: "-", options: .regularExpression)
            .trimmingCharacters(in: CharacterSet(charactersIn: "-"))
        var connectorId: String
        do {
            let created: Created = try await APIClient.shared.post(
                path: "mcp/connectors",
                body: CreateConnector(name: name, name_id: nameId, url: serverURL, type: "streamable-http")
            )
            connectorId = created.id
        } catch {
            let existing: ConnectorsEnvelope? = try? await APIClient.shared.get(path: "mcp/connectors")
            guard let again = existing?.connectors?.first(where: { $0.url == serverURL })?.id else { throw error }
            connectorId = again
        }
        try await APIClient.shared.put(path: "mcp-app-installs", body: PutInstall(appId: appId, connectorId: connectorId))
        let authorize = try await startOAuth(connectorId: connectorId)
        await refresh()
        return ("Added \(name). Finish sign-in in the new window.", authorize)
    }

    func addByURL(_ raw: String) async throws -> (note: String, authorizeURL: URL?) {
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let host = URL(string: trimmed)?.host, !host.isEmpty else {
            throw ACIComputersClient.Failure.server("Enter a full server address, like https://example.com/mcp.")
        }
        return try await installServer(appId: "url:\(host)", name: host, serverURL: trimmed)
    }

    func remove(_ app: InstalledApp) async {
        do {
            // The connector first: it ends the app's subscriptions and its stored sign-in.
            if !app.connectorId.isEmpty {
                try await APIClient.shared.delete(path: "mcp/connectors/\(Self.escape(app.connectorId))")
            }
            try await APIClient.shared.delete(path: "mcp-app-installs/\(Self.escape(app.appId))")
            await refresh()
        } catch {
            notice = Self.friendly(error, "Couldn’t remove \(app.name). Try again.")
        }
    }

    func submissions() async throws -> [Submission] {
        let envelope: SubmissionsEnvelope = try await APIClient.shared.get(path: "miniapps/submissions")
        return envelope.items ?? []
    }

    static func friendly(_ error: Error, _ fallback: String) -> String {
        let message = error.localizedDescription
        return message.isEmpty ? fallback : FriendlyErrorMessage.from(message)
    }

    private static func escape(_ value: String) -> String {
        value.addingPercentEncoding(withAllowedCharacters: .alphanumerics.union(CharacterSet(charactersIn: "-_.~"))) ?? value
    }
}

// MARK: - Pieces

/// The permission select on an installed card.
private struct ACIPermissionMenu: View {
    let app: ACIMcpAppsModel.InstalledApp
    @ObservedObject var model: ACIMcpAppsModel

    var body: some View {
        Menu {
            ForEach(ACIMcpAppsModel.permissionModes, id: \.id) { mode in
                Button {
                    Task { await model.setPermissionMode(app, mode: mode.id) }
                } label: {
                    if app.permissionMode == mode.id { Label(mode.label, systemImage: "checkmark") } else { Text(mode.label) }
                }
            }
        } label: {
            HStack(spacing: 4) {
                Text(ACIMcpAppsModel.modeLabel(app.permissionMode))
                    .lineLimit(1)
                    .frame(maxWidth: .infinity, alignment: .leading)
                PhIcon("caret-down", size: 12)
            }
            .font(.system(size: 12))
            .foregroundColor(Color("TextPrimary"))
            .padding(.horizontal, 8)
            .frame(height: 32)
            .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color("BgPanel")))
            .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
        }
        .accessibilityLabel("Permission for \(app.name)")
    }
}

/// AppLogo in the Build tab: the real logo when the name has one.
struct ACIAppLogo: View {
    let name: String

    var body: some View {
        AppLogoView(name: name, size: 40)
    }
}

/// AddByUrlDialog: paste an MCP server address; sign-in follows.
private struct ACIAddByURLSheet: View {
    let onAdd: (String) async throws -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var url = ""
    @State private var busy = false
    @State private var error: String? = nil
    @FocusState private var focused: Bool

    init(onAdd: @escaping (String) async throws -> Void) {
        self.onAdd = onAdd
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            VStack(alignment: .leading, spacing: 4) {
                Text("Add by URL")
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                Text("Paste the address of an MCP server. You’ll sign in to it next.")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
            }
            TextField("https://example.com/mcp", text: $url)
                .font(.system(size: 15))
                .keyboardType(.URL)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .focused($focused)
                .padding(.horizontal, 12)
                .frame(height: 40)
                .background(Color("BgPanel"))
                .clipShape(RoundedRectangle(cornerRadius: 8))
                .overlay(RoundedRectangle(cornerRadius: 8).stroke(focused ? Color("TextPrimary") : Color("BorderStrong"), lineWidth: 1))
                .accessibilityLabel("MCP server URL")
            if let error {
                Text(error)
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
            }
            HStack(spacing: 8) {
                Spacer()
                Button("Cancel") { dismiss() }
                    .font(.system(size: 14, weight: .medium))
                    .foregroundColor(Color("TextSecondary"))
                    .padding(.horizontal, 14)
                    .frame(height: 36)
                Button { submit() } label: {
                    Text(busy ? "Adding…" : "Add")
                        .font(.system(size: 14, weight: .medium))
                        .foregroundColor(Color("OnAccent"))
                        .padding(.horizontal, 16)
                        .frame(height: 36)
                        .background(Color("AccentPrimary"))
                        .clipShape(RoundedRectangle(cornerRadius: 8))
                }
                .buttonStyle(.plain)
                .disabled(url.trimmingCharacters(in: .whitespaces).isEmpty || busy)
                .opacity(url.trimmingCharacters(in: .whitespaces).isEmpty || busy ? 0.5 : 1)
            }
            Spacer(minLength: 0)
        }
        .padding(20)
        .background(Color("BgPanel").ignoresSafeArea())
        .presentationDetents([.height(260)])
        .presentationDragIndicator(.visible)
        .onAppear { focused = true }
    }

    private func submit() {
        busy = true
        error = nil
        Task {
            defer { busy = false }
            do {
                try await onAdd(url)
                dismiss()
            } catch {
                self.error = ACIMcpAppsModel.friendly(error, "Couldn’t add that server. Check the address and try again.")
            }
        }
    }
}

/// The installed app's detail (DetailModal + AppDrawerBody): logo and source,
/// status, permission, the tools it brings, Use in chat and Remove.
private struct ACIInstalledAppSheet: View {
    let app: ACIMcpAppsModel.InstalledApp
    @ObservedObject var model: ACIMcpAppsModel
    @ObservedObject var connect: DirectoryConnectModel
    let onManageConnectors: () -> Void

    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var modeStore: AppModeStore
    /// nil = still loading.
    @State private var tools: [ACIMcpAppsModel.Tool]? = nil
    @State private var toolsFailed = false
    @State private var showAllTools = false

    /// The live row, so a permission change shows straight away.
    private var current: ACIMcpAppsModel.InstalledApp {
        model.apps.first { $0.appId == app.appId } ?? app
    }

    var body: some View {
        VStack(spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: 20) {
                    header
                    status
                    if let description = current.description, !description.isEmpty {
                        Text(description)
                            .font(.system(size: 14))
                            .lineSpacing(4)
                            .foregroundColor(Color("TextSecondary"))
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    if current.readOnly {
                        Text("This service is connected through Connectors. Manage or disconnect it there.")
                            .font(.system(size: 13))
                            .foregroundColor(Color("TextSecondary"))
                    } else {
                        permission
                        toolList
                    }
                    // The same screen Desktop and the web show, plus the vendor's own views (AppScreenWebView).
                    AppScreenSection(target: AppScreenTarget(
                        app: (current.logoKey ?? current.appId).replacingOccurrences(of: "^(connector:|mcp:)", with: "", options: .regularExpression),
                        name: current.name,
                        connected: current.connected,
                        install: current.readOnly ? nil : current.connectorId
                    ))
                }
                .padding(20)
            }
            Rectangle().fill(Color("BorderSubtle")).frame(height: 1)
            HStack(spacing: 8) {
                if current.readOnly {
                    Button("Open Connectors") { dismiss(); onManageConnectors() }
                        .buttonStyle(DirectoryQuietButton())
                } else {
                    Button("Remove") {
                        Task {
                            await model.remove(current)
                            dismiss()
                        }
                    }
                    .buttonStyle(DirectoryQuietButton())
                }
                Spacer()
                Button("Use in chat") {
                    dismiss()
                    modeStore.selectBarItem(.chats)
                }
                .buttonStyle(DirectoryPrimaryButton())
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 12)
        }
        .background(Color("BgPanel").ignoresSafeArea())
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
        .task {
            guard !current.readOnly else { return }
            do {
                tools = try await model.tools(connectorId: current.connectorId)
            } catch is CancellationError {
            } catch {
                toolsFailed = true
                tools = []
            }
        }
    }

    private var header: some View {
        HStack(alignment: .top, spacing: 16) {
            AppLogoView(id: current.logoKey ?? current.appId, name: current.name, url: current.url, size: 64)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(current.name)
                        .font(.system(size: 20, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(2)
                    if current.official { VerifiedMark(size: 15) }
                }
                Text(current.official ? "Official server" : current.readOnly ? "Styled by Allternit" : (current.host ?? "Added by you"))
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
            }
        }
    }

    private var status: some View {
        HStack(spacing: 8) {
            Circle()
                .fill((current.connected ? PolishStatus.ok : .attention).color)
                .frame(width: 8, height: 8)
            Text(current.connected ? "Connected" : "Sign-in expired")
                .font(.system(size: 13))
                .foregroundColor(Color("TextSecondary"))
            Spacer()
            if !current.connected && !current.readOnly {
                Button("Reconnect") {
                    Task {
                        do {
                            let url = try await model.startOAuth(connectorId: current.connectorId)
                            dismiss()
                            connect.signIn = .init(url: url)
                        } catch {
                            model.notice = ACIMcpAppsModel.friendly(error, "Couldn’t start sign-in for \(current.name). Try again.")
                            dismiss()
                        }
                    }
                }
                .font(.system(size: 12, weight: .medium))
                .foregroundColor(Color("TextPrimary"))
                .padding(.horizontal, 10)
                .frame(height: 32)
                .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color("FillNeutral")))
                .buttonStyle(.plain)
            }
        }
    }

    private var permission: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("When the app wants to act")
                .font(.system(size: 13, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
                .padding(.bottom, 4)
            ForEach(ACIMcpAppsModel.permissionModes, id: \.id) { mode in
                Button {
                    Task { await model.setPermissionMode(current, mode: mode.id) }
                } label: {
                    HStack(spacing: 10) {
                        ZStack {
                            Circle().stroke(Color("BorderStrong"), lineWidth: 1).frame(width: 16, height: 16)
                            if current.permissionMode == mode.id {
                                Circle().fill(Color("TextPrimary")).frame(width: 9, height: 9)
                            }
                        }
                        Text(mode.label)
                            .font(.system(size: 13))
                            .foregroundColor(Color("TextPrimary"))
                        Spacer()
                    }
                    .frame(minHeight: 36)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(current.permissionMode == mode.id ? .isSelected : [])
            }
        }
    }

    private var toolList: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 4) {
                Text("What it can do")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                if let tools, !tools.isEmpty {
                    Text("· \(tools.count)")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextTertiary"))
                }
            }
            if let tools {
                if toolsFailed {
                    note("Couldn’t load the list of tools.")
                } else if tools.isEmpty {
                    note("The list of tools appears after you connect.")
                } else {
                    let shown = showAllTools ? tools : Array(tools.prefix(30))
                    VStack(spacing: 0) {
                        ForEach(Array(shown.enumerated()), id: \.element.id) { index, tool in
                            if index > 0 { Rectangle().fill(Color("BorderSubtle")).frame(height: 1) }
                            HStack(alignment: .top, spacing: 12) {
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(tool.name)
                                        .font(.system(size: 13, weight: .medium))
                                        .foregroundColor(Color("TextPrimary"))
                                        .lineLimit(1)
                                    if let description = tool.description, !description.isEmpty {
                                        Text(description)
                                            .font(.system(size: 12))
                                            .foregroundColor(Color("TextSecondary"))
                                            .lineLimit(2)
                                    }
                                }
                                .frame(maxWidth: .infinity, alignment: .leading)
                                effectTag(tool.effect)
                            }
                            .padding(.horizontal, 12)
                            .padding(.vertical, 8)
                        }
                    }
                    .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
                    if tools.count > 30 && !showAllTools {
                        Button("Show all \(tools.count)") { showAllTools = true }
                            .font(.system(size: 13, weight: .medium))
                            .foregroundColor(Color("TextPrimary"))
                            .underline()
                            .frame(minHeight: 36)
                    }
                }
            } else {
                note("Loading…")
            }
        }
    }

    private func note(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 13))
            .foregroundColor(Color("TextTertiary"))
    }

    /// EffectTag.
    private func effectTag(_ effect: String) -> some View {
        let important = effect == "important"
        return Text(effect == "reads" ? "Reads" : important ? "Important" : "Changes")
            .font(.system(size: 11, weight: .medium))
            .foregroundColor(important ? Color("BgPanel") : Color("TextSecondary"))
            .padding(.horizontal, 8)
            .padding(.vertical, 2)
            .background(Capsule().fill(important ? Color("TextPrimary") : Color("FillNeutral")))
    }
}

/// Build: the apps you've submitted to the directory, and the docs link.
private struct ACIMcpBuildTab: View {
    @ObservedObject var model: ACIMcpAppsModel
    @State private var mine: [ACIMcpAppsModel.Submission]? = nil
    @State private var loadFailed = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 32) {
                VStack(alignment: .leading, spacing: 12) {
                    Text("Your apps")
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                    if loadFailed {
                        HStack(spacing: 8) {
                            Text("Couldn’t load your apps.").foregroundColor(Color("TextSecondary"))
                            Button("Retry") { Task { await load() } }
                                .fontWeight(.medium)
                                .foregroundColor(Color("TextPrimary"))
                                .underline()
                        }
                        .font(.system(size: 14))
                    }
                    if let mine {
                        if mine.isEmpty {
                            if !loadFailed {
                                Text("You haven’t submitted an app yet. Package one below and it shows up here.")
                                    .font(.system(size: 13))
                                    .foregroundColor(Color("TextSecondary"))
                            }
                        } else {
                            ForEach(mine) { record in
                                VStack(alignment: .leading, spacing: 8) {
                                    HStack(spacing: 12) {
                                        ACIAppLogo(name: record.submission.name)
                                        Text(record.submission.name)
                                            .font(.system(size: 15, weight: .semibold))
                                            .foregroundColor(Color("TextPrimary"))
                                            .lineLimit(1)
                                        Spacer()
                                        Text(Self.stateLabel(record.state))
                                            .font(.system(size: 12))
                                            .foregroundColor(Color("TextSecondary"))
                                    }
                                    if let reason = record.rejectionReason, !reason.isEmpty {
                                        Text(reason)
                                            .font(.system(size: 12))
                                            .foregroundColor(Color("TextSecondary"))
                                    }
                                }
                                .padding(16)
                                .background(Color("BgPanel"))
                                .clipShape(RoundedRectangle(cornerRadius: 12))
                                .overlay(RoundedRectangle(cornerRadius: 12).stroke(Color("BorderSubtle"), lineWidth: 1))
                            }
                        }
                    } else {
                        RoundedRectangle(cornerRadius: 12)
                            .fill(Color("FillNeutral"))
                            .frame(height: 112)
                            .accessibilityHidden(true)
                    }
                }
                (Text("New to building one? Read the ")
                 + Text("[MCP Apps docs](https://docs.allternit.com/mini-apps)").underline()
                 + Text("."))
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
                    .tint(Color("TextSecondary"))
            }
            .padding(.horizontal, 24)
            .padding(.vertical, 20)
        }
        .task { await load() }
    }

    private func load() async {
        loadFailed = false
        do {
            mine = try await model.submissions()
        } catch is CancellationError {
        } catch {
            mine = []
            loadFailed = true
        }
    }

    /// mini-apps-model.ts `pipelineProgress` labels.
    static func stateLabel(_ state: String) -> String {
        switch state {
        case "draft": return "Draft"
        case "in_review": return "In review"
        case "rejected": return "Rejected"
        case "approved": return "Approved"
        case "published": return "Published"
        default: return state.replacingOccurrences(of: "_", with: " ").capitalized
        }
    }
}
