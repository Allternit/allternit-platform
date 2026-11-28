import SwiftUI

// MARK: - Store

/// Search state for the Freeweights section of the Models screen.
@MainActor
final class FreeweightsBrowseStore: ObservableObject {
    @Published var query = "" {
        didSet { if query != oldValue { scheduleSearch() } }
    }
    @Published private(set) var results: [FreeweightsModelSummary] = []
    @Published private(set) var catalog: [String: FreeweightsCatalogRow] = [:]
    @Published private(set) var nextCursor: String?
    @Published private(set) var isLoading = false
    @Published private(set) var isLoadingMore = false
    /// Age of the cached copy on screen; nil when live.
    @Published private(set) var staleAge: TimeInterval?
    /// Set when the catalog is down and nothing is cached.
    @Published private(set) var downReason: String?
    @Published private(set) var hasLoaded = false
    /// The model whose detail sheet is open. The sheet hangs off the
    /// Models List (`freeweightsDetailSheet`), not the Section: a sheet on
    /// a List Section does not present.
    @Published var selected: FreeweightsModelSummary?
    /// The signed-in account's own community repos. Empty when signed out,
    /// when there is no Freeweights profile yet, or when the call failed.
    @Published private(set) var myRepos: [FreeweightsMyRepo] = []
    @Published private(set) var myReposFailed = false

    // On-device (W8): the device index gives every model's builds, so each
    // tile shows the one fit function's label for this iPhone.
    enum Filter: String, CaseIterable, Identifiable {
        case fits = "Fits this iPhone"
        case everything = "Everything"
        case phone = "Phone collection"
        var id: String { rawValue }
    }
    @Published var filter: Filter = .fits {
        didSet { if filter != oldValue { localLimit = Self.localPage; if filter == .everything, results.isEmpty { Task { await reload() } } } }
    }
    @Published private(set) var deviceIndex: FreeweightsDeviceIndex?
    /// Catalog ids in the catalog's own order (the dictionary loses it).
    @Published private(set) var catalogOrder: [String] = []
    @Published var localLimit = FreeweightsBrowseStore.localPage
    /// Opens chat with an installed model (`ondevice/…` id). Set by the
    /// Models tab; nil where there is no chat to switch to (Settings).
    var onRun: ((String) -> Void)?

    private let client: FreeweightsClient
    private var searchTask: Task<Void, Never>?
    private static let pageSize = 8
    static let localPage = 24

    init(client: FreeweightsClient = .shared) {
        self.client = client
    }

    /// Device builds for a model; nil = no device data ("Size not checked").
    func quants(_ id: String) -> [Quant]? { deviceIndex?.models[id] }

    /// The fit on this iPhone (OnDeviceRuntime drops engines this build can't run).
    func fit(_ id: String) -> FitResult { OnDeviceRuntime.shared.fit(quants(id)) }

    /// Chat models only: the iPhone runtime answers chat, nothing else.
    private static func isChat(_ row: FreeweightsCatalogRow) -> Bool {
        guard row.kind == nil || row.kind == "model" else { return false }
        let tasks = row.tasks ?? []
        return tasks.isEmpty || tasks.contains("text-generation") || tasks.contains("image-text-to-text")
    }

    /// The local lists (Fits this iPhone, Phone collection): built from the
    /// catalog and the device index, filtered by the search text.
    var localIds: [String] {
        guard let index = deviceIndex else { return [] }
        let picks = index.recommended["phone"] ?? []
        var ids: [String]
        switch filter {
        case .everything: return []
        case .phone: ids = picks
        case .fits:
            let rest = catalogOrder.filter { id in
                guard index.models[id] != nil, let row = catalog[id], Self.isChat(row), !picks.contains(id) else { return false }
                let k = fit(id).k
                return k == .ok || k == .slow
            }
            ids = picks.filter { [FitKind.ok, .slow].contains(fit($0).k) } + rest
        }
        let q = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        if !q.isEmpty {
            ids = ids.filter { $0.lowercased().contains(q) || (catalog[$0]?.displayName?.lowercased().contains(q) ?? false) }
        }
        return ids
    }

    /// A summary for a catalog-only row (the local lists have no `/api/models` row).
    func summary(for id: String) -> FreeweightsModelSummary {
        if let s = results.first(where: { $0.id == id }) { return s }
        let row = catalog[id]
        let org = id.split(separator: "/").first.map(String.init) ?? ""
        return FreeweightsModelSummary(id: id, sha: nil, pipelineTag: row?.tasks?.first, libraryName: nil, tags: nil,
                                       downloads: nil, lastModified: row?.addedAt, section: row?.section, license: row?.licenseId,
                                       badges: nil, publisherLogo: "https://freeweights.allternit.com/assets/orgs/\(org).png")
    }

    func loadIfNeeded() async {
        guard !hasLoaded else { return }
        await reload()
    }

    func reload() async {
        searchTask?.cancel()
        isLoading = true
        defer { isLoading = false; hasLoaded = true }
        // Each answer shows as soon as it lands (the catalog is the big one).
        let client = self.client, query = self.query
        let page = Task { self.apply(await client.search(query: query, limit: Self.pageSize), appending: false) }
        let index = Task { if let v = await client.deviceIndex().value { self.deviceIndex = v } }
        let rows = Task {
            if let v = await client.catalogRowList().value {
                self.catalog = Dictionary(v.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
                self.catalogOrder = v.map(\.id)
            }
        }
        let mine = Task { await self.reloadMine() }
        _ = await (page.value, index.value, rows.value, mine.value)
    }

    /// Reads "My uploads". Signed out is not an error: the section just
    /// does not show.
    func reloadMine() async {
        guard AuthManager.shared.isSignedIn else {
            myRepos = []
            myReposFailed = false
            return
        }
        do {
            myRepos = try await client.myRepos()
            myReposFailed = false
        } catch is CancellationError {
            return
        } catch {
            myReposFailed = true
        }
    }

    func loadMore() async {
        guard let cursor = nextCursor, !isLoadingMore else { return }
        isLoadingMore = true
        defer { isLoadingMore = false }
        apply(await client.search(query: query, limit: Self.pageSize, cursor: cursor), appending: true)
    }

    private func apply(_ load: FreeweightsLoad<FreeweightsPage>, appending: Bool) {
        switch load {
        case .live(let page), .cached(let page, _):
            results = appending ? results + page.models : page.models
            nextCursor = page.nextCursor
            staleAge = load.staleAge
            downReason = nil
        case .catalogDown(let reason):
            if reason == "Cancelled" { return }
            if !appending { results = []; nextCursor = nil }
            downReason = reason
        }
    }

    private func scheduleSearch() {
        searchTask?.cancel()
        searchTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(300))
            guard !Task.isCancelled, let self else { return }
            self.isLoading = true
            let page = await self.client.search(query: self.query, limit: Self.pageSize)
            guard !Task.isCancelled else { return }
            self.isLoading = false
            self.apply(page, appending: false)
        }
    }
}

// MARK: - Section on the Models screen

/// The Freeweights section of the Models screen: search the open-weights
/// catalog at freeweights.allternit.com and send a pull to your computer.
/// Lives inside `ModelManagementListContent`'s List (iOS adds no rail tab).
struct FreeweightsSection: View {
    @ObservedObject var store: FreeweightsBrowseStore

