import SwiftUI
import UIKit

/// Customize as the phone opens it from the drawer (allternit-ai
/// src/views/customize/CustomizeView.tsx, `initialSection="skills"`): a
/// plain ☰ bar, the serif "Customize" title and its line, search · filter ·
/// sort · "+ Add", the Skills · Connectors · Plugins · More tabs, Yours |
/// Discover, then either the installed rows (with switches) or the Discover
/// landing (featured item, "For you", "More …").
///
/// Data: skills and plugins on the user's computer (CapabilitiesClient),
/// connectors from `/api/v1/connectors` (ConnectorsClient), drawn with the
/// connector directory MCP Apps uses. More opens the native MCP servers and
/// Webhooks screens. Glyphs and logos are Desktop's own artwork.
struct CustomizeScreen: View {
    @Binding var isSidebarOpen: Bool

    @ObservedObject private var auth = AuthManager.shared
    @State private var section: CustomizeSection = .skills
    @State private var scope: Scope? = nil
    @State private var query = ""
    @State private var category: String? = nil
    @State private var sort: Sort = .recommended
    @State private var items: [CustomizeSection: [CustomizeItem]] = [:]
    @State private var loading: Set<CustomizeSection> = []
    @State private var failures: [CustomizeSection: SettingsLoadFailure] = [:]
    @State private var pending: Set<String> = []
    @State private var actionError: String? = nil
    @State private var path = NavigationPath()
    @State private var isAddingSource = false
    @EnvironmentObject private var modeStore: AppModeStore
    @State private var detail: InstalledCapability? = nil
    @State private var isCreatingSkill = false
    @State private var isConnectorsPresented = false
    /// Skill directory ids installed on the computer (id → folder name).
    @State private var installedCatalog: [String: String] = [:]
    @State private var showAll = false
    /// A built-in plugin's page (web ItemDetail).
    @State private var featureDetail: CustomizeItem? = nil
    @StateObject private var connect = DirectoryConnectModel()
    /// A connector's page.
    @State private var connectorDetail: DirectoryEntry? = nil
    /// A curated collection opened from the connectors hero.
    @State private var collection: String? = nil

    enum CustomizeSection: String, CaseIterable {
        case skills, connectors, plugins

        var label: String {
            switch self {
            case .skills: return "Skills"
            case .connectors: return "Connectors"
            case .plugins: return "Plugins"
            }
        }

        /// SECTIONS: BookOpen / PlugsConnected / PuzzlePiece.
        var icon: String {
            switch self {
            case .skills: return "book-open"
            case .connectors: return "plugs-connected"
            case .plugins: return "puzzle-piece"
            }
        }

        var singular: String { String(label.lowercased().dropLast()) }
    }

    /// The "More" group sections iOS has (MCP servers, Webhooks).
    enum MoreSection: String, CaseIterable, Hashable {
        case mcps, webhooks

        var label: String { self == .mcps ? "MCP servers" : "Webhooks" }
        var icon: String { self == .mcps ? "ph-cpu" : "ph-webhooks-logo" }
    }

    /// The "Build for the Allternit directory" pages (web Sources, Publish).
    enum DirectoryPage: Hashable { case sources, publish }

    enum Scope { case yours, discover }
    enum Sort { case recommended, name }

    private var all: [CustomizeItem] { items[section] ?? [] }
    private var enabled: [CustomizeItem] { all.filter(\.enabled) }

    /// Discover = local items that are off, plus (Skills) the skill directory
    /// entries not on the computer yet (the web's useDiscoverExtras).
    private var discoverable: [CustomizeItem] {
        let local = all.filter { !$0.enabled }
        guard section == .skills else { return local }
        let localIds = Set(all.map(\.id))
        let extras = SkillCatalog.entries
            .filter { !localIds.contains($0.id) && installedCatalog[$0.id] == nil }
            .map(CustomizeItem.init(catalog:))
        return local + extras
    }

    /// Claude opens on what you have; with nothing on yet, Discover is the page.
    private var effectiveScope: Scope { scope ?? (enabled.isEmpty ? .discover : .yours) }

    private var pool: [CustomizeItem] { effectiveScope == .yours ? enabled : discoverable }

