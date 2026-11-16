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

    private let client: FreeweightsClient
    private var searchTask: Task<Void, Never>?
    private static let pageSize = 8

    init(client: FreeweightsClient = .shared) {
        self.client = client
    }

    func loadIfNeeded() async {
        guard !hasLoaded else { return }
        await reload()
    }

    func reload() async {
        searchTask?.cancel()
        isLoading = true
        defer { isLoading = false; hasLoaded = true }
        async let pageLoad = client.search(query: query, limit: Self.pageSize)
        async let catalogLoad = client.catalogRows()
        async let mineLoad: Void = reloadMine()
        let (page, rows, _) = await (pageLoad, catalogLoad, mineLoad)
        if let rows = rows.value { catalog = rows }
        apply(page, appending: false)
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
        Group {
            browseSection
            if auth.isSignedIn, !store.myRepos.isEmpty || store.myReposFailed {
                myUploadsSection
            }
        }
        .task(id: auth.isSignedIn) { await store.reloadMine() }
    }

    private var myUploadsSection: some View {
        Section {
            if store.myRepos.isEmpty {
                Text("Couldn't load your uploads. Pull to refresh.")
                    .font(.subheadline)
                    .foregroundColor(Color("TextSecondary"))
            }
            ForEach(store.myRepos) { repo in
                FreeweightsMyRepoRow(repo: repo)
            }
        } header: {
            Text("My uploads")
        } footer: {
            Text("Upload from the Freeweights site or the fw command on your computer. This list follows each scan.")
        }
    }

    private var browseSection: some View {
        Section {
            searchField

            if store.isLoading && store.results.isEmpty {
                ProgressView()
                    .frame(maxWidth: .infinity, alignment: .center)
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
                    message: "Try a family name like qwen, gemma or deepseek."
                )
            } else {
                ForEach(store.results) { model in
                    Button { store.selected = model } label: {
                        FreeweightsRow(model: model, row: store.catalog[model.id])
                    }
                    .buttonStyle(.plain)
                }
                if store.nextCursor != nil {
                    Button {
                        Task { await store.loadMore() }
                    } label: {
                        HStack {
                            Text("Show more")
                                .font(.subheadline.weight(.medium))
                            if store.isLoadingMore { ProgressView().padding(.leading, 4) }
                        }
                        .frame(maxWidth: .infinity, alignment: .center)
                    }
                    .disabled(store.isLoadingMore)
                }
            }
        } header: {
            Text("Freeweights")
        } footer: {
            if let age = store.staleAge {
                Text("Offline copy from \(Self.ageText(age)). Pull to refresh.")
            } else {
                Text("Open-weights models with checked licenses. Pulls download to your computer, not this phone.")
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
                .font(.subheadline)
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
    }

    static func ageText(_ age: TimeInterval) -> String {
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .full
        return formatter.localizedString(for: Date().addingTimeInterval(-age), relativeTo: Date())
    }
}

extension View {
    /// Presents the Freeweights detail sheet for `store.selected`. Attach
    /// to the List that holds `FreeweightsSection`.
    func freeweightsDetailSheet(_ store: FreeweightsBrowseStore) -> some View {
        modifier(FreeweightsDetailSheetModifier(store: store))
    }
}

private struct FreeweightsDetailSheetModifier: ViewModifier {
    @ObservedObject var store: FreeweightsBrowseStore

    func body(content: Content) -> some View {
        content.sheet(item: $store.selected) { model in
            FreeweightsDetailSheet(summary: model, row: store.catalog[model.id])
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
                .background(Color("BgPanel"))
                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))

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
        .padding(.vertical, 4)
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
        .background(Color("TextSecondary").opacity(0.1))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusSM / 2))
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

/// License, size and files for one model, plus Send to computer (or a link
/// to the site for link-only entries).
struct FreeweightsDetailSheet: View {
    let summary: FreeweightsModelSummary
    let row: FreeweightsCatalogRow?

    @Environment(\.dismiss) private var dismiss
    @Environment(\.openURL) private var openURL
    @State private var detail: FreeweightsModelDetail?
    @State private var staleAge: TimeInterval?
    @State private var downReason: String?
    @State private var isLoading = true
    @State private var isReporting = false

    private var isCommunity: Bool { detail?.isCommunity ?? summary.isCommunity }
    private var catalogRow: FreeweightsCatalogRow? { detail?.catalog ?? row }
    private var isLinkOnly: Bool { detail?.isLinkOnly ?? row?.isLinkOnly ?? false }

