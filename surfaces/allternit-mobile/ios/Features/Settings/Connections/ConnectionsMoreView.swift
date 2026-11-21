import SwiftUI
import UIKit

// "More" and "Find more connections" (contract §11, decisions D3/D4). The
// server's catalog holds more than the page lists; each group gets a More
// tile that opens a small capped panel in place, and the browse page shows
// every group at once. Everything here reuses the row pieces (logo, trailing
// action) so an unavailable item reads like an unavailable row.

// MARK: - One catalog item as row data

/// A catalog entry turned into the same data a row uses.
struct ConnMoreItem: Identifiable, Equatable {
    let id: String
    let source: ConnCatalogEntry
    let entry: ConnEntry
    let row: ConnectionWire

    var blocked: Bool { !row.available }

    init(_ e: ConnCatalogEntry) {
        id = e.id
        source = e
        let group = ConnGroup(rawValue: e.group) ?? .apps
        let ways = e.ways.compactMap { ConnWay(rawValue: $0) }
        let runs = e.runsOn.flatMap { ConnRunsOn(rawValue: $0) } ?? .server
        var logo: ConnLogo
        if let l = e.logo, UIImage(named: l) != nil {
            logo = .asset(l)
        } else if let k = ConnectionsCatalog.keyProvider(e.id) {
            logo = k.logo
        } else if UIImage(named: "applogo-\(e.connectorId ?? e.id)") != nil {
            logo = .asset("applogo-\(e.connectorId ?? e.id)")
        } else {
            logo = .symbol(e.isKey ? "key" : group == .apps ? "square.grid.2x2" : "circle.grid.2x2")
        }
        entry = ConnEntry(id: e.id, group: group, name: e.name, logo: logo, blurb: e.blurb ?? e.category,
                          ways: ways.isEmpty ? [.oauth] : ways, runsOn: runs, keyPage: e.keyPage)
        var reason = e.unavailableReason
        var fallbacks = e.fallbacks
        if !e.available {
            if reason == nil { reason = runs == .computer ? "Runs on a computer." : "Not available yet." }
            if fallbacks.isEmpty && runs == .computer { fallbacks = ["this-mac"] }
        }
        row = ConnectionWire(id: e.id, available: e.available, unavailableReason: reason, fallbacks: fallbacks)
    }

    /// The line under the name.
    var statusText: String {
        if blocked { return row.unavailableReason ?? "Not available yet." }
        return entry.blurb ?? entry.group.title
    }
}

// MARK: - Paging model

/// Loads the catalog 24 at a time with the server's opaque cursor. A change of
/// search text or group starts over; late answers from an old query are dropped.
@MainActor
final class ConnMoreModel: ObservableObject {
    enum Phase: Equatable { case loading, ready, failed(String) }

    @Published private(set) var items: [ConnMoreItem] = []
    @Published private(set) var phase: Phase = .loading
    @Published private(set) var loadingMore = false
    @Published private(set) var moreFailed = false
    /// The server's count for this query (the group's count, or all groups).
    @Published private(set) var total = 0
    /// Counts per group for the same query (browse chips).
    @Published private(set) var counts: [String: Int] = [:]

    private let store: ConnectionsStore
    private var cursor: String?
    private var seen = Set<String>()
    private var generation = 0

    init(store: ConnectionsStore) { self.store = store }

    var hasMore: Bool { cursor != nil }

    func reset(group: ConnGroup?, q: String) async {
        generation += 1
        let gen = generation
        items = []
        cursor = nil
        seen = []
        moreFailed = false
        phase = .loading
        await fetch(group: group, q: q, gen: gen)
    }

    func loadMore(group: ConnGroup?, q: String) async {
        guard cursor != nil, !loadingMore, phase == .ready else { return }
        let gen = generation
        loadingMore = true
        moreFailed = false
        await fetch(group: group, q: q, gen: gen)
        if gen == generation { loadingMore = false }
    }

    func loadMoreIfNeeded(current id: String, group: ConnGroup?, q: String) {
        guard let index = items.firstIndex(where: { $0.id == id }), index >= items.count - 3 else { return }
        Task { await loadMore(group: group, q: q) }
    }