    private var categories: [String] {
        Array(Set(pool.compactMap(\.category))).sorted()
    }

    private var filtered: [CustomizeItem] {
        let q = query.trimmingCharacters(in: .whitespaces).lowercased()
        var list = pool.filter { item in
            (category == nil || item.category == category)
                && (q.isEmpty || "\(item.name) \(item.description)".lowercased().contains(q))
        }
        if sort == .name { list.sort { $0.name.localizedCompare($1.name) == .orderedAscending } }
        return list
    }

    var body: some View {
        NavigationStack(path: $path) {
            VStack(spacing: 0) {
                LibraryPlainHeader(onMenu: { openLibraryDrawer($isSidebarOpen) })
                if !auth.isSignedIn && !auth.isSkippingAuth {
                    FriendlyStateView(style: .empty, icon: "person.crop.circle", title: "Sign in to customize",
                                      message: "Skills, connectors and plugins belong to your account.",
                                      actionTitle: "Sign in", action: { auth.isPresentingAuth = true })
                        .frame(maxHeight: .infinity)
                } else {
                    ScrollView {
                        VStack(alignment: .leading, spacing: 0) {
                            header
                            Rectangle().fill(Color("BorderSubtle")).frame(height: 1).padding(.horizontal, -24)
                            if let note = connect.note {
                                HStack(spacing: 12) {
                                    Text(note)
                                        .font(.system(size: 13))
                                        .foregroundColor(Color("TextPrimary"))
                                        .frame(maxWidth: .infinity, alignment: .leading)
                                    Button("Dismiss") { connect.note = nil }
                                        .font(.system(size: 13))
                                        .foregroundColor(Color("TextSecondary"))
                                        .underline()
                                }
                                .padding(.vertical, 8)
                            }
                            landing.padding(.top, 20)
                        }
                        .padding(.horizontal, 24)
                        .padding(.bottom, 96)
                    }
                    .scrollDismissesKeyboard(.interactively)
                    .refreshable { await load(section, force: true) }
                }
            }
            .background(Color("BgPrimary").ignoresSafeArea())
            .toolbar(.hidden, for: .navigationBar)
            .navigationDestination(for: DirectoryPage.self) { page in
                switch page {
                case .sources:
                    CustomizeSourcesView()
                case .publish:
                    CustomizePublishView(
                        onStart: {
                            // The web opens the plugin manager on Plugins;
                            // here that is the Plugins tab.
                            path = NavigationPath()
                            select(.plugins)
                        },
                        onPublished: { modeStore.selectBarItem(.artifacts) }
                    )
                }
            }
            .navigationDestination(for: MoreSection.self) { more in
                switch more {
                case .mcps: McpServersSettingsView()
                case .webhooks: WebhooksSettingsView()
                }
            }
            .navigationDestination(isPresented: Binding(get: { detail != nil }, set: { if !$0 { detail = nil } })) {
                if let item = detail {
                    capabilityDetail(item)
                }
            }
        }
        .sheet(isPresented: $isCreatingSkill) {
            CreateSkillSheet { created in
                var list = items[.skills] ?? []
                list.removeAll { $0.id == created.id }
                list.append(CustomizeItem(capability: created))
                items[.skills] = list.sorted { $0.name.localizedCompare($1.name) == .orderedAscending }
            }
        }
        .sheet(isPresented: $isConnectorsPresented, onDismiss: { Task { await load(.connectors, force: true) } }) {
            ConnectorsListView()
        }
        .sheet(isPresented: $isAddingSource) {
            AddSourceSheet()
        }
        .sheet(item: $featureDetail) { item in
            FeaturePluginDetailView(item: item, enabled: Binding(
                get: { items[.plugins]?.first { $0.id == item.id }?.enabled ?? item.enabled },
                set: { value in Task { await setEnabled(item, value) } }
            ))
            .presentationDetents([.medium, .large])
        }
        .modifier(DirectoryFlow(selected: $connectorDetail, model: connect))
        .task(id: section) { await load(section, force: false) }
        .task { connect.onChanged = { await load(.connectors, force: true) } }
        #if DEBUG
        // `-customize-section <skills|connectors|plugins>` (DEBUG only): land on a section.
        .onAppear {
            if let raw = UserDefaults.standard.string(forKey: "customize-section"), let pick = CustomizeSection(rawValue: raw) {
                section = pick
            }
            if launchArgumentEnabled("customize-discover") { scope = .discover }
        }
        #endif
    }

