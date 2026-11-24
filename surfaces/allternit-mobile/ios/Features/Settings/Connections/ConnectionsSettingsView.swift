import SwiftUI
import UIKit

/// A settings page a row opens (the existing flow for that kind of connection).
enum ConnPage: String, Identifiable, Hashable {
    case voice, phoneControl, androidScreen, pairComputer, cloudComputer, billing, gizzi, browse
    var id: String { rawValue }
}

/// Where a row's tap goes. Shared by Settings › Connections and first-run setup.
enum ConnRoute {
    case sheet(ConnSheet)
    case page(ConnPage)
    case connectors
    case channels
    case none

    /// Rows that live in their own settings page show a chevron.
    @MainActor
    static func opensPage(_ entry: ConnEntry, store: ConnectionsStore) -> Bool {
        ["openai-voice", "elevenlabs", "phone-control", "android-screen", "this-mac", "server", "gizzi"].contains(entry.id)
            || entry.group == .apps || entry.group == .channels
            // No cloud computer yet: the row says "See plans" instead.
            || (entry.id == "cloud-computer" && store.state(entry.id) == .on)
    }

    @MainActor
    static func forConnect(_ id: String, store: ConnectionsStore) -> ConnRoute {
        if id == "api-key" { return .sheet(.keys(only: nil)) }
        if id == "gemini" { return .sheet(.keys(only: "google-key")) }
        if id.hasSuffix("-key") { return .sheet(.keys(only: id)) }
        switch id {
        case "openai-voice", "elevenlabs": return .page(.voice)
        case "phone-control": return .page(.phoneControl)
        case "android-screen": return .page(.androidScreen)
        case "this-mac", "server": return .page(.pairComputer)
        case "cloud-computer": return .page(store.state(id) == .on ? .cloudComputer : .billing)
        case "gizzi": return .page(.gizzi)
        case "local": return .sheet(.onComputer(id))
        // The box on Allternit when the server offers it (device code in
        // Safari); otherwise, or when already signed in here, on this iPhone (§9.9).
        case "chatgpt":
            return store.row(id).runsOn == "runner" && !store.chatgptOnDevice ? .sheet(.login(id)) : .sheet(.chatgptDevice)
        case "phone", "allternit": return .none
        default: break
        }
        guard let entry = ConnectionsCatalog.entry(id) else { return .none }
        switch entry.group {
        case .apps: return .connectors
        case .channels: return .channels
        default:
            // The Claude organization link is a redirect sign-in the server starts and polls.
            return entry.ways.contains(.code) || id == "claude-org" ? .sheet(.login(id)) : .none
        }
    }
}

/// Settings › Connections (connect kit option A). One page for every
/// connection: what needs you, then one card per group with the same row,
/// the same four status words and the same sheet. Brains are split into
/// "Works on any device", "Needs your computer" and "Coming soon".
struct ConnectionsSettingsView: View {
    @ObservedObject private var store = ConnectionsStore.shared
    @State private var sheet: ConnSheet?
    @State private var page: ConnPage?
    @State private var showConnectors = false
    @StateObject private var moreRouter = ConnMoreRouter()
    /// The group whose More panel is open (one at a time).
    @State private var moreGroup: ConnGroup?
    @State private var showChannels = false
    @State private var status: ConnStatusFilter = .all
    @State private var query = ""
    @State private var isSearching = false
    @AppStorage("connections.layout") private var layoutRaw = OverviewLayout.list.rawValue
    @Environment(\.dismiss) private var dismiss
    @State private var actionError: String?
    @State private var confirmUnlink = false
    @Environment(\.openURL) private var openURL

    @ReducedMotion private var reduceMotion

    /// The chosen layout: List rows, Grid cards (two columns) or Deck cards
    /// (one horizontal strip). Brains keep their sub-sections only in List.
    private var layout: OverviewLayout { OverviewLayout(stored: layoutRaw) ?? .list }