    private func fetch(group: ConnGroup?, q: String, gen: Int) async {
        // A page can be emptied by the client rules; keep going a few pages.
        for _ in 0..<4 {
            do {
                let result = try await store.catalogPage(group: group, q: q, cursor: cursor)
                guard gen == generation else { return }
                counts = result.page.counts
                total = group.flatMap { result.page.counts[$0.rawValue] } ?? result.page.total
                cursor = result.page.nextCursor
                let fresh = result.entries.filter { seen.insert($0.id).inserted }
                items += fresh.map(ConnMoreItem.init)
                phase = .ready
                if !fresh.isEmpty || cursor == nil { return }
            } catch {
                guard gen == generation else { return }
                if items.isEmpty { phase = .failed(ConnectionsClient.describe(error)) } else { moreFailed = true }
                return
            }
        }
    }
}

// MARK: - Connect routing

struct ConnConnectorsRequest: Identifiable {
    let id = UUID()
    let initialId: String?
}

/// Where a Connect tap in More goes: the key sheet for key providers, the
/// connectors flow (with that connector preselected) for apps, and the same
/// routes as rows for everything else.
@MainActor
final class ConnMoreRouter: ObservableObject {
    @Published var sheet: ConnSheet?
    @Published var connectors: ConnConnectorsRequest?
    @Published var channels = false
    @Published var page: ConnPage?
    /// Bumped when a sheet closes so open panels reload (D4).
    @Published var epoch = 0

    func connect(_ item: ConnMoreItem, store: ConnectionsStore) {
        let e = item.source
        if e.isKey { route(ConnRoute.forConnect(e.id, store: store)); return }
        if e.connectorId != nil || item.entry.group == .apps {
            connectors = ConnConnectorsRequest(initialId: e.connectorId ?? e.id)
            return
        }
        if item.entry.group == .channels { channels = true; return }
        route(ConnRoute.forConnect(e.id, store: store))
    }

    func fallback(_ id: String, store: ConnectionsStore) {
        if id == "cloud-computer" { page = .billing; return }
        route(ConnRoute.forConnect(id, store: store))
    }

    private func route(_ r: ConnRoute) {
        switch r {
        case .sheet(let s): sheet = s
        case .page(let p): page = p
        case .connectors: connectors = ConnConnectorsRequest(initialId: nil)
        case .channels: channels = true
        case .none: break
        }
    }
}

/// Presents what the router asks for. After any sheet closes the store reloads
/// (promoted apps included) so a connected app moves into its group.
struct ConnMoreRouting: ViewModifier {
    @ObservedObject var router: ConnMoreRouter
    @ObservedObject var store: ConnectionsStore

    func body(content: Content) -> some View {
        content
            .sheet(item: $router.sheet, onDismiss: reload) { s in
                ConnectSheetView(sheet: s, store: store, onFallback: { router.fallback($0, store: store) })
            }
            .sheet(item: $router.connectors, onDismiss: reload) { req in
                ConnectorsListView(initialConnectorId: req.initialId)
            }
            .sheet(isPresented: $router.channels, onDismiss: reload) { GiveBotPhoneSheet(botId: nil) }
            .navigationDestination(item: $router.page) { ConnPageView(page: $0) }
    }

    private func reload() {
        Task {
            await store.load()
            await store.refreshPromotedApps()
            router.epoch += 1
        }
    }
}

extension View {
    func connMoreRouting(_ router: ConnMoreRouter, store: ConnectionsStore) -> some View {
        modifier(ConnMoreRouting(router: router, store: store))
    }
}

// MARK: - Tile

/// The More tile at the end of a group. `card` is the Grid/Deck shape.
struct ConnMoreTile: View {
    let group: ConnGroup
    let count: Int
    let expanded: Bool
    var card = false
    let action: () -> Void

    var body: some View {
        Button {
            Haptics.light()
            action()
        } label: {
            if card {
                VStack(alignment: .leading, spacing: 10) {
                    Image(systemName: "ellipsis")
                        .font(.system(size: 18, weight: .medium))
                        .foregroundColor(Color("TextPrimary"))
                        .frame(width: 40, height: 40)
                        .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color("TextPrimary").opacity(0.07)))
                    Text("More")
                        .font(.system(size: 16, weight: .medium))
                        .foregroundColor(Color("TextPrimary"))
                    Text("\(count) more \(group.title.lowercased())")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(2)
                    Spacer(minLength: 0)
                }
                .padding(14)
                .frame(maxWidth: .infinity, minHeight: 168, alignment: .topLeading)
                .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(Color("FillNeutral")))
            } else {
                HStack(spacing: 12) {
                    Text("More \(group.title.lowercased())")
                        .font(.system(size: 15, weight: .medium))
                        .foregroundColor(Color("TextPrimary"))
                    Spacer()
                    Text("\(count)")
                        .font(.system(size: 13)).monospacedDigit()
                        .foregroundColor(Color("TextSecondary"))
                    Image(systemName: expanded ? "chevron.up" : "chevron.down")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundColor(Color("TextTertiary"))
                }
                .padding(.horizontal, 16)
                .frame(minHeight: 48)
                .contentShape(Rectangle())
            }
        }
        .buttonStyle(.plain)
        .accessibilityLabel("More \(group.title.lowercased()), \(count)")
        .accessibilityValue(expanded ? "Expanded" : "Collapsed")
        .accessibilityHint(expanded ? "Closes the list" : "Opens a short list")
        .accessibilityIdentifier("conn-more-\(group.rawValue)")
    }
}