    @ObservedObject private var auth = AuthManager.shared

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            browseSection
            if auth.isSignedIn, !store.myRepos.isEmpty || store.myReposFailed {
                myUploadsSection
            }
        }
        .task(id: auth.isSignedIn) { await store.reloadMine() }
    }

    /// Rows grouped in one rounded card on the neutral fill (radius 20).
    private func card<Content: View>(@ViewBuilder _ content: () -> Content) -> some View {
        VStack(spacing: 0) { content() }
            .frame(maxWidth: .infinity)
            .background(RoundedRectangle(cornerRadius: 20, style: .continuous).fill(PolishTokens.neutralFill))
    }

    private var myUploadsSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            SectionLabel("My uploads")
            card {
                if store.myRepos.isEmpty {
                    Text("Couldn't load your uploads. Pull to refresh.")
                        .font(.subheadline)
                        .foregroundColor(Color("TextSecondary"))
                        .padding(14)
                }
                ForEach(Array(store.myRepos.enumerated()), id: \.element.id) { i, repo in
                    if i > 0 { Divider().padding(.leading, 14) }
                    FreeweightsMyRepoRow(repo: repo)
                        .padding(.horizontal, 14)
                        .padding(.vertical, 6)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            Text("Upload from the Freeweights site or the fw command on your computer. This list follows each scan.")
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
                .padding(.horizontal, 4)
        }
    }

    private var browseSection: some View {
        VStack(alignment: .leading, spacing: 12) {
            searchField

            if let age = store.staleAge {
                Text("Offline copy from \(Self.ageText(age)). Pull to refresh.")
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
                    .padding(.horizontal, 4)
            }

            SectionLabel("Models")

            if store.isLoading && store.results.isEmpty {
                ProgressView()
                    .frame(maxWidth: .infinity, alignment: .center)
                    .padding(.vertical, 20)
            } else if store.downReason != nil, store.results.isEmpty {
                FriendlyInlineStateView(
                    style: .offline,
                    icon: "wifi.slash",
                    title: "Freeweights is not answering",
                    message: "The catalog couldn't be reached. Check your connection, then try again.",
                    actionTitle: "Retry",
                    action: { Task { await store.reload() } }
                )
            } else if store.hasLoaded && store.results.isEmpty {
                FriendlyInlineStateView(
                    style: .empty,
                    icon: "magnifyingglass",
                    title: store.query.isEmpty ? "The catalog is empty" : "No models match \u{201C}\(store.query)\u{201D}",
                    message: "Try a family name like qwen, gemma or deepseek.",
                    art: .search
                )
            } else {
                card {
                    ForEach(Array(store.results.enumerated()), id: \.element.id) { i, model in
                        if i > 0 { Divider().padding(.leading, 66) }
                        Button { store.selected = model } label: {
                            FreeweightsRow(model: model, row: store.catalog[model.id])
                        }
                        .buttonStyle(FreeweightsPressRowStyle())
                    }
                }
                if store.nextCursor != nil {
                    HStack {
                        Spacer()
                        if store.isLoadingMore {
                            ProgressView()
                        } else {
                            GlassPill(title: "Show more") {
                                Task { await store.loadMore() }
                            }
                        }
                        Spacer()
                    }
                }
            }
        }
        .task {
            #if DEBUG
            if let term = UserDefaults.standard.string(forKey: "fw-search"), store.query.isEmpty {
                store.query = term
            }
            #endif
            await store.loadIfNeeded()
            #if DEBUG
            if let id = UserDefaults.standard.string(forKey: "fw-detail"), store.selected == nil {
                store.selected = store.results.first(where: { $0.id == id })
                    ?? FreeweightsModelSummary(id: id, sha: nil, pipelineTag: nil, libraryName: nil, tags: nil,
                                               downloads: nil, lastModified: nil, section: nil, license: nil, badges: nil,
                                               publisherLogo: nil)
            }
            #endif
        }
    }

    private var searchField: some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 14, weight: .medium))
                .foregroundColor(Color("TextSecondary"))
            TextField("Search Freeweights", text: $store.query)
                .font(.system(size: 13.5))
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .submitLabel(.search)
            if !store.query.isEmpty {
                Button { store.query = "" } label: {
                    Image(systemName: "xmark.circle.fill")
                        .foregroundColor(Color("TextSecondary"))
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Clear search")
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(PolishTokens.neutralFill))
    }

    static func ageText(_ age: TimeInterval) -> String {
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .full
        return formatter.localizedString(for: Date().addingTimeInterval(-age), relativeTo: Date())
    }
}

extension View {
    /// Presents the Freeweights detail sheet for `store.selected`. Attach
    /// to the page that holds `FreeweightsSection`.
    func freeweightsDetailSheet(_ store: FreeweightsBrowseStore) -> some View {
        modifier(FreeweightsDetailSheetModifier(store: store))
    }
}

private struct FreeweightsDetailSheetModifier: ViewModifier {
    @ObservedObject var store: FreeweightsBrowseStore

    func body(content: Content) -> some View {
        content.sheet(item: $store.selected) { model in
            FreeweightsDetailSheet(summary: model, row: store.catalog[model.id], indexQuants: store.quants(model.id), onRun: store.onRun)
                .presentationDetents([.large])
        }
    }
}

// MARK: - Row

private struct FreeweightsRow: View {
    let model: FreeweightsModelSummary
    let row: FreeweightsCatalogRow?

    var body: some View {
        HStack(spacing: 12) {
            icon
                .frame(width: 40, height: 40)
                .background(Color(.systemBackground))
                .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))

            VStack(alignment: .leading, spacing: 4) {
                Text(row?.displayName ?? model.name)
                    .font(.system(size: 15, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                Text(model.org)
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(1)
                HStack(spacing: 6) {
                    if model.isCommunity {
                        FreeweightsBadge(text: "Community upload", dot: Theme.statusInfo)
                    }
                    if let license = model.license ?? row?.licenseId {
                        FreeweightsBadge(text: license, dot: FreeweightsFormat.licenseTint(row?.licenseClass))
                    }
                    if let section = model.section {
                        FreeweightsBadge(text: FreeweightsFormat.section(section))
                    }
                    if let size = row?.totalSize, size > 0 {
                        FreeweightsBadge(text: FreeweightsFormat.bytes(size))
                    }
                    if row?.isLinkOnly == true {
                        FreeweightsBadge(text: "Link only", dot: Theme.statusWarning)
                    }
                }
            }
            Spacer(minLength: 0)
            Image(systemName: "chevron.right")
                .font(.system(size: 12, weight: .semibold))
                .foregroundColor(Color("TextSecondary").opacity(0.6))
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
        .accessibilityHint("Shows license, size and Send to computer")
    }

    /// The publisher's logo when the API sends one; the task glyph while it
    /// loads, when it fails, or when there is none.
    @ViewBuilder
    private var icon: some View {
        let glyph = Image(systemName: FreeweightsFormat.icon(for: model.pipelineTag))
            .font(.system(size: 17, weight: .medium))
            .foregroundColor(Color("TextSecondary"))
        if let url = model.publisherLogoURL {
            AsyncImage(url: url, transaction: Transaction(animation: .easeOut(duration: 0.15))) { phase in
                if case .success(let image) = phase {
                    image.resizable().scaledToFit().padding(6)
                } else {
                    glyph
                }
            }
            .accessibilityHidden(true)
        } else {
            glyph
        }
    }
}

/// A small fact chip. A status tint shows as a dot beside readable text
/// (status colors are too light for text on the light theme).
private struct FreeweightsBadge: View {
    let text: String
    var dot: Color? = nil