    private func capabilityDetail(_ item: InstalledCapability) -> some View {
        CapabilityDetailView(item: item, onChanged: { updated in
            var list = items[section] ?? []
            if let updated {
                if let index = list.firstIndex(where: { $0.id == updated.id }) {
                    list[index] = CustomizeItem(capability: updated)
                }
            } else {
                list.removeAll { $0.id == item.id }
            }
            items[section] = list
        })
    }

    // MARK: - Header

    private var header: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("Customize")
                .font(.system(size: 30, weight: .medium, design: .serif))
                .tracking(-0.3)
                .foregroundColor(Color("TextPrimary"))
                .padding(.top, 16)
            Text("Skills, connectors, and plugins that shape what the agent can do.")
                .font(.system(size: 15))
                .foregroundColor(Color("TextSecondary"))
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, 6)

            HStack(spacing: 8) {
                searchField
                filterButton
                sortButton
                addButton
            }
            .padding(.top, 16)

            HStack(spacing: 4) {
                ForEach(CustomizeSection.allCases, id: \.self) { tab in
                    tabButton(tab.label, active: tab == section) { select(tab) }
                }
                Menu {
                    ForEach(MoreSection.allCases, id: \.self) { more in
                        Button { path.append(more) } label: { Label(more.label, image: more.icon) }
                    }
                } label: {
                    HStack(spacing: 4) {
                        Text("More")
                        PhIcon("caret-down", size: 11)
                    }
                    .font(.system(size: 14))
                    .foregroundColor(Color("TextSecondary"))
                    .padding(.horizontal, 12)
                    .padding(.top, 4)
                    .padding(.bottom, 12)
                }
            }
            .padding(.top, 16)
            .padding(.horizontal, -12)

            scopeToggle
                .padding(.top, 4)
                .padding(.bottom, 12)
        }
    }

    private var searchField: some View {
        HStack(spacing: 8) {
            PhIcon("magnifying-glass", size: 16)
                .foregroundColor(Color("TextTertiary"))
            TextField("Search \(section.label.lowercased())", text: $query)
                .font(.system(size: 13))
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
        }
        .padding(.horizontal, 12)
        .frame(height: 36)
        .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color("BgPanel")))
        .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
    }

    private func iconLabel(_ icon: String, active: Bool) -> some View {
        PhIcon(icon, size: 16)
            .foregroundColor(active ? Color("TextPrimary") : Color("TextSecondary"))
            .frame(width: 36, height: 36)
            .background(RoundedRectangle(cornerRadius: 8).fill(active ? Color("FillNeutral") : Color.clear))
    }

    private var filterButton: some View {
        Menu {
            LibraryMenuCheck(title: "All categories", checked: category == nil) { category = nil }
            ForEach(categories, id: \.self) { c in
                LibraryMenuCheck(title: c.capitalized, checked: category == c) { category = c }
            }
        } label: {
            iconLabel("sliders-horizontal", active: category != nil)
        }
        .accessibilityLabel("Filter by category")
    }

    private var sortButton: some View {
        Menu {
            LibraryMenuCheck(title: "Recommended", checked: sort == .recommended) { sort = .recommended }
            LibraryMenuCheck(title: "Name", checked: sort == .name) { sort = .name }
        } label: {
            iconLabel("arrows-down-up", active: false)
        }
        .accessibilityLabel("Sort")
    }

    /// ADD_ACTIONS, the ones iOS can do.
    private var addButton: some View {
        Menu {
            switch section {
            case .skills:
                Button { isCreatingSkill = true } label: { Label("Create skill", image: "ph-plus") }
            case .connectors:
                Button { isConnectorsPresented = true } label: { Label("Browse connectors", image: "ph-plus") }
            case .plugins:
                Button { isAddingSource = true } label: { Label("Add marketplace", image: "ph-plus") }
            }
        } label: {
            HStack(spacing: 6) {
                PhIcon("plus-bold", size: 14)
                Text("Add").font(.system(size: 13, weight: .medium))
                PhIcon("caret-down", size: 11)
            }
            .foregroundColor(Color("BgPrimary"))
            .padding(.horizontal, 12)
            .frame(height: 36)
            .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color("TextPrimary")))
            .fixedSize()
        }
    }

    private func tabButton(_ title: String, active: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title)
                .font(.system(size: 14, weight: active ? .medium : .regular))
                .foregroundColor(active ? Color("TextPrimary") : Color("TextSecondary"))
                .padding(.horizontal, 12)
                .padding(.top, 4)
                .padding(.bottom, 10)
                .overlay(alignment: .bottom) {
                    Rectangle()
                        .fill(active ? Color("TextPrimary") : Color.clear)
                        .frame(height: 2)
                }
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(active ? .isSelected : [])
    }

    private var scopeToggle: some View {
        HStack(spacing: 2) {
            ForEach([Scope.yours, .discover], id: \.self) { option in
                let active = option == effectiveScope
                Button(action: { scope = option; showAll = false; collection = nil }) {
                    Text(option == .yours ? "Yours" : "Discover")
                        .font(.system(size: 14, weight: active ? .medium : .regular))
                        .foregroundColor(active ? Color("TextPrimary") : Color("TextTertiary"))
                        .padding(.horizontal, 12)
                        .frame(height: 30)
                        .background(
                            RoundedRectangle(cornerRadius: 7, style: .continuous)
                                .fill(active ? Color("BgPanel") : Color.clear)
                                .shadow(color: .black.opacity(active ? 0.06 : 0), radius: 1, y: 1)
                        )
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(active ? .isSelected : [])
            }
        }
        .padding(3)
        .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color("FillNeutral")))
        .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
    }

    private func select(_ next: CustomizeSection) {
        guard next != section else { return }
        section = next
        scope = nil
        showAll = false
        collection = nil
        query = ""
        category = nil
    }

    // MARK: - Landing

    @ViewBuilder
    private var landing: some View {
        if let actionError {
            Text(actionError)
                .font(.system(size: 13))
                .foregroundColor(Theme.statusError)
                .padding(.bottom, 12)
        }
        if loading.contains(section) && all.isEmpty {
            PolishCardSkeletons(count: 6)
        } else if let failure = failures[section], all.isEmpty, effectiveScope == .yours || discoverable.isEmpty {
            SettingsFailureView(failure: failure, subject: "your \(section.label.lowercased())") {
                Task { await load(section, force: true) }
            }
        } else if section == .connectors && effectiveScope == .discover && collection != nil {
            collectionPage
        } else if section == .connectors && effectiveScope == .discover && query.isEmpty && category == nil && !showAll {
            connectorsLanding
        } else if filtered.isEmpty {
            emptyState
        } else if effectiveScope == .yours {
            VStack(alignment: .leading, spacing: 0) {
                Text(section == .connectors ? "Connected" : "Installed")
                    .font(.system(size: 13, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                    .padding(.bottom, 4)
                ForEach(Array(filtered.enumerated()), id: \.element.id) { index, item in
                    if index > 0 { Rectangle().fill(Color("BorderSubtle")).frame(height: 1) }
                    if section == .connectors, item.connector != nil {
                        connectedRow(item)
                    } else {
                        installedRow(item)
                    }
                }
            }
        } else if section == .connectors {
            VStack(alignment: .leading, spacing: 12) {
                if showAll || category != nil {
                    DirectoryBackLink(label: "Customize") { showAll = false; category = nil }
                }
                DirectoryGrid(list: Array(AppDirectory.ranked(entries(filtered)).prefix(120)), busy: connect.busy,
                              onOpen: { connectorDetail = $0 }, onAdd: { entry in Task { await connect.connect(entry) } })
            }
        } else if query.isEmpty && category == nil && !showAll {
            discoverLanding(filtered)
        } else {
            cardGrid(filtered)
        }
    }

    // MARK: - Connectors

    private func entries(_ list: [CustomizeItem]) -> [DirectoryEntry] {
        list.compactMap { $0.connector.map(DirectoryEntry.init(connector:)) }
    }

    /// ConnectorsLanding, the directory MCP Apps › Discover shows too.
    private var connectorsLanding: some View {
        let pool = entries(all)
        return DirectoryLanding(
            list: pool.filter { !$0.connected }, pool: pool, total: pool.count, noun: "connectors", busy: connect.busy,
            onOpen: { connectorDetail = $0 },
            onAdd: { entry in Task { await connect.connect(entry) } },
            onCategory: { category = $0 },
            onShowAll: { showAll = true },
            onCollection: { collection = $0 }
        )
    }

    /// ConnectorListPage for a curated collection.
    private var collectionPage: some View {
        let picked = AppDirectory.collections.first { $0.id == collection }
        let byKey = Dictionary(entries(all).filter { !$0.connected }.map { (AppDirectory.rankKey($0.id), $0) },
                               uniquingKeysWith: { first, _ in first })
        return VStack(alignment: .leading, spacing: 12) {
            DirectoryBackLink(label: "Customize") { collection = nil }
            Text(picked?.title ?? "Connectors")
                .font(.system(size: 24, weight: .medium, design: .serif))
                .foregroundColor(Color("TextPrimary"))
            DirectoryGrid(list: (picked?.ids ?? []).compactMap { byKey[$0] }, busy: connect.busy,
                          onOpen: { connectorDetail = $0 }, onAdd: { entry in Task { await connect.connect(entry) } })
        }
    }

    /// ConnectedRow: the real logo, name, "Connected as …", ⋯.
    private func connectedRow(_ item: CustomizeItem) -> some View {
        let entry = item.connector.map(DirectoryEntry.init(connector:))
        return HStack(spacing: 12) {
            AppLogoView(id: item.id, name: item.name, url: entry?.site, size: 36)
            Button { if let entry { connectorDetail = entry } } label: {
                VStack(alignment: .leading, spacing: 2) {
                    Text(item.name)
                        .font(.system(size: 14))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    Text(entry?.account.map { "Connected as \($0)" } ?? item.description)
                        .font(.system(size: 12))
                        .foregroundColor(Color("TextTertiary"))
                        .lineLimit(1)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            Menu {
                Button { UIPasteboard.general.string = item.id } label: { Label("Copy ID", image: "ph-copy") }
                Button { Task { await setEnabled(item, false) } } label: { Label("Disconnect", image: "ph-prohibit") }
            } label: {
                PhIcon("dots-three-vertical", size: 17)
                    .foregroundColor(Color("TextSecondary"))
                    .frame(width: 44, height: 44)
                    .contentShape(Rectangle())
            }
            .accessibilityLabel("More actions for \(item.name)")
        }
        .padding(.vertical, 12)
    }

    private var emptyState: some View {
        let label = section.label.lowercased()
        let q = query.trimmingCharacters(in: .whitespaces)
        return VStack(spacing: 6) {
            Text(!q.isEmpty
                 ? "No \(label) match \"\(q)\"."
                 : effectiveScope == .yours ? "No \(label) turned on yet." : "Everything available in \(label) is already on.")
            if effectiveScope == .yours && q.isEmpty {
                Button("Discover \(label)") { scope = .discover }
                    .foregroundColor(Color("TextPrimary"))
                    .underline()
            }
        }
        .font(.system(size: 13))
        .foregroundColor(Color("TextSecondary"))
        .multilineTextAlignment(.center)
        .frame(maxWidth: .infinity)
        .padding(.horizontal, 24)
        .padding(.vertical, 40)
        .overlay(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .strokeBorder(Color("BorderSubtle"), style: StrokeStyle(lineWidth: 1, dash: [4, 3]))
        )
    }

    @ViewBuilder
    private func discoverLanding(_ list: [CustomizeItem]) -> some View {
        // Official Allternit items lead the hero (added or not); otherwise the
        // featured item: an Allternit-authored one when there is one, else the first.
        let official = (enabled + discoverable).filter { $0.author?.caseInsensitiveCompare("Allternit") == .orderedSame }
        let featured = list.first { $0.author?.localizedCaseInsensitiveContains("allternit") == true } ?? list[0]
        let heroItems = official.isEmpty ? [featured] : official
        let rest = official.isEmpty ? list.filter { $0.id != featured.id } : list
        CustomizeSectionHero(
            lockup: section.label,
            items: heroItems,
            eyebrow: { item in official.isEmpty ? (item.author.map { "From \($0)" } ?? "Featured \(section.singular)") : "Official \(section.singular)" },
            actions: { item, palette in heroActions(item, palette: palette) }
        )
        .padding(.bottom, 32)
        if !rest.isEmpty {
            landingSection("For you") { cardGrid(Array(rest.prefix(6))) }
        }
        if rest.count > 6 {
            landingSection("More \(section.label.lowercased())", onShowAll: { showAll = true }) {
                cardGrid(Array(rest.dropFirst(6).prefix(6)))
            }
        }
        let counts = Dictionary(grouping: list.compactMap(\.category), by: { $0 }).mapValues { $0.count }
        let keys = counts.keys.sorted { counts[$0]! != counts[$1]! ? counts[$0]! > counts[$1]! : $0 < $1 }
        if !keys.isEmpty {
            landingSection("Categories") {
                VStack(spacing: 10) {
                    ForEach(keys, id: \.self) { key in
                        Button { category = key } label: {
                            HStack(spacing: 12) {
                                PhIcon(section.icon, size: 17)
                                    .foregroundColor(Color("TextSecondary"))
                                    .frame(width: 36, height: 36)
                                    .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
                                Text(Self.titleCase(key))
                                    .font(.system(size: 13))
                                    .foregroundColor(Color("TextPrimary"))
                                    .frame(maxWidth: .infinity, alignment: .leading)
                                Text("\(counts[key] ?? 0)")
                                    .font(.system(size: 12))
                                    .foregroundColor(Color("TextTertiary"))
                            }
                            .padding(12)
                            .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("BgPanel")))
                            .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
        }
        directoryCard
    }

    /// Discover's last card: Sources and Publish (web CustomizeView).
    private var directoryCard: some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Build for the Allternit directory")
                    .font(.system(size: 13, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                Text("List your connector, skill, or plugin and reach everyone using Allternit.")
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextSecondary"))
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            VStack(spacing: 6) {
                Button { path.append(DirectoryPage.sources) } label: {
                    Text("Sources")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextPrimary"))
                        .padding(.horizontal, 12)
                        .frame(height: 32)
                        .background(RoundedRectangle(cornerRadius: 8).fill(Color("BgPanel")))
                        .overlay(RoundedRectangle(cornerRadius: 8).stroke(Color("BorderSubtle"), lineWidth: 1))
                }
                Button { path.append(DirectoryPage.publish) } label: {
                    Text("Publish")
                        .font(.system(size: 13))
                        .foregroundColor(Color("BgPrimary"))
                        .padding(.horizontal, 12)
                        .frame(height: 32)
                        .background(RoundedRectangle(cornerRadius: 8).fill(Color("TextPrimary")))
                }
            }
            .buttonStyle(.plain)
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 16)
        .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("FillNeutral")))
        .padding(.top, 32)
    }

    static func titleCase(_ value: String) -> String {
        value.replacingOccurrences(of: "-", with: " ").replacingOccurrences(of: "_", with: " ").capitalized
    }

    private func landingSection<Content: View>(_ title: String, onShowAll: (() -> Void)? = nil, @ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text(title)
                    .font(.system(size: 14, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                Spacer()
                if let onShowAll {
                    Button(action: onShowAll) {
                        HStack(spacing: 4) {
                            Text("Show all")
                            PhIcon("arrow-right", size: 13)
                        }
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextPrimary"))
                    }
                    .buttonStyle(.plain)
                }
            }
            content()
        }
        .padding(.bottom, 36)
    }

    /// The hero's Add (or Added) and View buttons.
    @ViewBuilder
    private func heroActions(_ item: CustomizeItem, palette: HeroPalette) -> some View {
        let busy = pending.contains(item.id)
        Button(action: { add(item) }) {
            HeroButtonLabel(title: busy ? "Adding…" : item.enabled ? "Added" : "Add",
                            icon: item.enabled ? "check-bold" : "plus-bold", busy: busy, palette: palette)
        }
        .buttonStyle(.plain)
        .disabled(item.enabled || busy)
        if item.capability != nil {
            Button { open(item) } label: {
                HeroButtonLabel(title: "View \(section.singular)", solid: false, palette: palette)
            }
            .buttonStyle(.plain)
        }
    }

    private func cardGrid(_ list: [CustomizeItem]) -> some View {
        PolishCardGrid {
            ForEach(list) { itemCard($0) }
        }
    }

    /// ItemCard (PreviewCard): glyph tile with a round + / ✓, status dot,
    /// name, description, "by author · Category".
    private func itemCard(_ item: CustomizeItem) -> some View {
        let isPending = pending.contains(item.id)
        let meta = [item.author.map { "by \($0)" }, item.category.map(Self.titleCase)].compactMap { $0 }.joined(separator: " · ")
        return PolishCard(
            name: item.name,
            description: item.description.isEmpty ? "No description." : item.description,
            status: isPending ? .attention : item.enabled ? .ok : .idle,
            statusLabel: isPending ? "Adding" : item.enabled ? "Added" : "Not added",
            footer: meta.isEmpty ? nil : meta,
            onOpen: { open(item) },
            logo: { PhIcon(section.icon, size: 36).foregroundColor(Color("TextSecondary")) },
            action: {
                PolishCardAddButton(added: item.enabled, pending: isPending,
                                    label: item.enabled ? "\(item.name) added" : isPending ? "Adding \(item.name)" : "Add \(item.name)",
                                    action: { add(item) })
            }
        )
    }

    /// InstalledRow: glyph, name, "from author · description", ⋯, switch.
    private func installedRow(_ item: CustomizeItem) -> some View {
        HStack(spacing: 12) {
            PhIcon(section.icon, size: 16)
                .foregroundColor(Color("TextSecondary"))
                .frame(width: 36, height: 36)
                .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
            Button(action: { open(item) }) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(item.name)
                        .font(.system(size: 14))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    Text([item.author.map { "from \($0)" }, item.description.isEmpty ? nil : item.description]
                        .compactMap { $0 }.joined(separator: " · "))
                        .font(.system(size: 12))
                        .foregroundColor(Color("TextTertiary"))
                        .lineLimit(1)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            Menu {
                if item.capability != nil {
                    Button { open(item) } label: { Label("Edit in manager", image: "ph-arrow-square-out") }
                }
                Button { UIPasteboard.general.string = item.id } label: { Label("Copy ID", image: "ph-copy") }
                Button { Task { await setEnabled(item, false) } } label: { Label("Turn off", image: "ph-prohibit") }
            } label: {
                PhIcon("dots-three-vertical", size: 17)
                    .foregroundColor(Color("TextSecondary"))
                    .frame(width: 44, height: 44)
                    .contentShape(Rectangle())
            }
            .accessibilityLabel("More actions for \(item.name)")
            Toggle("", isOn: Binding(get: { item.enabled }, set: { value in Task { await setEnabled(item, value) } }))
                .labelsHidden()
                .tint(Color(hex: "#3B82F6"))
                .disabled(pending.contains(item.id))
                .accessibilityLabel(item.enabled ? "Turn off \(item.name)" : "Turn on \(item.name)")
        }
        .padding(.vertical, 12)
    }

    // MARK: - Actions

    private func open(_ item: CustomizeItem) {
        if item.feature != nil {
            featureDetail = item
        } else if let capability = item.capability {
            detail = capability
        } else if item.catalog != nil {
            add(item)
        } else if let connector = item.connector {
            connectorDetail = DirectoryEntry(connector: connector)
        } else {
            isConnectorsPresented = true
        }
    }

    private func add(_ item: CustomizeItem) {
        if item.catalog != nil {
            Task { await installCatalog(item) }
        } else if let connector = item.connector {
            Task { await connect.connect(DirectoryEntry(connector: connector)) }
        } else {
            Task { await setEnabled(item, true) }
        }
    }

    @MainActor
    private func setEnabled(_ item: CustomizeItem, _ value: Bool) async {
        actionError = nil
        pending.insert(item.id)
        defer { pending.remove(item.id) }
        do {
            if item.feature != nil {
                FeaturePlugins.setEnabled(item.id, value)
            } else if let capability = item.capability {
                try await CapabilitiesClient.shared.setEnabled(capability, enabled: value)
            } else if item.connector != nil {
                guard !value else {
                    if let connector = item.connector { await connect.connect(DirectoryEntry(connector: connector)) }
                    return
                }
                try await ConnectorsClient().disconnect(id: item.id)
            }
            var list = items[section] ?? []
            if let index = list.firstIndex(where: { $0.id == item.id }) {
                list[index].enabled = value
                list[index].capability?.enabled = value
            }
            items[section] = list
        } catch {
            actionError = "Couldn't turn \(item.name) \(value ? "on" : "off"). \(FriendlyErrorMessage.from(error.localizedDescription))"
        }
    }

    /// One-click install from the skill directory (installCatalogSkill), then
    /// a rescan so the new folder lists under Yours.
    @MainActor
    private func installCatalog(_ item: CustomizeItem) async {
        guard !pending.contains(item.id) else { return }
        actionError = nil
        pending.insert(item.id)
        defer { pending.remove(item.id) }
        do {
            let folder = try await CapabilitiesClient.shared.installCatalogSkill(id: item.id)
            installedCatalog[item.id] = folder
            await load(.skills, force: true)
        } catch {
            actionError = "Couldn't add \(item.name). \(FriendlyErrorMessage.from(error.localizedDescription))"
        }
    }

    @MainActor
    private func load(_ which: CustomizeSection, force: Bool) async {
        guard force || items[which] == nil else { return }
        loading.insert(which)
        defer { loading.remove(which) }
        do {
            switch which {
            case .skills:
                if let installed = try? await CapabilitiesClient.shared.installedCatalogSkills() {
                    installedCatalog = installed
                }
                items[which] = try await CapabilitiesClient.shared.scanSkills().map(CustomizeItem.init(capability:))
            case .plugins:
                // The built-in plugins first, then plugin folders on the
                // computer (web useSectionItems('plugins')). The built-ins
                // list even when the computer can't be scanned.
                let on = FeaturePlugins.enabledIds()
                let builtIns = FeaturePlugins.all.map { CustomizeItem(feature: $0, enabled: on.contains($0.id)) }
                items[which] = builtIns
                let ids = Set(builtIns.map(\.id))
                let folders = try await CapabilitiesClient.shared.scanPlugins()
                    .filter { !ids.contains($0.id) }
                    .map(CustomizeItem.init(capability:))
                items[which] = builtIns + folders
            case .connectors:
                items[which] = try await ConnectorsClient().listConnectors().map(CustomizeItem.init(connector:))
            }
            failures[which] = nil
        } catch is CancellationError {
        } catch {
            failures[which] = SettingsLoadFailure.from(error)
        }
    }
}

/// A skill, plugin or connector as Customize shows it (the web's ItemView).
struct CustomizeItem: Identifiable {
    let id: String
    let name: String
    let description: String
    let author: String?
    let category: String?
    var enabled: Bool
    var capability: InstalledCapability?
    let connector: Connector?
    /// A skill directory entry not on the computer yet.
    var catalog: SkillCatalogEntry? = nil
    /// A built-in plugin (on/off kept on this device).
    var feature: FeaturePlugin? = nil

    init(feature plugin: FeaturePlugin, enabled: Bool) {
        id = plugin.id
        name = plugin.name
        description = plugin.description
        author = plugin.author
        category = plugin.category
        self.enabled = enabled
        capability = nil
        connector = nil
        feature = plugin
    }

    init(capability: InstalledCapability) {
        id = capability.id
        name = capability.name
        description = capability.description
        author = capability.author.isEmpty ? nil : capability.author
        category = nil
        enabled = capability.enabled
        self.capability = capability
        connector = nil
    }

    init(catalog entry: SkillCatalogEntry) {
        id = entry.id
        name = entry.name
        description = entry.description
        author = entry.author
        category = entry.category
        enabled = false
        capability = nil
        connector = nil
        catalog = entry
    }

    init(connector: Connector) {
        id = connector.id
        name = connector.name
        description = connector.description ?? ""
        author = nil
        category = connector.category
        enabled = connector.isConnected
        capability = nil
        self.connector = connector
    }
}