// MARK: - Pieces

private struct ConnMoreSearchField: View {
    @Binding var query: String
    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass").font(.system(size: 14)).foregroundColor(Color("TextTertiary"))
            TextField("Search or type a category", text: $query)
                .font(.system(size: 15))
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .submitLabel(.search)
            if !query.isEmpty {
                Button { query = "" } label: {
                    Image(systemName: "xmark.circle.fill").foregroundColor(Color("TextTertiary"))
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Clear search")
            }
        }
        .padding(.horizontal, 12)
        .frame(height: 40)
        .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("TextPrimary").opacity(0.06)))
    }
}

/// A catalog item as a list row.
struct ConnMoreRow: View {
    let item: ConnMoreItem
    var tag: String? = nil
    let onConnect: () -> Void
    let onFallback: (String) -> Void

    var body: some View {
        HStack(spacing: 12) {
            ConnLogoView(logo: item.entry.logo, name: item.entry.name, size: 36)
                .opacity(item.blocked ? 0.5 : 1)
                .saturation(item.blocked ? 0 : 1)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text(item.entry.name)
                        .font(.system(size: 16, weight: .medium))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    if let tag {
                        Text("· \(tag)").font(.system(size: 12)).foregroundColor(Color("TextSecondary")).lineLimit(1)
                    }
                }
                Text(item.statusText)
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(item.blocked ? 3 : 1)
                    .fixedSize(horizontal: false, vertical: item.blocked)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            ConnRowTrailing(entry: item.entry, row: item.row, blocked: item.blocked, onAction: act)
        }
        .padding(.horizontal, 16)
        .frame(minHeight: 64)
        .padding(.vertical, 4)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("conn-more-row-\(item.id)")
    }

    private func act(_ a: ConnRowAction) {
        if case .fallback(let f) = a { onFallback(f) } else { onConnect() }
    }
}

/// A catalog item as a card (Grid and Deck).
struct ConnMoreCard: View {
    let item: ConnMoreItem
    let onConnect: () -> Void
    let onFallback: (String) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            ConnLogoView(logo: item.entry.logo, name: item.entry.name, size: 40)
                .opacity(item.blocked ? 0.5 : 1)
                .saturation(item.blocked ? 0 : 1)
            Text(item.entry.name)
                .font(.system(size: 16, weight: .medium))
                .foregroundColor(Color("TextPrimary"))
                .lineLimit(1)
            Text(item.statusText)
                .font(.system(size: 13))
                .foregroundColor(Color("TextSecondary"))
                .lineLimit(3)
                .frame(maxWidth: .infinity, alignment: .leading)
            Spacer(minLength: 0)
            ConnRowTrailing(entry: item.entry, row: item.row, blocked: item.blocked, compact: true, fill: true) { a in
                if case .fallback(let f) = a { onFallback(f) } else { onConnect() }
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, minHeight: 168, alignment: .topLeading)
        .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(Color("FillNeutral")))
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("conn-more-card-\(item.id)")
    }
}

private struct ConnMoreSkeleton: View {
    @ReducedMotion private var reduceMotion
    @State private var dim = false
    var body: some View {
        VStack(spacing: 0) {
            ForEach(0..<3, id: \.self) { _ in
                HStack(spacing: 12) {
                    RoundedRectangle(cornerRadius: 9).frame(width: 36, height: 36)
                    VStack(alignment: .leading, spacing: 6) {
                        RoundedRectangle(cornerRadius: 4).frame(width: 120, height: 12)
                        RoundedRectangle(cornerRadius: 4).frame(width: 190, height: 10)
                    }
                    Spacer()
                }
                .padding(.horizontal, 16)
                .frame(height: 64)
            }
        }
        .foregroundColor(Color("TextPrimary").opacity(dim ? 0.05 : 0.09))
        .onAppear {
            guard !reduceMotion else { return }
            withAnimation(.easeInOut(duration: 0.8).repeatForever(autoreverses: true)) { dim = true }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Loading")
    }
}

/// Empty, error and footer lines shared by the panel and the browse page.
private struct ConnMoreStates: View {
    @ObservedObject var model: ConnMoreModel
    let query: String
    let retry: () -> Void