    var body: some View {
        NavigationStack {
            List {
                Section {
                    VStack(alignment: .leading, spacing: 6) {
                        Text(catalogRow?.displayName ?? summary.name)
                            .font(.system(.title3, design: .serif))
                            .fontWeight(.medium)
                            .foregroundColor(Color("TextPrimary"))
                        Text(summary.id)
                            .font(.caption.monospaced())
                            .foregroundColor(Color("TextSecondary"))
                            .textSelection(.enabled)
                    }
                    .padding(.vertical, 4)
                }

                if isLoading && detail == nil {
                    Section { ProgressView().frame(maxWidth: .infinity, alignment: .center) }
                } else if detail == nil, let downReason {
                    Section {
                        FriendlyInlineStateView(
                            style: .offline,
                            icon: "wifi.slash",
                            title: "Freeweights is not answering",
                            message: "Couldn't load this model. Check your connection, then try again.",
                            actionTitle: "Retry",
                            action: { Task { await load() } }
                        )
                    }
                }

                factsSection

                if isCommunity {
                    Section {
                        Text("Uploaded by a Freeweights member, not by Allternit. Every file is scanned before it can be downloaded.")
                            .font(.subheadline)
                            .foregroundColor(Color("TextSecondary"))
                        if let note = detail?.communityStatusNote {
                            Text(note)
                                .font(.subheadline)
                                .foregroundColor(Theme.statusWarning)
                        }
                        Button {
                            isReporting = true
                        } label: {
                            Label("Report this repo", systemImage: "flag")
                        }
                    } header: {
                        Text("Community upload")
                    }
                }

                if isLinkOnly {
                    Section {
                        Text(catalogRow?.note ?? "Freeweights does not host or sign this model. Downloads come from the publisher's own release.")
                            .font(.subheadline)
                            .foregroundColor(Color("TextSecondary"))
                        Button {
                            openURL(FreeweightsClient.pageURL(for: summary.id))
                        } label: {
                            Label("Open on Freeweights", systemImage: "arrow.up.right.square")
                        }
                    } header: {
                        Text("Link only")
                    }
                } else if detail != nil || (downReason != nil && row != nil) {
                    // Wait for the detail so the pull is pinned to the
                    // manifest commit; with the catalog down, pull `main`.
                    Section {
                        ComputerPullPanel(repoId: summary.id, revision: detail?.sha ?? summary.sha)
                    } header: {
                        Text("Your computer")
                    }
                    Section {
                        Button {
                            openURL(FreeweightsClient.pageURL(for: summary.id))
                        } label: {
                            Label("View on Freeweights", systemImage: "safari")
                        }
                    }
                }
            }
            .listStyle(.insetGrouped)
            .scrollContentBackground(.hidden)
            .background(Color("BgPrimary"))
            .navigationTitle("Model")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Done") { dismiss() }
                }
            }
            .task { await load() }
            .sheet(isPresented: $isReporting) {
                FreeweightsReportSheet(repoId: summary.id)
                    .presentationDetents([.medium, .large])
            }
        }
    }

    @ViewBuilder
    private var factsSection: some View {
        let lic = catalogRow?.licenseId ?? detail?.license ?? summary.license
        Section {
            if let lic {
                fact("License", "\(lic)\(catalogRow?.licenseClass.map { " (\($0))" } ?? "")")
            }
            if let size = catalogRow?.totalSize, size > 0 {
                fact("Size", FreeweightsFormat.bytes(size))
            }
            if let detail {
                fact("Files", isLinkOnly ? "Link only" : "\(detail.filesCount), each with a sha256")
            }
            if let params = catalogRow?.params, params > 0 {
                fact("Parameters", FreeweightsFormat.params(params))
            }
            if let task = detail?.pipelineTag ?? summary.pipelineTag {
                fact("Task", task.replacingOccurrences(of: "-", with: " "))
            }
            if let section = detail?.section ?? summary.section {
                fact("Section", FreeweightsFormat.section(section))
            }
            if let commit = detail?.sha {
                fact("Pinned commit", String(commit.prefix(12)), mono: true)
            }
        } header: {
            Text("Facts")
        } footer: {
            if let staleAge {
                Text("Offline copy from \(FreeweightsSection.ageText(staleAge)).")
            }
        }
    }

    private func fact(_ label: String, _ value: String, mono: Bool = false) -> some View {
        HStack(alignment: .firstTextBaseline) {
            Text(label)
                .font(.subheadline)
                .foregroundColor(Color("TextSecondary"))
            Spacer(minLength: 12)
            Text(value)
                .font(mono ? .subheadline.monospaced() : .subheadline)
                .foregroundColor(Color("TextPrimary"))
                .multilineTextAlignment(.trailing)
        }
    }

    private func load() async {
        isLoading = true
        defer { isLoading = false }
        let result = await FreeweightsClient.shared.detail(id: summary.id)
        switch result {
        case .live(let d), .cached(let d, _):
            detail = d
            staleAge = result.staleAge
            downReason = nil
        case .catalogDown(let reason):
            downReason = reason
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
                        action: { isConnectSheetPresented = true }
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