    var body: some View {
        HStack(spacing: 4) {
            if let dot {
                Circle().fill(dot).frame(width: 6, height: 6)
            }
            Text(text)
                .font(.system(size: 10, weight: .semibold))
                .foregroundColor(Color("TextSecondary"))
                .lineLimit(1)
        }
        .padding(.horizontal, 6)
        .padding(.vertical, 3)
        .background(Color(.systemBackground))
        .clipShape(Capsule())
    }
}

/// Press highlight for a row in the Freeweights card.
private struct FreeweightsPressRowStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .background(
                RoundedRectangle(cornerRadius: 14, style: .continuous)
                    .fill(configuration.isPressed ? Color("TextPrimary").opacity(0.06) : Color.clear)
            )
    }
}

// MARK: - My uploads row

/// One of the account's own repos with its scan state.
private struct FreeweightsMyRepoRow: View {
    let repo: FreeweightsMyRepo

    private var tint: Color {
        switch repo.scanState {
        case .live: return Theme.statusSuccess
        case .held, .removed: return Theme.statusError
        case .scanning, .draft: return Theme.statusWarning
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 8) {
                Text(repo.name)
                    .font(.system(size: 15, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                Spacer(minLength: 0)
                FreeweightsBadge(text: repo.scanLabel, dot: tint)
            }
            Text(repo.detailText)
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
            if repo.scanState == .held, let codes = repo.scans.heldCodes, !codes.isEmpty {
                Text("Reason: \(codes.joined(separator: ", "))")
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }
            if repo.scanState == .scanning {
                let total = repo.scans.queued + repo.scans.scanning + repo.scans.clean + repo.scans.held + repo.scans.error
                if total > 0 {
                    Text("\(repo.scans.clean) of \(total) files cleared")
                        .font(.caption.monospacedDigit())
                        .foregroundColor(Color("TextSecondary"))
                }
            }
            if repo.bytes > 0 {
                Text(FreeweightsFormat.bytes(repo.bytes))
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }
        }
        .padding(.vertical, 4)
        .accessibilityElement(children: .combine)
    }
}

// MARK: - Report

/// Report a community repo. Needs an account; says so when signed out.
private struct FreeweightsReportSheet: View {
    let repoId: String

    @Environment(\.dismiss) private var dismiss
    @ObservedObject private var auth = AuthManager.shared
    @State private var reason: FreeweightsReportReason = .other
    @State private var note = ""
    @State private var isSending = false
    @State private var sent = false
    @State private var error: String?

    var body: some View {
        NavigationStack {
            Form {
                if !auth.isSignedIn {
                    Section {
                        FriendlyInlineStateView(
                            style: .empty,
                            icon: "person.crop.circle",
                            title: "Sign in to report",
                            message: "Reports need an account.",
                            actionTitle: "Sign in",
                            action: { auth.isPresentingAuth = true }
                        )
                    }
                } else if sent {
                    Section {
                        Text("Thanks. We got your report and will review it.")
                            .font(.subheadline)
                    }
                } else {
                    Section {
                        Picker("Reason", selection: $reason) {
                            ForEach(FreeweightsReportReason.allCases) { Text($0.label).tag($0) }
                        }
                        TextField("Details (optional)", text: $note, axis: .vertical)
                            .lineLimit(3...6)
                    } header: {
                        Text(repoId)
                    } footer: {
                        Text("For a legal notice such as a copyright claim, use the takedown address on freeweights.allternit.com.")
                    }
                    if let error {
                        Section { Text(error).font(.caption).foregroundColor(Theme.statusError) }
                    }
                    Section {
                        Button {
                            Task { await submit() }
                        } label: {
                            HStack {
                                if isSending { ProgressView() }
                                Text(isSending ? "Sending…" : "Send report")
                            }
                            .frame(maxWidth: .infinity)
                        }
                        .disabled(isSending)
                    }
                }
            }
            .navigationTitle("Report")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button(sent ? "Done" : "Cancel") { dismiss() }
                }
            }
        }
    }

    private func submit() async {
        isSending = true
        error = nil
        defer { isSending = false }
        do {
            try await FreeweightsClient.shared.report(repo: repoId, reason: reason, note: note)
            sent = true
        } catch {
            self.error = error.localizedDescription
        }
    }
}

// MARK: - Detail sheet

/// One model, laid out like the site's model page head (W2): the record
/// panel (From / Newsgroups / Subject / Date / Commit / Signed), the
/// identity face, the facts, the source badges and the `fw get` block. Then
/// "Run on this iPhone": the builds this iPhone can run with the fit for
/// each and Download (background, any network). Builds that need a computer
/// keep Send to computer; link-only entries link to the site.
struct FreeweightsDetailSheet: View {
    let summary: FreeweightsModelSummary
    let row: FreeweightsCatalogRow?
    /// Device builds from the index (nil = no device data).
    var indexQuants: [Quant]? = nil
    var onRun: ((String) -> Void)? = nil

    @Environment(\.dismiss) private var dismiss
    @Environment(\.openURL) private var openURL
    @State private var detail: FreeweightsModelDetail?
    @State private var device: FreeweightsDeviceData?
    @State private var staleAge: TimeInterval?
    @State private var downReason: String?
    @State private var isLoading = true
    @State private var isReporting = false

    private var isCommunity: Bool { detail?.isCommunity ?? summary.isCommunity }
    private var catalogRow: FreeweightsCatalogRow? { detail?.catalog ?? row }
    private var isLinkOnly: Bool { detail?.isLinkOnly ?? row?.isLinkOnly ?? false }
    /// The per-model route has file lists; the index is the offline fallback.
    private var quants: [Quant]? { device?.quants ?? indexQuants }
    private var fit: FitResult { OnDeviceRuntime.shared.fit(quants) }
    private var org: String { summary.org }
    private var name: String { catalogRow?.displayName ?? summary.name }