    var body: some View {
        VStack(spacing: 0) {
            header
            if isSearching || !query.isEmpty {
                ProjectsSearchField(query: $query, placeholder: "Search connections")
            }
            pageContent
        }
        .background(Color("BgPrimary").ignoresSafeArea())
        .toolbar(.hidden, for: .navigationBar)
        .background(KeepsSwipeBack())
        .task { await store.load() }
        .sheet(item: $sheet, onDismiss: { Task { await store.load() } }) { s in
            ConnectSheetView(sheet: s, store: store, onFallback: { f in handleFallback(f) })
        }
        .sheet(isPresented: $showConnectors, onDismiss: { Task { await store.load(); await store.refreshPromotedApps() } }) { ConnectorsListView() }
        .sheet(isPresented: $showChannels, onDismiss: { Task { await store.load() } }) { GiveBotPhoneSheet(botId: nil) }
        .navigationDestination(item: $page) { p in ConnPageView(page: p) }
        .connMoreRouting(moreRouter, store: store)
        .confirmationDialog("Unlink your Claude organization?", isPresented: $confirmUnlink, titleVisibility: .visible) {
            Button("Unlink", role: .destructive) {
                UINotificationFeedbackGenerator().notificationOccurred(.warning)
                disconnect(name: "Claude organization", id: "claude-org")
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Chats on your Claude organization stop. Allternit stops billing it right away. You can link it again later.")
        }
        .accessibilityIdentifier("connections-page")
    }

    // MARK: - Header

    private var header: some View {
        ZStack {
            Text("Connections")
                .font(.system(size: 17, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
                .accessibilityAddTraits(.isHeader)
            HStack {
                CodeGlassRoundButton(icon: "chevron.left", label: "Back") { dismiss() }
                Spacer()
                ProjectsPill(
                    searching: $isSearching,
                    filtersActive: status != .all || !needs.isEmpty,
                    searchLabel: "Search connections",
                    dotColor: needs.isEmpty ? nil : Color(hex: "#E8A33A"),
                    filterLabel: needs.isEmpty ? "Filter and layout" : "Filter and layout, something needs you"
                ) {
                    ConnectionsFilterPopover(
                        layoutRaw: $layoutRaw,
                        status: $status,
                        counts: ConnStatusCounts(states: listed.map { store.state($0.id) }),
                        onFindMore: { page = .browse }
                    )
                }
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 4)
    }

    private var pageContent: some View {
        PhoneSettingsPage(title: "Connections") {
            SettingsDescription(text: "Everything Allternit uses, in one place. Each one connects the same way and shows the same status.")
            if let loadError = store.loadError {
                SettingsErrorBanner(message: "Couldn't load your connections. \(loadError)")
            }
            if let actionError {
                SettingsErrorBanner(message: actionError)
            }
            if status != .all {
                statusChip
            }
            if !store.loaded && store.isLoading {
                HStack { Spacer(); ProgressView(); Spacer() }.padding(.vertical, 24)
            }
            ForEach(ConnGroup.allCases, id: \.self) { group in
                groupSection(group)
            }
            // Harness & Bots 2.0: the runtimes (status filters don't apply to them).
            if status == .all {
                RuntimesConnectionsSection(layout: layout, query: query)
            }
            if visible.isEmpty && !runtimesMatch {
                Text(query.isEmpty && status == .all ? "Nothing here yet." : query.isEmpty ? "Nothing is \(status.label.lowercased())." : "Nothing matches “\(query)”.")
                    .font(.system(size: 14))
                    .foregroundColor(Color("TextSecondary"))
                    .padding(.top, 12)
            }
        }
        .refreshable { await store.load() }
    }

    // MARK: - Filtering

    private var visible: [ConnEntry] {
        let q = query.trimmingCharacters(in: .whitespaces).lowercased()
        return ConnectionsCatalog.entries.filter { e in
            guard store.isListed(e.id), status.matches(store.state(e.id)) else { return false }
            return q.isEmpty || e.name.lowercased().contains(q) || (e.blurb ?? "").lowercased().contains(q)
                || e.group.title.lowercased().contains(q)
        }
    }

    /// The Runtimes section shows something (it has no status filter).
    private var runtimesMatch: Bool {
        guard status == .all else { return false }
        let q = query.trimmingCharacters(in: .whitespaces).lowercased()
        return q.isEmpty || "runtimes".contains(q) || RuntimeRegistryStore.shared.runtimes.contains {
            [$0.name, $0.vendor, $0.categoryLabel].joined(separator: " ").lowercased().contains(q)
        }
    }

    private var needs: [ConnEntry] { ConnectionsCatalog.entries.filter { store.isListed($0.id) && store.state($0.id) == .needs } }

    private var listed: [ConnEntry] { ConnectionsCatalog.entries.filter { store.isListed($0.id) } }

    /// The removable chip shown while a status filter is on.
    private var statusChip: some View {
        HStack {
            Button {
                Haptics.light()
                status = .all
            } label: {
                HStack(spacing: 6) {
                    Text(status.label).font(.system(size: 13.5, weight: .medium))
                    Image(systemName: "xmark").font(.system(size: 11, weight: .semibold))
                }
                .foregroundColor(Color("TextPrimary"))
                .padding(.horizontal, 12)
                .frame(minHeight: 32)
                .background(Capsule().fill(Color("FillNeutral")))
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Showing \(status.label). Remove filter")
            Spacer()
        }
        .padding(.top, 8)
    }

    // MARK: - Groups

    /// Option A's brain sections. "Coming soon" holds every brain this account
    /// can't use yet that doesn't need a computer (the server says which).
    private func brainSections(_ rows: [ConnEntry]) -> [(String, [ConnEntry])] {
        let anyDevice = rows.filter { $0.runsOn != .computer && store.isAvailable($0.id) }
        let computer = rows.filter { $0.runsOn == .computer }
        let soon = rows.filter { $0.runsOn != .computer && !store.isAvailable($0.id) }
        return [("Works on any device", anyDevice), ("Needs your computer", computer), ("Coming soon", soon)].filter { !$0.1.isEmpty }
    }

    @ViewBuilder
    /// The ChatGPT subscription and Codex CLI sit in one card at the top of Brains
    /// (List layout, when both rows are listed). Codex then leaves Coding tools.
    private var groupsAccountPair: Bool {
        layout == .list && AccountPair.pairsUp(visible.map(\.id))
    }

    private func groupSection(_ group: ConnGroup) -> some View {
        let rows = visible.filter { $0.group == group && !(groupsAccountPair && group == .tools && $0.id == AccountPair.codexId) }
        let pairEntries = (group == .brains && groupsAccountPair)
            ? [AccountPair.chatgptId, AccountPair.codexId].compactMap { id in visible.first { $0.id == id } } : []
        if !rows.isEmpty {
            VStack(alignment: .leading, spacing: 0) {
                HStack(alignment: .firstTextBaseline) {
                    Text(group.title)
                        .font(.system(size: 17, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .accessibilityAddTraits(.isHeader)
                    Spacer()
                    Text("\(rows.filter { store.state($0.id) == .on }.count) connected")
                        .font(.system(size: 12.5))
                        .foregroundColor(Color("TextTertiary"))
                }
                .padding(.top, 20)
                Text(group.blurb)
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
                    .padding(.top, 2)
                    .padding(.bottom, 8)
                if group != .brains, rows.count > 1, rows.allSatisfy({ !store.isAvailable($0.id) && store.row($0.id).fallbacks.first == "this-mac" }) {
                    collapsedNeedsComputer(rows)
                } else {
                    switch layout {
                    case .grid:
                        cardLayout(flatRows(group, rows), deck: false, more: group)
                    case .deck:
                        cardLayout(flatRows(group, rows), deck: true, more: group)
                    case .list:
                    PhoneSettingsCard {
                        if group == .brains {
                            if !pairEntries.isEmpty {
                                AccountPairCard { rowList(pairEntries, accountMarks: true) }
                            }
                            let sections = brainSections(rows.filter { e in !pairEntries.contains { $0.id == e.id } })
                            ForEach(Array(sections.enumerated()), id: \.element.0) { si, section in
                                if si > 0 || !pairEntries.isEmpty { PhoneCardDivider() }
                                Text(section.0.uppercased())
                                    .font(.system(size: 11, weight: .semibold))
                                    .tracking(0.6)
                                    .foregroundColor(Color("TextTertiary"))
                                    .frame(maxWidth: .infinity, alignment: .leading)
                                    .padding(.horizontal, 16)
                                    .padding(.vertical, 6)
                                    .background(Color("TextPrimary").opacity(0.03))
                                rowList(section.1)
                            }
                        } else {
                            rowList(rows)
                        }
                        if showsMore(group) {
                            PhoneCardDivider()
                            moreTile(group, card: false)
                        }
                    }
                    }
                }
                if showsMore(group) && moreGroup == group {
                    ConnMorePanel(group: group, layout: layout, store: store, router: moreRouter) { page = .browse }
                        .transition(reduceMotion ? .identity : .opacity)
                }
            }
            .animation(reduceMotion ? nil : Motion.fade, value: layoutRaw)
        }
    }

    /// Grid and Deck show Brains flat, in the order of the List sub-sections.
    private func flatRows(_ group: ConnGroup, _ rows: [ConnEntry]) -> [ConnEntry] {
        group == .brains ? brainSections(rows).flatMap { $0.1 } : rows
    }

    private func isOpensPage(_ e: ConnEntry) -> Bool {
        ConnRoute.opensPage(e, store: store) && store.isAvailable(e.id)
    }

    /// Cards for Grid or Deck. The Claude organization link needs room for its
    /// note, so it stays a full-width row under the cards.
    @ViewBuilder
    private func cardLayout(_ rows: [ConnEntry], deck: Bool, more: ConnGroup? = nil) -> some View {
        let cards = rows.filter { $0.id != "claude-org" }
        let wide = rows.filter { $0.id == "claude-org" }
        VStack(alignment: .leading, spacing: 12) {
            if !cards.isEmpty {
                if deck {
                    ScrollView(.horizontal, showsIndicators: false) {
                        LazyHStack(alignment: .top, spacing: 12) {
                            ForEach(cards) { e in
                                ConnectionCardView(entry: e, store: store, opensPage: isOpensPage(e), accountExtras: cardExtras(e)) { act(e, $0) }
                                    .containerRelativeFrame(.horizontal) { w, _ in w * 0.78 }
                            }
                            if let more, showsMore(more) {
                                moreTile(more, card: true)
                                    .containerRelativeFrame(.horizontal) { w, _ in w * 0.5 }
                            }
                        }
                        .scrollTargetLayout()
                    }
                    .scrollTargetBehavior(.viewAligned)
                } else {
                    LazyVGrid(columns: [GridItem(.flexible(), spacing: 12, alignment: .top), GridItem(.flexible(), spacing: 12, alignment: .top)], alignment: .center, spacing: 12) {
                        ForEach(cards) { e in
                            ConnectionCardView(entry: e, store: store, opensPage: isOpensPage(e), accountExtras: cardExtras(e)) { act(e, $0) }
                        }
                        if let more, showsMore(more) { moreTile(more, card: true) }
                    }
                }
            }
            if !wide.isEmpty {
                PhoneSettingsCard { rowList(wide) }
            }
        }
    }

    /// Grid and Deck keep both ChatGPT rows as separate cards (Codex stays in Coding tools) but give them the
    /// List rows' titles, badges and Powers line. `paired` adds the "signing out of one" line when both show.
    private func cardExtras(_ e: ConnEntry) -> AccountPair.Extras? {
        AccountPair.extras(id: e.id, row: store.row(e.id), paired: AccountPair.pairsUp(visible.map(\.id)))
    }

    /// The More tile shows only while the list is unfiltered and the server
    /// counted extras for the group (hidden silently when the count call failed).
    private func showsMore(_ group: ConnGroup) -> Bool {
        status == .all && query.isEmpty && store.hasMore(group)
    }

    private func moreTile(_ group: ConnGroup, card: Bool) -> some View {
        ConnMoreTile(group: group, count: store.catalogCounts?[group.rawValue] ?? 0, expanded: moreGroup == group, card: card) {
            withAnimation(reduceMotion ? nil : Motion.fade) { moreGroup = moreGroup == group ? nil : group }
        }
    }

    @ViewBuilder
    private func rowList(_ rows: [ConnEntry], accountMarks: Bool = false) -> some View {
        ForEach(Array(rows.enumerated()), id: \.element.id) { i, e in
            if i > 0 { PhoneCardDivider() }
            ConnectionRowView(
                entry: e, store: store,
                opensPage: ConnRoute.opensPage(e, store: store) && store.isAvailable(e.id),
                accountExtras: AccountPair.extras(id: e.id, row: store.row(e.id), paired: accountMarks),
                accountMark: accountMarks
            ) { action in
                act(e, action)
            }
        }
    }

    /// Every row in the group runs on a computer and there is none: one card, one button.
    private func collapsedNeedsComputer(_ rows: [ConnEntry]) -> some View {
        HStack(spacing: 12) {
            HStack(spacing: -6) {
                ForEach(rows.prefix(4)) { e in
                    ConnLogoView(logo: e.logo, name: e.name, size: 28)
                        .overlay(RoundedRectangle(cornerRadius: 7).stroke(Color("FillNeutral"), lineWidth: 2))
                }
            }
            VStack(alignment: .leading, spacing: 2) {
                Text(rows.map(\.name).joined(separator: ", "))
                    .font(.system(size: 14, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(2)
                Text("These run on a computer. Add one when you want them.")
                    .font(.system(size: 12.5))
                    .foregroundColor(Color("TextSecondary"))
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            Button("Add a computer") { page = .pairComputer }.buttonStyle(ConnPrimaryButtonStyle())
        }
        .padding(14)
        .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(Color("FillNeutral")))
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("conn-group-collapsed")
    }

    // MARK: - Actions

    private func act(_ entry: ConnEntry, _ action: ConnRowAction) {
        actionError = nil
        switch action {
        case .fallback(let f):
            handleFallback(f)
        case .disconnect:
            if entry.id == "claude-org" { confirmUnlink = true; return }
            disconnect(name: entry.name, id: entry.id)
        case .fix where entry.id == "claude-org" && store.row(entry.id).reasonCode == "spend_limit_reached":
            // Raised at Anthropic, not here.
            openURL(ConnectionsCatalog.claudeLinkedPlatformsURL)
        case .connect, .fix, .open:
            route(ConnRoute.forConnect(entry.id, store: store))
        }
    }

    private func disconnect(name: String, id: String) {
        Task {
            do {
                try await store.disconnect(id)
            } catch let unconfirmed as ChatGPTSignOutUnconfirmed {
                actionError = unconfirmed.errorDescription
            } catch {
                actionError = "Couldn't disconnect \(name). \(ConnectionsClient.describe(error))"
            }
        }
    }

    private func handleFallback(_ id: String) {
        if id == "cloud-computer" { page = .billing; return }
        route(ConnRoute.forConnect(id, store: store))
    }

    private func route(_ r: ConnRoute) {
        switch r {
        case .sheet(let s): sheet = s
        case .page(let p): page = p
        case .connectors: showConnectors = true
        case .channels: showChannels = true
        case .none: break
        }
    }
}

/// The existing settings pages a row opens.
struct ConnPageView: View {
    let page: ConnPage

    var body: some View {
        switch page {
        case .voice: VoiceSettingsPage()
        case .phoneControl: AppPhonesSettingsView()
        case .androidScreen: AndroidScreenPairingView()
        case .pairComputer: DevicePairingView()
        case .cloudComputer: CloudInstancesManagerView()
        case .billing: ComputeBillingView(title: "Billing")
        case .gizzi: ProductsSettingsView(part: .gizziCode)
        case .browse: ConnectionsBrowseView()
        }
    }
}
