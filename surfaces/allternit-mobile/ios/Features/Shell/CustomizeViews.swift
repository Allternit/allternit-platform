import SwiftUI

/// "Show in rail" choices per mode (allternit-ai src/shell/rail/rail-tabs.ts):
/// which of the mode's rail tabs the drawer shows. Same keys and tab ids as
/// the web (there in localStorage, here in UserDefaults), so the contract
/// reads the same; choices are per device on both.
@MainActor
final class RailTabsStore: ObservableObject {
    static let shared = RailTabsStore()

    enum TabMode: String {
        case home, code, browser

        var storageKey: String {
            switch self {
            case .home: return "allternit-home-rail-tabs"
            case .code: return "allternit-code-rail-tabs"
            case .browser: return "allternit-browser-rail-tabs"
            }
        }
    }

    struct Tab: Identifiable, Hashable {
        let id: String
        let label: String
        let icon: String
    }

    /// The tabs the phone drawer can show per mode (PhoneDrawer PHONE_TABS,
    /// limited to the rows iOS has).
    static func tabs(for mode: TabMode) -> [Tab] {
        switch mode {
        case .home:
            return [
                Tab(id: "projects", label: "Projects", icon: "folder"),
                Tab(id: "artifacts-library", label: "Artifacts", icon: "doc.text"),
                Tab(id: "automation-tasks", label: "Automation Tasks", icon: "clock"),
                Tab(id: "customize", label: "Catalog", icon: "slider.horizontal.3"),
            ]
        case .code:
            return [
                Tab(id: "projects", label: "Projects", icon: "folder"),
                Tab(id: "artifacts-library", label: "Artifacts", icon: "doc.text"),
                Tab(id: "code-automations", label: "Automation Tasks", icon: "clock"),
            ]
        case .browser:
            return [
                Tab(id: "mini-apps", label: "MCP Apps", icon: "macwindow"),
                Tab(id: "computers", label: "Computers", icon: "desktopcomputer"),
                Tab(id: "site-apis", label: "Teach", icon: "record.circle"),
            ]
        }
    }

    @Published private var saved: [TabMode: [String: Bool]] = [:]
    private let defaults: UserDefaults

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        for mode in [TabMode.home, .code, .browser] {
            if let data = defaults.string(forKey: mode.storageKey)?.data(using: .utf8),
               let decoded = try? JSONDecoder().decode([String: Bool].self, from: data) {
                saved[mode] = decoded
            }
        }
    }

    /// Default: every tab shows (rail-tabs DEFAULTS for the phone's tabs).
    func isVisible(_ id: String, in mode: TabMode) -> Bool {
        saved[mode]?[id] ?? true
    }

    func toggle(_ id: String, in mode: TabMode) {
        var map = saved[mode] ?? [:]
        map[id] = !isVisible(id, in: mode)
        saved[mode] = map
        if let data = try? JSONEncoder().encode(map), let json = String(data: data, encoding: .utf8) {
            defaults.set(json, forKey: mode.storageKey)
        }
    }

    func anyHidden(in mode: TabMode) -> Bool {
        Self.tabs(for: mode).contains { !isVisible($0.id, in: mode) }
    }
}

/// Catalog (allternit-ai src/views/catalog): everything the assistant can
/// use. Search, All / Connectors / Skills, an Interactive filter, then Your
/// connections, Most used, Trending, New and Categories. Rows open
/// `CatalogDetailView`. Plugins, MCP servers and webhooks stay under Manage.
/// (The type keeps its old name: Settings presents it.)
struct CustomizeSheet: View {
    @Environment(\.dismiss) private var dismiss
    @ObservedObject private var auth = AuthManager.shared

    private enum Kind: String, CaseIterable { case all = "All", connectors = "Connectors", skills = "Skills" }

    @State private var kind: Kind = .all
    @State private var interactiveOnly = false
    @State private var search = ""
    @State private var category: String?
    @State private var mostUsed: [CatalogEntry] = []
    @State private var trending: [CatalogEntry] = []
    @State private var fresh: [CatalogEntry] = []
    @State private var results: [CatalogEntry] = []
    @State private var yours: [Connector] = []
    @State private var isLoading = false
    @State private var loadError: String?
    private let connectorsClient = ConnectorsClient()