    var body: some View {
        NavigationStack {
            ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    crumbs
                    if isLoading && detail == nil && row == nil {
                        ProgressView().frame(maxWidth: .infinity).padding(.vertical, 24)
                    } else if detail == nil, row == nil, downReason != nil {
                        FriendlyInlineStateView(
                            style: .offline, icon: "wifi.slash",
                            title: "Freeweights is not answering",
                            message: "Couldn't load this model. Check your connection, then try again.",
                            actionTitle: "Retry", action: { Task { await load() } })
                    }
                    head
                    if isCommunity { communitySection }
                    OnDeviceRunPanel(modelId: summary.id, quants: quants, onRun: onRun.map { run in { id in dismiss(); run(id) } })
                        .id("run")
                    streamsFromDiskNote
                    if fit.k != .ok && fit.k != .slow { computerSection }
                    Button { openURL(FreeweightsClient.pageURL(for: summary.id)) } label: {
                        Label(isLinkOnly ? "Open on Freeweights" : "View on Freeweights", systemImage: "safari")
                            .font(.system(size: 15, weight: .medium))
                            .frame(maxWidth: .infinity).padding(.vertical, 11)
                    }
                    .buttonStyle(.plain)
                    .foregroundColor(Color("TextPrimary"))
                    .overlay(RoundedRectangle(cornerRadius: 12).stroke(Color("BorderStrong"), lineWidth: 1))
                    if let staleAge {
                        Text("Offline copy from \(FreeweightsSection.ageText(staleAge)).")
                            .font(.caption).foregroundColor(Color("TextSecondary"))
                    }
                }
                .padding(.horizontal, 16)
                .padding(.bottom, 28)
            }
            #if DEBUG
            // `-fw-detail-scroll run` (DEBUG): open scrolled to the builds, for screenshots.
            .task(id: device?.id) {
                if UserDefaults.standard.string(forKey: "fw-detail-scroll") == "run", device != nil {
                    try? await Task.sleep(for: .milliseconds(400))
                    proxy.scrollTo("run", anchor: .top)
                }
            }
            #endif
            }
            .background(Color("BgPrimary"))
            .navigationTitle("Model")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) { Button("Done") { dismiss() } }
            }
            .task { await load() }
            .sheet(isPresented: $isReporting) {
                FreeweightsReportSheet(repoId: summary.id)
                    .presentationDetents([.medium, .large])
            }
        }
    }

    // MARK: Head (site model page)

    private var crumbs: some View {
        HStack(spacing: 6) {
            Text(catalogRow?.section ?? summary.section ?? "alt.freeweights")
                .foregroundColor(Color("TextSecondary"))
            Text("/").foregroundColor(Color("TextTertiary"))
            Text(name).foregroundColor(.fwAccent).lineLimit(1)
        }
        .font(.system(size: 13, design: .monospaced))
        .padding(.top, 6)
    }

    private var head: some View {
        VStack(alignment: .leading, spacing: 0) {
            record
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(14)
                .background(Color("FillNeutral").opacity(0.6))
            Rectangle().fill(Color("BorderStrong")).frame(height: 1).mask(
                HStack(spacing: 3) { ForEach(0..<80, id: \.self) { _ in Rectangle().frame(width: 3) } })
            VStack(alignment: .leading, spacing: 14) {
                FwIdentityFace(model: FwFaceModel(summary: summary, row: catalogRow, id: summary.id,
                                                  sizeBytes: fit.quant?.size ?? quants?.map(\.size).min(),
                                                  formats: fit.quant.map { [$0.format] } ?? catalogRow?.formats))
                facts
                badges
                terminal
            }
            .padding(14)
        }
        .clipShape(RoundedRectangle(cornerRadius: 14))
        .overlay(RoundedRectangle(cornerRadius: 14).stroke(Color("BorderSubtle"), lineWidth: 1))
    }

    private var record: some View {
        VStack(alignment: .leading, spacing: 4) {
            recordLine("From:", "\(org) <huggingface.co/\(org)>")
            recordLine("Newsgroups:", catalogRow?.section ?? summary.section ?? "alt.freeweights")
            recordLine("Subject:", "")
            Text(name)
                .font(.system(size: 22, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
                .lineLimit(2)
                .minimumScaleFactor(0.7)
                .textSelection(.enabled)
            if let date = (catalogRow?.addedAt ?? summary.lastModified).map({ String($0.prefix(10)) }) {
                recordLine("Date:", date)
            }
            if let sha = detail?.sha ?? summary.sha { recordLine("Commit:", String(sha.prefix(7))) }
            recordLine("Signed:", isLinkOnly ? "link only, not hosted" : "Freeweights manifest ✓")
            Text(sentence)
                .font(.system(size: 14))
                .foregroundColor(Color("TextSecondary"))
                .padding(.top, 6)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func recordLine(_ label: String, _ value: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text(label).foregroundColor(Color("TextSecondary")).frame(width: 96, alignment: .leading)
            Text(value).foregroundColor(Color("TextPrimary")).lineLimit(1).truncationMode(.tail)
        }
        .font(.system(size: 12, design: .monospaced))
        .accessibilityElement(children: .combine)
    }

    /// "Text generation model with 4.2B parameters. GGUF weights, 11.3 GB in all. Published by Qwen under Apache-2.0."
    private var sentence: String {
        let task = (detail?.pipelineTag ?? summary.pipelineTag ?? catalogRow?.tasks?.first)?.replacingOccurrences(of: "-", with: " ")
        var s = (task.map { $0.prefix(1).uppercased() + $0.dropFirst() } ?? "Model")
        if let p = FwText.params(catalogRow?.params) { s += " model with \(p) parameters." } else { s += " model." }
        let fmts = (catalogRow?.formats ?? []).map { $0.prefix(1).uppercased() + $0.dropFirst() }.joined(separator: ", ")
        if !fmts.isEmpty || (catalogRow?.totalSize ?? 0) > 0 {
            s += " \(fmts.isEmpty ? "The" : fmts) weights"
            if let t = catalogRow?.totalSize, t > 0 { s += ", \(FwText.bytesText(Double(t))) in all" }
            s += "."
        }
        if let lic = catalogRow?.licenseId ?? summary.license { s += " Published by \(org) under \(lic)." }
        return s
    }

    private var facts: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("FACTS").font(.system(size: 11, weight: .medium)).tracking(1).foregroundColor(Color("TextSecondary"))
            LazyVGrid(columns: [GridItem(.flexible(), alignment: .topLeading), GridItem(.flexible(), alignment: .topLeading)], spacing: 12) {
                fact("Params", FwText.params(catalogRow?.params) ?? "not stated")
                fact("Size", fit.quant.map { "from \(FwText.deviceSize($0.size))" } ?? quants.flatMap { $0.map(\.size).min() }.map { "from \(FwText.deviceSize($0))" } ?? "Size not checked")
                fact("Formats", (catalogRow?.formats ?? []).joined(separator: ", ").nilIfEmpty ?? "see files", mono: true)
                fact("Task", (detail?.pipelineTag ?? summary.pipelineTag)?.replacingOccurrences(of: "-", with: " ") ?? "not tagged")
                fact("License", catalogRow?.licenseId ?? summary.license ?? "not stated")
                if let detail { fact("Files", isLinkOnly ? "link only" : "\(detail.filesCount) with sha256") }
            }
        }
    }

    private func fact(_ label: String, _ value: String, mono: Bool = false) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(label.uppercased()).font(.system(size: 10.5, weight: .medium)).tracking(0.8).foregroundColor(Color("TextSecondary"))
            Text(value)
                .font(mono ? .system(size: 14, design: .monospaced) : .system(size: 15))
                .foregroundColor(Color("TextPrimary"))
                .fixedSize(horizontal: false, vertical: true)
        }
        .accessibilityElement(children: .combine)
    }

    private var badges: some View {
        HStack(spacing: 8) {
            pill(isLinkOnly ? "From the publisher" : "Hosted (signed)", icon: "diamond", tint: nil)
            if let lc = catalogRow?.licenseClass {
                pill(FwText.licenseNames[lc] ?? lc, icon: nil, tint: FreeweightsFormat.licenseTint(lc))
            }
            if catalogRow?.requiresTerms == true { pill("read terms", icon: nil, tint: Theme.statusWarning) }
        }
    }

    private func pill(_ text: String, icon: String?, tint: Color?) -> some View {
        HStack(spacing: 5) {
            if let icon { Image(systemName: icon).font(.system(size: 10)) }
            if let tint { Circle().fill(tint).frame(width: 6, height: 6) }
            Text(text).font(.system(size: 13))
        }
        .foregroundColor(Color("TextPrimary"))
        .padding(.horizontal, 10).padding(.vertical, 5)
        .overlay(Capsule().stroke(Color("BorderStrong"), lineWidth: 1))
    }

    private var terminal: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 6) {
                ForEach(0..<3, id: \.self) { _ in Circle().fill(Color.white.opacity(0.25)).frame(width: 8, height: 8) }
                Text("fw").foregroundColor(Color.white.opacity(0.45)).padding(.leading, 6)
            }
            .padding(10)
            Rectangle().fill(Color.white.opacity(0.08)).frame(height: 1)
            VStack(alignment: .leading, spacing: 3) {
                if isLinkOnly {
                    term("$", "fw info \(summary.id)")
                    term("#", "link only: from the publisher, never hosted here")
                } else {
                    term("$", "fw get \(summary.id)")
                    term("#", "\((catalogRow?.totalSize).map { FwText.bytesText(Double($0)) } ?? "size pending") · \(detail?.filesCount ?? 0) files")
                    term("$", "fw verify ./\(name) \(summary.id)")
                }
            }
            .padding(12)
        }
        .font(.system(size: 12, design: .monospaced))
        .background(Color(red: 0.12, green: 0.12, blue: 0.12), in: RoundedRectangle(cornerRadius: 12))
        .environment(\.colorScheme, .dark)
        .accessibilityElement(children: .combine)
    }

    private func term(_ mark: String, _ text: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            Text(mark).foregroundColor(mark == "$" ? Color(red: 0.910, green: 0.537, blue: 0.424) : Color.white.opacity(0.4))
            Text(text).foregroundColor(mark == "$" ? Color.white.opacity(0.92) : Color.white.opacity(0.5))
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    // MARK: Community upload

    private var communitySection: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("COMMUNITY UPLOAD").font(.system(size: 11, weight: .medium)).tracking(1).foregroundColor(Color("TextSecondary"))
            Text("Uploaded by a Freeweights member, not by Allternit. Every file is scanned before it can be downloaded.")
                .font(.subheadline)
                .foregroundColor(Color("TextSecondary"))
                .fixedSize(horizontal: false, vertical: true)
            if let note = detail?.communityStatusNote {
                Text(note)
                    .font(.subheadline)
                    .foregroundColor(Theme.statusWarning)
            }
            Button {
                isReporting = true
            } label: {
                Label("Report this repo", systemImage: "flag")
                    .font(.system(size: 15, weight: .medium))
            }
            .foregroundColor(Color("TextPrimary"))
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(14)
        .overlay(RoundedRectangle(cornerRadius: 14).stroke(Color("BorderSubtle"), lineWidth: 1))
    }

    // MARK: Streams from disk (other devices only)

    /// The phone never streams (it has no disk_free_bytes); this tells the
    /// owner that a Mac or PC (My other devices, Remote) can run the model.
    @ViewBuilder
    private var streamsFromDiskNote: some View {
        if fit.k != .ok, fit.k != .slow, let s = Fit.preferredStream(device?.streamQuants ?? []) {
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 5) {
                    Circle().fill(FitTone.color(.stream)).frame(width: 7, height: 7)
                    Text("Streams from disk").font(.system(size: 12, weight: .medium)).foregroundColor(Color("TextSecondary"))
                }
                Text("On your Mac or PC: \(FwText.deviceSize(s.size)) on disk. " + (s.bestFor ?? "Not on this iPhone."))
                    .font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
            }
            .padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)
            .overlay(RoundedRectangle(cornerRadius: 14).stroke(Color("BorderSubtle"), lineWidth: 1))
            .accessibilityElement(children: .combine)
        }
    }

    // MARK: Builds that need a computer

    @ViewBuilder
    private var computerSection: some View {
        if !isLinkOnly, detail != nil || (downReason != nil && row != nil) {
            VStack(alignment: .leading, spacing: 8) {
                Text("YOUR COMPUTER").font(.system(size: 11, weight: .medium)).tracking(1).foregroundColor(Color("TextSecondary"))
                ComputerPullPanel(repoId: summary.id, revision: detail?.sha ?? summary.sha)
            }
            .padding(14)
            .overlay(RoundedRectangle(cornerRadius: 14).stroke(Color("BorderSubtle"), lineWidth: 1))
        } else if isLinkOnly {
            Text(catalogRow?.note ?? "Freeweights does not host or sign this model. Downloads come from the publisher's own release.")
                .font(.subheadline).foregroundColor(Color("TextSecondary"))
        }
    }

    private func load() async {
        isLoading = true
        defer { isLoading = false }
        async let d = FreeweightsClient.shared.detail(id: summary.id)
        async let dev = FreeweightsClient.shared.deviceData(id: summary.id)
        let (result, devResult) = await (d, dev)
        if let v = devResult.value { device = v }
        switch result {
        case .live(let v), .cached(let v, _):
            detail = v
            staleAge = result.staleAge
            downReason = nil
        case .catalogDown(let reason):
            downReason = reason
        }
    }
}