    var body: some View {
        switch model.phase {
        case .loading:
            ConnMoreSkeleton()
        case .failed(let message):
            VStack(spacing: 10) {
                Text("Couldn't load more connections. \(message)")
                    .font(.system(size: 13.5))
                    .foregroundColor(Color("TextSecondary"))
                    .multilineTextAlignment(.center)
                Button("Retry", action: retry).buttonStyle(ConnQuietButtonStyle())
            }
            .frame(maxWidth: .infinity)
            .padding(20)
        case .ready:
            if model.items.isEmpty {
                let trimmed = query.trimmingCharacters(in: .whitespaces)
                Text(trimmed.isEmpty ? "Nothing more to add right now." : "Nothing matches “\(trimmed)”.")
                    .font(.system(size: 14))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(maxWidth: .infinity)
                    .padding(24)
            }
        }
    }
}

private struct ConnMoreFooter: View {
    @ObservedObject var model: ConnMoreModel
    let retry: () -> Void

    var body: some View {
        if model.phase == .ready && !model.items.isEmpty {
            VStack(spacing: 6) {
                if model.loadingMore { ProgressView().controlSize(.small) }
                if model.moreFailed {
                    Button("Couldn't load more. Retry", action: retry).buttonStyle(ConnQuietButtonStyle())
                }
                Text(model.hasMore || model.total > model.items.count
                     ? "Showing \(model.items.count) of \(max(model.total, model.items.count)) · scroll for more"
                     : "Showing \(model.items.count) of \(max(model.total, model.items.count))")
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextTertiary"))
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 10)
            .accessibilityElement(children: .combine)
        }
    }
}

// MARK: - The capped panel

/// The in-place panel under the More tile: a search field and a fixed-height
/// scroll that shows six at a time (List: 6 rows, Grid: 2×3, Deck: sideways).
struct ConnMorePanel: View {
    let group: ConnGroup
    let layout: OverviewLayout
    @ObservedObject var store: ConnectionsStore
    @ObservedObject var router: ConnMoreRouter
    let onBrowse: () -> Void

    @StateObject private var model: ConnMoreModel
    @State private var query = ""

    private let rowHeight: CGFloat = 72
    private let cardHeight: CGFloat = 168

    init(group: ConnGroup, layout: OverviewLayout, store: ConnectionsStore, router: ConnMoreRouter, onBrowse: @escaping () -> Void) {
        self.group = group
        self.layout = layout
        self.store = store
        self.router = router
        self.onBrowse = onBrowse
        _model = StateObject(wrappedValue: ConnMoreModel(store: store))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            ConnMoreSearchField(query: $query)
            ConnMoreStates(model: model, query: query, retry: reload)
            if !model.items.isEmpty { content }
            ConnMoreFooter(model: model) { Task { await model.loadMore(group: group, q: query) } }
            Button("Find more connections", action: onBrowse)
                .buttonStyle(ConnQuietButtonStyle())
                .frame(maxWidth: .infinity)
        }
        .padding(12)
        .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(Color("FillNeutral").opacity(0.6)))
        .padding(.top, 10)
        .task(id: "\(query)|\(router.epoch)") {
            if !query.isEmpty { try? await Task.sleep(nanoseconds: 300_000_000) }
            guard !Task.isCancelled else { return }
            await model.reset(group: group, q: query)
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("conn-more-panel-\(group.rawValue)")
    }

    private func reload() { Task { await model.reset(group: group, q: query) } }