    private var groups: [String] {
        switch kind {
        case .all: return ["apps", "skills"]
        case .connectors: return ["apps"]
        case .skills: return ["skills"]
        }
    }
    private var isBrowsing: Bool {
        search.trimmingCharacters(in: .whitespaces).isEmpty && category == nil
    }
    private var connectedIds: Set<String> { Set(yours.filter(\.isConnected).map(\.id)) }
    private var categories: [String] {
        var seen = Set<String>(), out: [String] = []
        for e in mostUsed + trending + fresh { for c in e.categories where seen.insert(c).inserted { out.append(c) } }
        return out.sorted()
    }

    var body: some View {
        NavigationStack {
            DockingTitleList(title: "Catalog") {
                if !auth.isSignedIn && !auth.isSkippingAuth {
                    FriendlyStateView(style: .empty, icon: "person.crop.circle", title: "Sign in to browse the Catalog",
                                      message: "Skills, connectors and plugins belong to your account.",
                                      actionTitle: "Sign in", action: {
                                          dismiss()
                                          auth.isPresentingAuth = true
                                      })
                        .listRowBackground(Color.clear)
                } else {
                    controls
                    if isBrowsing { sections } else { resultsSection }
                    Section("Manage") {
                        NavigationLink { CapabilitiesSettingsView(kind: .skill) } label: { Label("Skills", image: "ph-book-open") }
                        NavigationLink { CapabilitiesSettingsView(kind: .plugin) } label: { Label("Plugins", image: "ph-puzzle-piece") }
                        NavigationLink { McpServersSettingsView() } label: { Label("MCP servers", image: "ph-cpu") }
                        NavigationLink { WebhooksSettingsView() } label: { Label("Webhooks", image: "ph-webhooks-logo") }
                    }
                }
            }
            .navigationTitle("Catalog")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } } }
            .task(id: "\(kind.rawValue)|\(interactiveOnly)") { await loadSections() }
            .task(id: "\(kind.rawValue)|\(interactiveOnly)|\(search)|\(category ?? "")") { await loadResults() }
        }
    }

    // MARK: Controls

    private var controls: some View {
        Section {
            HStack(spacing: 8) {
                Image(systemName: "magnifyingglass").foregroundColor(Color("TextSecondary"))
                TextField("Search", text: $search).textInputAutocapitalization(.never).autocorrectionDisabled()
                if !search.isEmpty {
                    Button { search = "" } label: { Image(systemName: "xmark.circle.fill") }
                        .buttonStyle(.plain).foregroundColor(Color("TextSecondary")).accessibilityLabel("Clear search")
                }
            }
            Picker("Show", selection: $kind) {
                ForEach(Kind.allCases, id: \.self) { Text($0.rawValue).tag($0) }
            }
            .pickerStyle(.segmented)
            Toggle("Interactive", isOn: $interactiveOnly)
            if let category {
                Button { self.category = nil } label: {
                    Label("\(category)  \u{2715}", systemImage: "line.3.horizontal.decrease")
                }
            }
        }
    }

    // MARK: Sections

    @ViewBuilder private var sections: some View {
        let mine = yours.filter(\.isConnected)
        if !mine.isEmpty {
            Section("Your connections") {
                ForEach(mine.prefix(5)) { c in
                    entryLink(CatalogEntry.stub(c), connected: true, subtitle: c.connection.account.map { "Connected \u{00B7} \($0)" } ?? "Connected")
                }
            }
        }
        listSection("Most used", mostUsed)
        listSection("Trending", trending)
        listSection("New", fresh)
        if !categories.isEmpty {
            Section("Categories") {
                ForEach(categories, id: \.self) { c in
                    Button { category = c } label: {
                        HStack { Text(c).foregroundColor(Color("TextPrimary")); Spacer()
                            Image(systemName: "chevron.right").font(.caption).foregroundColor(Color("TextSecondary")) }
                    }
                    .buttonStyle(.plain)
                }
            }
        }
        if isLoading && mostUsed.isEmpty {
            HStack { Spacer(); ProgressView(); Spacer() }.listRowBackground(Color.clear)
        } else if let loadError, mostUsed.isEmpty {
            FriendlyStateView(style: .error, icon: "wifi.exclamationmark", title: "Couldn't load the Catalog",
                              message: loadError, actionTitle: "Try again", action: { Task { await loadSections() } })
                .listRowBackground(Color.clear)
        }
    }

    @ViewBuilder private func listSection(_ title: String, _ entries: [CatalogEntry]) -> some View {
        if !entries.isEmpty {
            Section(title) { ForEach(entries) { entryLink($0, connected: connectedIds.contains($0.connectId)) } }
        }
    }

    @ViewBuilder private var resultsSection: some View {
        Section(category ?? "Results") {
            if results.isEmpty {
                Text(isLoading ? "Searching\u{2026}" : "Nothing matches.").foregroundColor(Color("TextSecondary"))
            } else {
                ForEach(results) { entryLink($0, connected: connectedIds.contains($0.connectId)) }
            }
        }
    }

    private func entryLink(_ e: CatalogEntry, connected: Bool, subtitle: String? = nil) -> some View {
        NavigationLink {
            CatalogDetailView(entry: e, isConnected: connected, onChanged: { Task { await loadYours() } })
        } label: {
            HStack(spacing: 12) {
                CatalogLogo(name: e.name, url: e.iconURL, size: 32)
                VStack(alignment: .leading, spacing: 2) {
                    HStack(spacing: 5) {
                        Text(e.name).font(.subheadline.weight(.semibold)).foregroundColor(Color("TextPrimary")).lineLimit(1)
                        if e.isVerified { Image(systemName: "checkmark.seal").font(.caption).foregroundColor(Color("TextSecondary")).accessibilityLabel("Verified") }
                        if e.interactive {
                            Text("Interactive").font(.caption2.weight(.medium)).foregroundColor(Color("TextSecondary"))
                                .padding(.horizontal, 6).padding(.vertical, 1)
                                .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
                        }
                    }
                    if !e.blurb.isEmpty {
                        Text(e.blurb).font(.caption).foregroundColor(Color("TextSecondary")).lineLimit(1)
                    }
                    Text(subtitle ?? e.byline).font(.caption2).foregroundColor(Color("TextSecondary")).lineLimit(1)
                }
            }
        }
    }

    // MARK: Loading

    @MainActor private func loadYours() async {
        if let all = try? await connectorsClient.listConnectors() { yours = all }
    }

    @MainActor private func loadSections() async {
        guard auth.isSignedIn || auth.isSkippingAuth else { return }
        isLoading = true
        loadError = nil
        await loadYours()
        func top(_ sort: CatalogSort) async throws -> [CatalogEntry] {
            var out: [CatalogEntry] = []
            for g in groups { out += try await CatalogClient.page(group: g, interactive: interactiveOnly, sort: sort, limit: 8).entries }
            return Array(out.prefix(8))
        }
        do {
            async let a = top(.popular), b = top(.trending), c = top(.new)
            (mostUsed, trending, fresh) = try await (a, b, c)
        } catch is CancellationError {
        } catch {
            loadError = ConnectionsClient.describe(error)
        }
        isLoading = false
    }

    @MainActor private func loadResults() async {
        guard !isBrowsing, auth.isSignedIn || auth.isSkippingAuth else { results = []; return }
        isLoading = true
        // Debounce typing: the task is cancelled when the text changes.
        try? await Task.sleep(nanoseconds: 250_000_000)
        guard !Task.isCancelled else { return }
        var out: [CatalogEntry] = []
        for g in groups {
            if let page = try? await CatalogClient.page(group: g, q: search, category: category, interactive: interactiveOnly, limit: 40) {
                out += page.entries
            }
        }
        guard !Task.isCancelled else { return }
        results = out
        isLoading = false
    }
}

private extension CatalogEntry {
    /// A connected connector the Catalog has not loaded yet: enough to show a row and open its page.
    static func stub(_ c: Connector) -> CatalogEntry {
        CatalogEntry(id: c.id, name: c.name, blurb: c.description ?? "")
    }
}