private extension String {
    var nilIfEmpty: String? { isEmpty ? nil : self }
}

// MARK: - Run on this iPhone

/// The builds this iPhone can run, the fit for each, and the local
/// Download with progress, Cancel and "Not enough storage". An installed
/// build shows Run (opens chat with it) and Remove.
struct OnDeviceRunPanel: View {
    let modelId: String
    let quants: [Quant]?
    var onRun: ((String) -> Void)?

    @ObservedObject private var rt = OnDeviceRuntime.shared
    @ObservedObject private var downloads = OnDeviceDownloads.shared
    @State private var picked: String?
    @State private var confirmRemove: OnDeviceInstall?

    /// Builds this iPhone can load (Fits or Tight fit), best first: the fit's
    /// pick leads, then smaller ones. At most 5.
    private var builds: [Quant] {
        guard let quants else { return [] }
        let usable = quants.filter { q in
            q.engine == .llamaCpp ? LlamaEngine.isLinked : q.engine == .mlx ? !rt.isSimulator : false
        }
        let fitting = Fit.candidates(rt.budget, usable).filter { [.ok, .slow].contains(Fit.fit(rt.budget, [$0]).k) }
        let ordered = fitting.sorted { Fit.needBytes($0) > Fit.needBytes($1) }
        let pick = fit.quant.flatMap { p in ordered.firstIndex { $0.name == p.name } } ?? 0
        return Array(ordered.dropFirst(pick).prefix(5))
    }