    @ViewBuilder
    private var content: some View {
        switch layout {
        case .list:
            ScrollView {
                LazyVStack(spacing: 0) {
                    ForEach(Array(model.items.enumerated()), id: \.element.id) { i, item in
                        if i > 0 { PhoneCardDivider() }
                        ConnMoreRow(item: item, onConnect: { router.connect(item, store: store) },
                                    onFallback: { router.fallback($0, store: store) })
                            .frame(height: rowHeight)
                            .onAppear { model.loadMoreIfNeeded(current: item.id, group: group, q: query) }
                    }
                }
            }
            .frame(height: min(rowHeight * 6, rowHeight * CGFloat(model.items.count)))
        case .grid:
            ScrollView {
                LazyVGrid(columns: [GridItem(.flexible(), spacing: 12, alignment: .top), GridItem(.flexible(), spacing: 12, alignment: .top)], spacing: 12) {
                    ForEach(model.items) { item in
                        card(item)
                    }
                }
            }
            .frame(height: min(cardHeight * 3 + 24, cardHeight * CGFloat((model.items.count + 1) / 2) + CGFloat(max(0, (model.items.count + 1) / 2 - 1)) * 12))
        case .deck:
            ScrollView(.horizontal, showsIndicators: false) {
                LazyHStack(alignment: .top, spacing: 12) {
                    ForEach(model.items) { item in
                        card(item)
                            .containerRelativeFrame(.horizontal) { w, _ in w * 0.72 }
                    }
                }
                .scrollTargetLayout()
            }
            .scrollTargetBehavior(.viewAligned)
            .frame(height: cardHeight)
        }
    }

    private func card(_ item: ConnMoreItem) -> some View {
        ConnMoreCard(item: item, onConnect: { router.connect(item, store: store) },
                     onFallback: { router.fallback($0, store: store) })
            .onAppear { model.loadMoreIfNeeded(current: item.id, group: group, q: query) }
    }
}

// MARK: - Browse page

/// "Find more connections": every group at once, with group chips and search.
struct ConnectionsBrowseView: View {
    @ObservedObject private var store = ConnectionsStore.shared
    @StateObject private var router = ConnMoreRouter()
    @StateObject private var model = ConnMoreModel(store: ConnectionsStore.shared)
    @State private var group: ConnGroup?
    @State private var query = ""
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(spacing: 0) {
            ZStack {
                Text("Find more connections")
                    .font(.system(size: 17, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .accessibilityAddTraits(.isHeader)
                HStack {
                    CodeGlassRoundButton(icon: "chevron.left", label: "Back") { dismiss() }
                    Spacer()
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 4)
            ConnMoreSearchField(query: $query).padding(.horizontal, 16).padding(.top, 6)
            chips
            ScrollView {
                VStack(spacing: 0) {
                    ConnMoreStates(model: model, query: query, retry: reload)
                    LazyVStack(spacing: 0) {
                        ForEach(Array(model.items.enumerated()), id: \.element.id) { i, item in
                            if i > 0 { PhoneCardDivider() }
                            ConnMoreRow(item: item, tag: group == nil ? item.entry.group.title : nil,
                                        onConnect: { router.connect(item, store: store) },
                                        onFallback: { router.fallback($0, store: store) })
                                .onAppear { model.loadMoreIfNeeded(current: item.id, group: group, q: query) }
                        }
                    }
                    ConnMoreFooter(model: model) { Task { await model.loadMore(group: group, q: query) } }
                }
            }
        }
        .background(Color("BgPrimary").ignoresSafeArea())
        .toolbar(.hidden, for: .navigationBar)
        .background(KeepsSwipeBack())
        .connMoreRouting(router, store: store)
        .task(id: "\(query)|\(group?.rawValue ?? "all")|\(router.epoch)") {
            if !query.isEmpty { try? await Task.sleep(nanoseconds: 300_000_000) }
            guard !Task.isCancelled else { return }
            await model.reset(group: group, q: query)
        }
        .accessibilityIdentifier("connections-browse-page")
    }

    private func reload() { Task { await model.reset(group: group, q: query) } }

    private var chips: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                chip("All", selected: group == nil) { group = nil }
                ForEach(ConnGroup.allCases, id: \.self) { g in
                    let n = model.counts[g.rawValue] ?? store.catalogCounts?[g.rawValue] ?? 0
                    if n > 0 || group == g {
                        chip("\(g.title) \(n)", selected: group == g) { group = g }
                    }
                }
            }
            .padding(.horizontal, 16)
        }
        .padding(.vertical, 10)
    }

    private func chip(_ title: String, selected: Bool, action: @escaping () -> Void) -> some View {
        Button {
            Haptics.selection()
            action()
        } label: {
            Text(title)
                .font(.system(size: 13.5, weight: .medium))
                .foregroundColor(selected ? Color("BgPrimary") : Color("TextPrimary"))
                .padding(.horizontal, 12)
                .frame(minHeight: 32)
                .background(Capsule().fill(selected ? Color("TextPrimary") : Color("TextPrimary").opacity(0.07)))
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}
