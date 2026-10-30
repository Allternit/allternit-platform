import SwiftUI

/// ACI › MCP Apps, the web's `McpAppsView compact` (views/mini-apps/
/// McpAppsView.tsx): "MCP Apps" header, one compact bar (Installed ·
/// Discover · Build, search, Add by URL), and the tab's body.
///
/// Routes (allternit-api, through the app's API route):
/// - `GET  mcp-app-installs` / `PUT mcp-app-installs` / `DELETE mcp-app-installs/:appId`
/// - `GET  mcp/connectors` / `POST mcp/connectors` / `DELETE mcp/connectors/:id`
/// - `POST mcp/connectors/:id/oauth/start` → `{ authorize_url }`
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

    @StateObject private var model = ACIMcpAppsModel()
    @ObservedObject private var catalog = MiniAppCatalogStore.shared
    @Environment(\.openURL) private var openURL
    @State private var tab: Tab = .installed
    @State private var query = ""
    @State private var isAddOpen = false
    @State private var selected: ACIMcpAppsModel.InstalledApp? = nil

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
                if !next.isEmpty && tab == .build { go(.discover) }
            }
            if let notice = model.notice {
                HStack(spacing: 12) {
                    Text(notice)
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextPrimary"))
                    Spacer()
                    Button("Dismiss") { model.notice = nil }
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
        .task { await model.refresh() }
        .onAppear { catalog.fetchCatalogIfNeeded() }
        .sheet(isPresented: $isAddOpen) {
            ACIAddByURLSheet { url in
                let result = try await model.addByURL(url)
                if let authorize = result.authorizeURL { openURL(authorize) }
                model.notice = result.note
            }
        }
        .sheet(item: $selected) { app in
            ACIInstalledAppSheet(app: app, model: model)
        }
    }

    private func go(_ next: Tab) {
        tab = next
        model.notice = nil
    }

    // MARK: Installed

    private var q: String { query.trimmingCharacters(in: .whitespaces).lowercased() }

    @ViewBuilder
    private var installed: some View {
        if !model.loaded {
            Text("Loading…")
                .font(.system(size: 15))
                .foregroundColor(Color("TextSecondary"))
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            let shown = q.isEmpty ? model.apps : model.apps.filter { $0.name.lowercased().contains(q) }
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    if model.failed {
                        HStack(spacing: 8) {
                            Text("Couldn’t load apps.")
                                .foregroundColor(Color("TextSecondary"))
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
                                Button { go(.discover) } label: {
                                    Text("Browse apps")
                                        .font(.system(size: 14, weight: .medium))
                                        .foregroundColor(Color("OnAccent"))
                                        .padding(.horizontal, 16)
                                        .frame(height: 36)
                                        .background(Color("AccentPrimary"))
                                        .clipShape(RoundedRectangle(cornerRadius: 8))
                                }
                                .buttonStyle(.plain)
                            }
                        }
                        .frame(maxWidth: .infinity)
                        .padding(.horizontal, 24)
                        .padding(.vertical, 40)
                        .background(Color("BgPanel"))
                        .clipShape(RoundedRectangle(cornerRadius: 12))
                        .overlay(RoundedRectangle(cornerRadius: 12).stroke(Color("BorderSubtle"), lineWidth: 1))
                        .padding(.top, 16)
                    } else {
                        ForEach(shown) { app in installedCard(app) }
                    }
                }
                .padding(.horizontal, 24)
                .padding(.top, 20)
                .padding(.bottom, 40)
            }
            .refreshable { await model.refresh() }
        }
    }

    private func installedCard(_ app: ACIMcpAppsModel.InstalledApp) -> some View {
        Button { selected = app } label: {
            VStack(alignment: .leading, spacing: 10) {
                HStack(spacing: 12) {
                    ACIAppLogo(name: app.name)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(app.name)
                            .font(.system(size: 15, weight: .semibold))
                            .foregroundColor(Color("TextPrimary"))
                            .lineLimit(1)
                        if let host = app.host {
                            Text(host)
                                .font(.system(size: 12))
                                .foregroundColor(Color("TextTertiary"))
                                .lineLimit(1)
                        }
                    }
                    Spacer()
                }
                Rectangle().fill(Color("BorderSubtle")).frame(height: 1)
                HStack(spacing: 6) {
                    Circle()
                        .fill(app.connected ? Theme.statusSuccess : Theme.statusWarning)
                        .frame(width: 8, height: 8)
                    Text(app.connected ? "Connected" : "Needs sign-in")
                        .font(.system(size: 12))
                        .foregroundColor(Color("TextSecondary"))
                    Spacer()
                    if !app.connected {
                        Button("Reconnect") { Task { await reconnect(app) } }
                            .font(.system(size: 12, weight: .medium))
                            .foregroundColor(Color("TextPrimary"))
                            .padding(.horizontal, 10)
                            .frame(height: 28)
                            .overlay(RoundedRectangle(cornerRadius: 8).stroke(Color("BorderStrong"), lineWidth: 1))
                    }
                }
            }
            .padding(16)
            .background(Color("BgPanel"))
            .clipShape(RoundedRectangle(cornerRadius: 12))
            .overlay(RoundedRectangle(cornerRadius: 12).stroke(Color("BorderSubtle"), lineWidth: 1))
        }
        .buttonStyle(.plain)
    }

    private func reconnect(_ app: ACIMcpAppsModel.InstalledApp) async {
        model.notice = nil
        do {
            let url = try await model.startOAuth(connectorId: app.connectorId)
            openURL(url)
            model.notice = "Finish signing in to \(app.name) in the new window, then come back."
        } catch {
            model.notice = ACIMcpAppsModel.friendly(error, "Couldn’t start sign-in for \(app.name). Try again.")
        }
    }

    // MARK: Discover

    @ViewBuilder
    private var discover: some View {
        let items = q.isEmpty ? catalog.apps : catalog.apps.filter {
            $0.name.lowercased().contains(q) || $0.description.lowercased().contains(q)
        }
        if catalog.isLoading && catalog.apps.isEmpty {
            Text("Loading…")
                .font(.system(size: 15))
                .foregroundColor(Color("TextSecondary"))
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    if catalog.loadError != nil && catalog.apps.isEmpty {
                        HStack(spacing: 8) {
                            Text("Couldn’t load apps.").foregroundColor(Color("TextSecondary"))
                            Button("Retry") { catalog.fetchCatalogIfNeeded(force: true) }
                                .fontWeight(.medium)
                                .foregroundColor(Color("TextPrimary"))
                                .underline()
                        }
                        .font(.system(size: 14))
                    } else if items.isEmpty {
                        Text("No apps match “\(query.trimmingCharacters(in: .whitespaces))”.")
                            .font(.system(size: 14))
                            .foregroundColor(Color("TextSecondary"))
                    }
                    ForEach(items) { app in discoverCard(app) }
                }
                .padding(.horizontal, 24)
                .padding(.top, 20)
                .padding(.bottom, 40)
            }
            .refreshable { catalog.fetchCatalogIfNeeded(force: true) }
        }
    }

    private func discoverCard(_ app: MiniApp) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 12) {
                ACIAppLogo(name: app.name)
                VStack(alignment: .leading, spacing: 2) {
                    Text(app.name)
                        .font(.system(size: 15, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    Text(app.publisher ?? app.category.label)
                        .font(.system(size: 12))
                        .foregroundColor(Color("TextTertiary"))
                        .lineLimit(1)
                }
                Spacer()
                if app.verified {
                    Text("Official")
                        .font(.system(size: 10, weight: .semibold))
                        .foregroundColor(Color("TextTertiary"))
                        .padding(.horizontal, 8)
                        .padding(.vertical, 4)
                        .background(Color("FillNeutral"))
                        .clipShape(Capsule())
                }
            }
            Text(app.description)
                .font(.system(size: 13))
                .lineSpacing(3)
                .foregroundColor(Color("TextSecondary"))
                .lineLimit(3)
            Button { Task { await connect(app) } } label: {
                Text("Connect")
                    .font(.system(size: 14, weight: .medium))
                    .foregroundColor(Color("OnAccent"))
                    .frame(maxWidth: .infinity, minHeight: 36)
                    .background(Color("AccentPrimary"))
                    .clipShape(RoundedRectangle(cornerRadius: 8))
            }
            .buttonStyle(.plain)
            .disabled(app.url.isEmpty)
        }
        .padding(16)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(Color("BorderSubtle"), lineWidth: 1))
    }

    private func connect(_ app: MiniApp) async {
        do {
            let result = try await model.installServer(appId: app.id, name: app.name, serverURL: app.url)
            if let authorize = result.authorizeURL { openURL(authorize) }
            model.notice = result.note
        } catch {
            model.notice = ACIMcpAppsModel.friendly(error, "Couldn’t add \(app.name). Try again.")
        }
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

        var id: String { appId }
        var host: String? { url.flatMap { URL(string: $0)?.host } }
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
    @Published private(set) var failed = false
    @Published var notice: String? = nil

    private struct InstallsEnvelope: Decodable { let items: [InstallRecord]? }
    private struct ConnectorsEnvelope: Decodable { let connectors: [Connector]? }
    private struct SubmissionsEnvelope: Decodable { let items: [Submission]? }
    private struct Created: Decodable { let id: String }
    private struct OAuthStart: Decodable { let authorize_url: String? }
    private struct EmptyBody: Encodable {}
    private struct CreateConnector: Encodable { let name: String; let name_id: String; let url: String; let type: String }
    private struct PutInstall: Encodable { let appId: String; let connectorId: String }

    /// mini-apps-model.ts `REAUTH_STATES`-style statuses that need a new sign-in.
    private static let reauthStates: Set<String> = ["expired", "error", "revoked", "pending", "needs_reauth"]

    func refresh() async {
        async let installsResult: InstallsEnvelope = APIClient.shared.get(path: "mcp-app-installs")
        async let connectorsResult: ConnectorsEnvelope = APIClient.shared.get(path: "mcp/connectors")
        let installs = try? await installsResult
        let connectors = try? await connectorsResult
        failed = installs == nil || connectors == nil
        let byId = Dictionary((connectors?.connectors ?? []).map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        apps = (installs?.items ?? []).map { record in
            let connector = byId[record.connectorId]
            return InstalledApp(
                appId: record.appId,
                connectorId: record.connectorId,
                name: connector?.name ?? record.appId.replacingOccurrences(of: #"^(mcp:|url:)"#, with: "", options: .regularExpression),
                url: connector?.url,
                connected: Self.isConnected(connector)
            )
        }
        loaded = true
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

/// AppLogo fallback: the name's first letter on a neutral tile.
struct ACIAppLogo: View {
    let name: String

    var body: some View {
        Text(String(name.prefix(1)).uppercased())
            .font(.system(size: 16, weight: .semibold))
            .foregroundColor(Color("TextSecondary"))
            .frame(width: 40, height: 40)
            .background(Color("FillNeutral"))
            .clipShape(RoundedRectangle(cornerRadius: 10))
            .overlay(RoundedRectangle(cornerRadius: 10).stroke(Color("BorderSubtle"), lineWidth: 1))
            .accessibilityHidden(true)
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

/// The installed app's detail (DetailModal + AppDrawerBody): status, where
/// it comes from, and Remove.
private struct ACIInstalledAppSheet: View {
    let app: ACIMcpAppsModel.InstalledApp
    @ObservedObject var model: ACIMcpAppsModel
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(spacing: 12) {
                ACIAppLogo(name: app.name)
                VStack(alignment: .leading, spacing: 2) {
                    Text(app.name)
                        .font(.system(size: 17, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                    if let host = app.host {
                        Text(host)
                            .font(.system(size: 13))
                            .foregroundColor(Color("TextTertiary"))
                    }
                }
            }
            HStack(spacing: 8) {
                Circle()
                    .fill(app.connected ? Theme.statusSuccess : Theme.statusWarning)
                    .frame(width: 8, height: 8)
                Text(app.connected ? "Connected" : "Sign-in expired")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
            }
            Spacer(minLength: 0)
            HStack(spacing: 8) {
                Button("Remove") {
                    Task {
                        await model.remove(app)
                        dismiss()
                    }
                }
                .font(.system(size: 14, weight: .medium))
                .foregroundColor(Color("TextSecondary"))
                .padding(.horizontal, 14)
                .frame(height: 40)
                Spacer()
                Button { dismiss() } label: {
                    Text("Done")
                        .font(.system(size: 14, weight: .medium))
                        .foregroundColor(Color("OnAccent"))
                        .padding(.horizontal, 18)
                        .frame(height: 40)
                        .background(Color("AccentPrimary"))
                        .clipShape(RoundedRectangle(cornerRadius: 8))
                }
                .buttonStyle(.plain)
            }
        }
        .padding(20)
        .background(Color("BgPanel").ignoresSafeArea())
        .presentationDetents([.medium])
        .presentationDragIndicator(.visible)
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