    private var fit: FitResult { rt.fit(quants) }
    private var selected: Quant? {
        builds.first { $0.name == picked } ?? fit.quant ?? builds.first
    }
    private var installed: [OnDeviceInstall] { rt.installs.filter { $0.modelId == modelId } }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text("RUN ON THIS IPHONE").font(.system(size: 11, weight: .medium)).tracking(1).foregroundColor(Color("TextSecondary"))
                Spacer()
                FitLabel(fit: fit, showSize: false, font: .system(size: 12, weight: .medium))
            }
            Text(fit.why).font(.system(size: 14)).foregroundColor(Color("TextSecondary"))
                .fixedSize(horizontal: false, vertical: true)

            ForEach(installed) { install in installedRow(install) }

            if fit.k == .ok || fit.k == .slow {
                VStack(spacing: 0) {
                    ForEach(Array(builds.enumerated()), id: \.element.name) { i, q in
                        buildRow(q)
                        if i < builds.count - 1 { Rectangle().fill(Color("BorderSubtle")).frame(height: 1).padding(.leading, 40) }
                    }
                }
                .background(Color("FillNeutral").opacity(0.5), in: RoundedRectangle(cornerRadius: 12))
                if let q = selected, !installed.contains(where: { $0.quant == q.name }) { action(q) }
                Label("Downloads keep going in the background, on any network. Files are checked against the signed Freeweights manifest (sha256) before the model loads.", systemImage: "checkmark.shield")
                    .font(.system(size: 12)).foregroundColor(Color("TextSecondary"))
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(14)
        .overlay(RoundedRectangle(cornerRadius: 14).stroke(Color("BorderSubtle"), lineWidth: 1))
        #if DEBUG
        // `-fw-autodownload` (DEBUG only): press Download once, for the
        // simulator check of download, manifest verify and install.
        .task(id: quants?.count) {
            if launchArgumentEnabled("fw-autodownload"), let q = selected, installed.isEmpty,
               fit.k == .ok || fit.k == .slow, !downloads.active.contains(OnDeviceDownloads.key(modelId, q.name)) {
                downloads.start(modelId: modelId, quant: q)
            }
        }
        #endif
        .confirmationDialog("Remove this model from your iPhone?", isPresented: Binding(get: { confirmRemove != nil }, set: { if !$0 { confirmRemove = nil } }), titleVisibility: .visible, presenting: confirmRemove) { i in
            Button("Remove \(FwText.deviceSize(Double(i.sizeBytes)))", role: .destructive) { Task { await rt.delete(i) } }
        } message: { _ in Text("You can download it again from the catalog.") }
    }

    private func buildRow(_ q: Quant) -> some View {
        let f = Fit.fit(rt.budget, [q])
        let on = q.name == selected?.name
        return Button { picked = q.name } label: {
            HStack(spacing: 12) {
                Image(systemName: on ? "largecircle.fill.circle" : "circle")
                    .font(.system(size: 18)).foregroundColor(on ? Color("TextPrimary") : Color("TextTertiary"))
                VStack(alignment: .leading, spacing: 2) {
                    Text("\(Text(q.name).fontWeight(.semibold)) with \(q.engine == .mlx ? "MLX" : "llama.cpp")")
                        .font(.system(size: 14)).foregroundColor(Color("TextPrimary"))
                    Text("\(q.format.uppercased()) · \(FwText.deviceSize(q.size)) · needs \(FwText.deviceSize(Fit.needBytes(q)))")
                        .font(.system(size: 11.5, design: .monospaced)).foregroundColor(Color("TextSecondary"))
                }
                Spacer(minLength: 4)
                FitLabel(fit: f, showSize: false)
            }
            .padding(.horizontal, 12).padding(.vertical, 10)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(on ? [.isSelected] : [])
    }

    @ViewBuilder
    private func action(_ q: Quant) -> some View {
        let key = OnDeviceDownloads.key(modelId, q.name)
        let p = rt.progress[key]
        if downloads.active.contains(key) {
            VStack(alignment: .leading, spacing: 6) {
                HStack {
                    Text(phaseText(p)).font(.system(size: 14, weight: .medium)).foregroundColor(Color("TextPrimary"))
                    Spacer()
                    if let p, p.totalBytes > 0 {
                        Text("\(FwText.deviceSize(Double(p.receivedBytes))) of \(FwText.deviceSize(Double(p.totalBytes)))")
                            .font(.caption.monospacedDigit()).foregroundColor(Color("TextSecondary"))
                    }
                }
                ProgressView(value: p?.fraction ?? 0).tint(Color("TextPrimary"))
                Button("Cancel download", role: .cancel) { downloads.cancel(modelId: modelId, quant: q.name) }
                    .font(.system(size: 14, weight: .medium))
            }
        } else {
            let free = rt.freeDiskBytes
            let tooBig = free > 0 && q.size * 1.05 + 300e6 > Double(free)
            VStack(alignment: .leading, spacing: 6) {
                Button { downloads.start(modelId: modelId, quant: q) } label: {
                    Text("Download to iPhone · \(FwText.deviceSize(q.size))")
                        .font(.system(size: 15, weight: .semibold))
                        .frame(maxWidth: .infinity).padding(.vertical, 12)
                        .foregroundColor(Color("BgPrimary"))
                        .background(Color("TextPrimary").opacity(tooBig ? 0.35 : 1), in: RoundedRectangle(cornerRadius: 12))
                }
                .buttonStyle(.plain)
                .disabled(tooBig)
                if tooBig {
                    Text("Not enough storage. Needs \(FwText.deviceSize(q.size * 1.05 + 300e6)), \(FwText.deviceSize(Double(free))) free.")
                        .font(.caption).foregroundColor(Theme.statusError)
                }
                if let error = downloads.errors[key] {
                    Text(error).font(.caption).foregroundColor(Theme.statusError).fixedSize(horizontal: false, vertical: true)
                }
            }
        }
    }

    private func phaseText(_ p: InstallProgress?) -> String {
        switch p?.phase {
        case .manifest?, nil: return "Checking the signed manifest…"
        case .downloading?: return "Downloading · \(Int(((p?.fraction ?? 0) * 100).rounded()))%"
        case .verifying?: return "Checking files…"
        case .done?: return "Ready"
        case .failed?: return "Stopped"
        }
    }

    private func installedRow(_ install: OnDeviceInstall) -> some View {
        HStack(spacing: 10) {
            Image(systemName: "checkmark.circle.fill").foregroundColor(Theme.statusSuccess)
            VStack(alignment: .leading, spacing: 1) {
                Text("\(install.quant) on this iPhone").font(.system(size: 14, weight: .medium)).foregroundColor(Color("TextPrimary"))
                Text("\(install.engine == .mlx ? "MLX" : "llama.cpp") · \(FwText.deviceSize(Double(install.sizeBytes)))")
                    .font(.system(size: 11.5, design: .monospaced)).foregroundColor(Color("TextSecondary"))
            }
            Spacer()
            Button("Remove") { confirmRemove = install }
                .font(.system(size: 14)).foregroundColor(Theme.statusError).buttonStyle(.plain)
            Button {
                let id = OnDeviceModelId.make(install)
                ModelStore.shared.userSelectedModel(id)
                onRun?(id)
            } label: {
                Text("Run").font(.system(size: 14, weight: .semibold))
                    .padding(.horizontal, 14).padding(.vertical, 7)
                    .foregroundColor(Color("BgPrimary"))
                    .background(Color("TextPrimary"), in: Capsule())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Run \(install.modelId) in chat")
        }
    }
}
// MARK: - Send to computer

/// Sends the pull to the user's computer and follows it to the end:
/// progress while downloading, then the verification label.
private struct ComputerPullPanel: View {
    let repoId: String
    let revision: String?

    @ObservedObject private var auth = AuthManager.shared
    @ObservedObject private var relay = RuntimeRelayStore.shared
    @ObservedObject private var environment = EnvironmentStore.shared
    @State private var pullId: String?
    @State private var status: ComputerModelStatus?
    @State private var isSending = false
    @State private var error: String?
    @State private var noComputer = false
    @State private var isConnectSheetPresented = false

    private enum Gate { case signedOut, checking, noComputer, unreachable, ready(String) }

    private var isSignedIn: Bool {
        #if DEBUG
        // `-skip-auth` sends the desktop bootstrap headers (APIClient), so
        // the panel treats it as signed in.
        if launchArgumentEnabled("skip-auth") { return true }
        #endif
        return auth.isSignedIn || auth.isSkippingAuth
    }

    /// Where a pull would go right now. Mirrors
    /// `InstanceConnection.runtimeAPIRelayTarget`: paired cloud environment
    /// or the cloud API host → the relayed computer; otherwise (DEBUG
    /// against a local allternit-api) this machine directly.
    private var gate: Gate {
        guard isSignedIn else { return .signedOut }
        if noComputer { return .noComputer }
        if environment.environment == .cloud {
            guard let paired = environment.pairedRuntimeId, !paired.isEmpty else { return .noComputer }
            return .ready(relay.devices.first(where: { $0.id == paired })?.displayName ?? "your computer")
        }
        guard AppConfig.apiBaseURL.host == AppConfig.cloudAPIBaseURL.host else {
            return .ready(AppConfig.apiBaseURL.host ?? "this computer")
        }
        switch relay.availability {
        case .unknown: return relay.isLoading ? .checking : .noComputer
        case .none: return .noComputer
        case .unreachable: return .unreachable
        case .available: return .ready(relay.activeDevice?.displayName ?? "your computer")
        }
    }

    var body: some View {
        Group {
            if let status {
                progressView(status)
            } else {
                switch gate {
                case .signedOut:
                    FriendlyInlineStateView(
                        style: .empty,
                        icon: "person.crop.circle",
                        title: "Sign in to send pulls",
                        message: "Pulls run on your own computer, so they need your account.",
                        actionTitle: "Sign in",
                        action: { auth.isPresentingAuth = true }
                    )
                case .checking:
                    ProgressView().frame(maxWidth: .infinity, alignment: .center)
                case .noComputer:
                    FriendlyInlineStateView(
                        style: .empty,
                        icon: "desktopcomputer",
                        title: "No computer paired",
                        message: "Pulls run on a computer. Pair one in Computers.",
                        actionTitle: "Pair a computer",
                        action: { isConnectSheetPresented = true },
                        art: .connectScene
                    )
                case .unreachable:
                    FriendlyInlineStateView(
                        style: .offline,
                        icon: "desktopcomputer.trianglebadge.exclamationmark",
                        title: "Can't reach your computer",
                        message: "Open Allternit Desktop or wake your cloud computer, then try again.",
                        actionTitle: "Fix",
                        action: { isConnectSheetPresented = true }
                    )
                case .ready(let name):
                    VStack(alignment: .leading, spacing: 10) {
                        Button {
                            Task { await send() }
                        } label: {
                            HStack {
                                if isSending { ProgressView().tint(.white) }
                                Text(isSending ? "Sending…" : "Send to computer")
                                    .font(.system(size: 15, weight: .semibold))
                            }
                            .frame(maxWidth: .infinity)
                            .padding(.vertical, 10)
                        }
                        .buttonStyle(.borderedProminent)
                        .tint(Color("AccentPrimary"))
                        .disabled(isSending)

                        Text("Downloads to \(name). The files are checked against the signed Freeweights manifest when it is available.")
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                        if let error {
                            Text(error)
                                .font(.caption)
                                .foregroundColor(Theme.statusError)
                        }
                    }
                    .padding(.vertical, 4)
                }
            }
        }
        .task {
            await relay.refresh()
            #if DEBUG
            // `-fw-autosend` (DEBUG only): press Send once the gate is open,
            // for the live check against a local allternit-api.
            if launchArgumentEnabled("fw-autosend"), case .ready = gate, status == nil {
                await send()
            }
            #endif
        }
        .sheet(isPresented: $isConnectSheetPresented, onDismiss: {
            noComputer = false
            Task { await relay.refresh() }
        }) {
            ConnectComputerSheet()
        }
    }

    @ViewBuilder
    private func progressView(_ status: ComputerModelStatus) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text(FreeweightsFormat.pullStatus(status.status))
                    .font(.system(size: 15, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                Spacer()
                if status.totalBytes > 0 {
                    Text("\(FreeweightsFormat.bytes(status.downloadedBytes)) of \(FreeweightsFormat.bytes(status.totalBytes))")
                        .font(.caption.monospacedDigit())
                        .foregroundColor(Color("TextSecondary"))
                }
            }
            if !status.isFinished {
                if let progress = status.progress {
                    ProgressView(value: progress)
                } else {
                    ProgressView(value: 0).opacity(0.4)
                }
            }
            HStack(spacing: 6) {
                Image(systemName: FreeweightsFormat.verificationIcon(status.verification))
                Text(ModelVerification.label(for: status.verification))
            }
            .font(.caption.weight(.semibold))
            .foregroundColor(FreeweightsFormat.verificationTint(status.verification))
            .accessibilityElement(children: .combine)
            if let error {
                Text(error)
                    .font(.caption)
                    .foregroundColor(Theme.statusError)
            }
        }
        .padding(.vertical, 4)
        .task(id: pullId) { await poll() }
    }

    private func send() async {
        isSending = true
        error = nil
        defer { isSending = false }
        do {
            let started = try await ComputerModelPullClient.shared.startPull(repoId: repoId, revision: revision)
            pullId = started.id
            status = (try? await ComputerModelPullClient.shared.status(id: started.id))
                ?? ComputerModelStatus(id: started.id, name: repoId, status: started.status)
        } catch RuntimeRelayError.noComputer {
            noComputer = true
        } catch {
            self.error = FriendlyErrorMessage.from(error.localizedDescription)
        }
    }

    /// Polls `GET /models` every 2 s until the pull is ready or failed.
    private func poll() async {
        guard let pullId else { return }
        var failures = 0
        while !Task.isCancelled {
            if let status, status.isFinished { return }
            try? await Task.sleep(for: .seconds(2))
            do {
                if let next = try await ComputerModelPullClient.shared.status(id: pullId) {
                    status = next
                    error = nil
                }
                failures = 0
            } catch {
                failures += 1
                if failures >= 3 {
                    self.error = "Lost touch with your computer. The pull keeps going there."
                }
            }
        }
    }
}

// MARK: - Formatting

enum FreeweightsFormat {
    static func bytes(_ value: Int64) -> String {
        ByteCountFormatter.string(fromByteCount: value, countStyle: .file)
    }

    static func params(_ value: Int64) -> String {
        let billions = Double(value) / 1_000_000_000
        if billions >= 1 { return String(format: "%.1fB", billions) }
        return String(format: "%.0fM", Double(value) / 1_000_000)
    }

    /// `alt.freeweights.qwen` → `qwen`.
    static func section(_ value: String) -> String {
        value.replacingOccurrences(of: "alt.freeweights.", with: "")
    }

    static func licenseTint(_ licenseClass: String?) -> Color {
        switch licenseClass {
        case "permissive": return Theme.statusSuccess
        case "non-commercial", "gated": return Theme.statusWarning
        default: return Theme.statusInfo
        }
    }

    static func icon(for pipelineTag: String?) -> String {
        guard let tag = pipelineTag else { return "cube" }
        if tag == "dataset" { return "tablecells" }
        if tag.contains("speech") || tag.contains("audio") { return "waveform" }
        if tag.contains("video") { return "film" }
        if tag.contains("image") { return "photo" }
        if tag.contains("feature") || tag.contains("similarity") || tag.contains("ranking") { return "point.3.connected.trianglepath.dotted" }
        return "text.bubble"
    }

    static func pullStatus(_ status: String) -> String {
        switch status {
        case "pending": return "Queued on your computer"
        case "downloading": return "Downloading"
        case "ready": return "Ready on your computer"
        case "failed": return "Pull failed"
        default: return status.capitalized
        }
    }

    static func verificationIcon(_ verification: ModelVerification?) -> String {
        switch verification?.state {
        case "freeweights_signed": return "checkmark.seal.fill"
        case "publisher_hash", "pinned": return "checkmark.shield"
        case "unverified": return "exclamationmark.triangle"
        default: return "questionmark.circle"
        }
    }

    static func verificationTint(_ verification: ModelVerification?) -> Color {
        switch verification?.state {
        case "freeweights_signed": return Theme.statusSuccess
        case "publisher_hash", "pinned": return Theme.statusInfo
        case "unverified": return Theme.statusWarning
        default: return Color("TextSecondary")
        }
    }
}

// MARK: - First-visit intro hero

/// The Freeweights intro hero: real model rows. Florence-2-base downloads with
/// a progress ring and gets an "On this iPhone" tag; gpt2 is pulled to the
/// computer. Loops; Reduce Motion shows the finished state.
struct FreeweightsIntroHero: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var pct: Double = 0
    @State private var sent = false

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            BlockText("FREEWEIGHTS", height: 8)
            HStack(spacing: 5) {
                Image(systemName: "magnifyingglass").font(.system(size: 10))
                Text("Search Freeweights").font(.system(size: 10))
            }
            .foregroundStyle(Color("TextSecondary"))
            .padding(.horizontal, 8).padding(.vertical, 6)
            .background(RoundedRectangle(cornerRadius: 9, style: .continuous).fill(PolishTokens.neutralFill))

            heroRow(name: "Florence-2-base", org: "microsoft", size: "930.3 MB", icon: "photo") {
                if pct >= 1 {
                    Image(systemName: "chevron.right").font(.system(size: 11)).foregroundStyle(Color("TextSecondary"))
                } else {
                    ZStack {
                        Circle().stroke(PolishTokens.neutralFill, lineWidth: 2.5)
                        Circle().trim(from: 0, to: pct)
                            .stroke(PolishTokens.ink, style: StrokeStyle(lineWidth: 2.5, lineCap: .round))
                            .rotationEffect(.degrees(-90))
                        Image(systemName: "arrow.down").font(.system(size: 8, weight: .semibold))
                    }
                    .frame(width: 22, height: 22)
                }
            } extra: {
                if pct >= 1 {
                    HStack(spacing: 2) {
                        Image(systemName: "iphone").font(.system(size: 7))
                        Text("On this iPhone").font(.system(size: 8.5, weight: .semibold))
                    }
                    .foregroundStyle(Color(red: 21 / 255, green: 128 / 255, blue: 61 / 255))
                    .padding(.horizontal, 5).padding(.vertical, 1)
                    .background(Capsule().fill(Color(red: 22 / 255, green: 163 / 255, blue: 74 / 255).opacity(0.12)))
                    .transition(.scale.combined(with: .opacity))
                }
            }
            heroRow(name: "gpt2", org: "openai-community", size: "5.63 GB", icon: "text.alignleft") {
                if sent {
                    HStack(spacing: 3) {
                        Image(systemName: "laptopcomputer").font(.system(size: 10))
                        Text("Sent").font(.system(size: 9.5, weight: .semibold))
                        Image(systemName: "checkmark").font(.system(size: 9, weight: .bold))
                    }
                    .foregroundStyle(Color(red: 21 / 255, green: 128 / 255, blue: 61 / 255))
                    .transition(.scale.combined(with: .opacity))
                } else {
                    HStack(spacing: 3) {
                        Image(systemName: "arrow.down").font(.system(size: 9))
                        Text("Pull").font(.system(size: 9.5, weight: .semibold))
                    }
                    .foregroundStyle(PolishTokens.ink)
                    .padding(.horizontal, 8).padding(.vertical, 3)
                    .background(Capsule().fill(Color(.systemBackground)))
                    .overlay(Capsule().strokeBorder(PolishTokens.glassBorder, lineWidth: 0.5))
                }
            } extra: { EmptyView() }
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(.systemBackground))
        .task { await run() }
    }

    private func heroRow<Right: View, Extra: View>(
        name: String, org: String, size: String, icon: String,
        @ViewBuilder right: () -> Right, @ViewBuilder extra: () -> Extra
    ) -> some View {
        HStack(spacing: 8) {
            Image(systemName: icon)
                .font(.system(size: 11))
                .foregroundStyle(Color("TextSecondary"))
                .frame(width: 24, height: 24)
                .background(RoundedRectangle(cornerRadius: 7, style: .continuous).fill(PolishTokens.neutralFill))
            VStack(alignment: .leading, spacing: 2) {
                Text(name).font(.system(size: 11, weight: .semibold))
                Text(org).font(.system(size: 9)).foregroundStyle(Color("TextSecondary"))
                HStack(spacing: 3) {
                    heroTag("MIT", dot: true)
                    heroTag("text")
                    heroTag(size)
                    extra()
                }
            }
            Spacer(minLength: 0)
            right()
        }
        .padding(.vertical, 7)
        .padding(.horizontal, 4)
    }

    private func heroTag(_ text: String, dot: Bool = false) -> some View {
        HStack(spacing: 2) {
            if dot { Circle().fill(Color(red: 34 / 255, green: 160 / 255, blue: 107 / 255)).frame(width: 4, height: 4) }
            Text(text).font(.system(size: 8.5)).foregroundStyle(Color("TextSecondary")).lineLimit(1)
        }
        .padding(.horizontal, 5).padding(.vertical, 1)
        .background(Capsule().fill(PolishTokens.neutralFill))
    }

    @MainActor private func run() async {
        if reduceMotion { pct = 1; sent = true; return }
        while !Task.isCancelled {
            pct = 0; sent = false
            try? await Task.sleep(for: .milliseconds(700))
            while pct < 1, !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(60))
                withAnimation(.linear(duration: 0.06)) { pct = min(1, pct + 0.05) }
            }
            try? await Task.sleep(for: .milliseconds(500))
            withAnimation(PolishTokens.press) { sent = true }
            try? await Task.sleep(for: .milliseconds(3000))
        }
    }
}
