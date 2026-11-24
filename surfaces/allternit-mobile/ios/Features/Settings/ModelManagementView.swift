import SwiftUI

/// Model Management parity — lists cloud providers and local engines with their
/// auth/ready status, mirroring the web's `ModelManagementView.tsx`.
///
/// Thin chrome wrapper around `ModelManagementListContent` for the
/// push-navigation entry point (Settings → Model Management). The rail-tab
/// entry point (`ModelsTabView`) wraps the same content with its own
/// hamburger-menu header instead — factored out so neither duplicates the
/// stores or list sections.
struct ModelManagementView: View {
    @Environment(\.dismiss) private var dismiss
    @ObservedObject private var modelStore = ModelStore.shared
    @StateObject private var store = ProviderManagementStore()

    var body: some View {
        ModelManagementListContent(modelStore: modelStore, store: store)
            .navigationBarTitleDisplayMode(.inline)
            .navigationBarBackButtonHidden(true)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    GlassCircleButton(systemImage: "chevron.left", label: "Back", size: 36) { dismiss() }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    GlassCircleButton(systemImage: "arrow.clockwise", label: "Refresh", size: 36) {
                        Task { await store.load() }
                    }
                    .disabled(store.isLoading)
                }
            }
    }
}

/// The actual list content (default model/effort pickers + engines list),
/// with no navigation chrome of its own — shared by `ModelManagementView`
/// (pushed from Settings) and `ModelsTabView` (rail tab root).
struct ModelManagementListContent: View {
    @ObservedObject var modelStore: ModelStore
    @ObservedObject var store: ProviderManagementStore
    @StateObject private var freeweights = FreeweightsBrowseStore()
    /// Search over the providers that aren't ready (the runtime catalog has ~300).
    @State private var providerQuery = ""

    var body: some View {
        PolishedTabPage(
            title: "Freeweights",
            description: "Open-weights models with checked licenses. Download one to this iPhone, or send a pull to your computer.",
            taller: true,
            introKey: .freeweights,
            hero: { FreeweightsIntroHero() }
        ) {
            FreeweightsSection(store: freeweights)
            VStack(alignment: .leading, spacing: 12) {
                selectedModelSection
                enginesSection
                StudioJobsSection()
                NavigationLink("Benchmark") {
                    BenchmarkView()
                }
                .font(.subheadline)
            }
            .padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: 20, style: .continuous).fill(PolishTokens.neutralFill))
        }
        .background(Color(.systemBackground))
        .freeweightsDetailSheet(freeweights)
        .task {
            modelStore.fetchModelsIfNeeded()
            await store.load()
        }
        .refreshable {
            modelStore.fetchModelsIfNeeded(force: true)
            async let engines: Void = store.load()
            async let catalog: Void = freeweights.reload()
            _ = await (engines, catalog)
        }
    }

    // MARK: - Selected model

    @ViewBuilder
    private var selectedModelSection: some View {
        Section {
            if modelStore.isLoading {
                ProgressView()
            } else if let error = modelStore.loadError {
                FriendlyInlineStateView(
                    style: .offline,
                    icon: "wifi.slash",
                    title: "Couldn't load default model",
                    message: FriendlyErrorMessage.from(error, doing: "load the default model"),
                    actionTitle: "Retry",
                    action: { modelStore.fetchModelsIfNeeded(force: true) }
                )
            } else {
                Picker("Default model", selection: Binding(
                    get: { modelStore.selectedModelId },
                    set: { modelStore.userSelectedModel($0) }
                )) {
                    Text("Backend default").tag(String?.none)
                    ForEach(modelStore.models) { model in
                        Text(model.name).tag(model.id as String?)
                    }
                }
                .font(.subheadline)

                if let thinking = modelStore.selectedModel?.effectiveThinking {
                    Picker(thinking.term, selection: Binding(
                        get: { thinking.level(modelStore.selectedEffort) },
                        set: { modelStore.selectedEffort = $0 }
                    )) {
                        ForEach(thinking.levels, id: \.id) { level in
                            Text(level.label).tag(level.id)
                        }
                    }
                    .font(.subheadline)
                }
            }
        } header: {
            Text("Default Model")
        }
    }

    // MARK: - Engines

    /// Engines the selected computer uses: three group rows (each opens its
    /// list), at most 3 "Needs you" rows, and "Other providers" (search over
    /// the rest of the ~300-row catalog). One search box filters everything.
    @ViewBuilder
    private var enginesSection: some View {
        let inUse = store.engines.filter(\.inUse)
        let others = store.engines.filter { !$0.inUse }
        let query = providerQuery.trimmingCharacters(in: .whitespaces)
        Section {
            if store.isLoading && store.engines.isEmpty {
                ProgressView()
                    .frame(maxWidth: .infinity, alignment: .center)
            } else if let error = store.error {
                FriendlyInlineStateView(
                    style: .offline,
                    icon: "wifi.slash",
                    title: "Couldn't load engines",
                    message: FriendlyErrorMessage.from(error, doing: "load engines"),
                    actionTitle: "Retry",
                    action: { Task { await store.load() } }
                )
            } else if store.engines.isEmpty {
                FriendlyInlineStateView(
                    style: .empty,
                    icon: "cpu",
                    title: "No engines configured",
                    message: "Add a model provider in Platform settings to see engines here."
                )
            } else {
                HStack(spacing: 8) {
                    Image(systemName: "magnifyingglass")
                        .foregroundColor(Color("TextSecondary"))
                    TextField("Search your providers or find one", text: $providerQuery)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .font(.subheadline)
                }
                if query.isEmpty {
                    ForEach(ProviderGroup.allCases) { group in
                        let list = inUse.filter { $0.group == group }
                        NavigationLink {
                            ProviderGroupList(title: group.title, engines: list)
                        } label: {
                            ProviderGroupRow(group: group, engines: list)
                        }
                    }
                    if !others.isEmpty {
                        NavigationLink {
                            OtherProvidersList(engines: others, connected: inUse)
                        } label: {
                            Label("Other providers", systemImage: "plus.circle")
                                .font(.subheadline)
                        }
                    }
                }
            }
        } header: {
            Text("Engines")
        }

        if store.error == nil && !store.engines.isEmpty {
            if query.isEmpty {
                let needsYou = inUse.filter(\.needsYou)
                if !needsYou.isEmpty {
                    Section {
                        ForEach(needsYou.prefix(ProviderSearch.needsYouLimit)) { engine in
                            EngineRow(engine: engine)
                        }
                        if needsYou.count > ProviderSearch.needsYouLimit {
                            Text("\(needsYou.count - ProviderSearch.needsYouLimit) more in the groups above")
                                .font(.caption)
                                .foregroundColor(Color("TextSecondary"))
                        }
                    } header: {
                        Text("Needs you · \(needsYou.count)")
                    }
                }
            } else {
                ForEach(ProviderGroup.allCases) { group in
                    let matches = ProviderSearch.matches(inUse.filter { $0.group == group }, query: query, limit: .max)
                    if !matches.isEmpty {
                        Section {
                            ForEach(matches.prefix(3)) { EngineRow(engine: $0) }
                            if matches.count > 3 {
                                NavigationLink("See all \(matches.count)") {
                                    ProviderGroupList(title: group.title, engines: matches)
                                }
                                .font(.subheadline)
                            }
                        } header: {
                            Text("\(group.title) · \(matches.count)")
                        }
                    }
                }
                Section {
                    let hits = ProviderSearch.matches(others, query: query)
                    if hits.isEmpty {
                        Text("No other provider matches “\(query)”.")
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                    }
                    ForEach(hits) { EngineRow(engine: $0) }
                } header: {
                    Text("Other providers")
                }
            }
        }
    }
}

// MARK: - Groups

/// The three tiles of the web panel, as list rows: title, summary, logos count.
private struct ProviderGroupRow: View {
    let group: ProviderGroup
    let engines: [ProviderEngine]

    var body: some View {
        let ready = engines.filter(\.isReady).count
        let waiting = engines.count - ready
        HStack(spacing: 12) {
            Image(systemName: group.icon)
                .font(.system(size: 16, weight: .medium))
                .foregroundColor(Color("TextSecondary"))
                .frame(width: 32, height: 32)
                .background(Color("BgPanel"))
                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
            VStack(alignment: .leading, spacing: 2) {
                Text(group.title)
                    .font(.system(size: 15, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                Text(engines.isEmpty ? "None yet" : "\(ready) ready" + (waiting > 0 ? " · \(waiting) need\(waiting == 1 ? "s" : "") you" : ""))
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }
            Spacer()
            Text("\(engines.count)")
                .font(.system(size: 17, weight: .semibold))
                .monospacedDigit()
                .foregroundColor(Color("TextPrimary"))
        }
        .padding(.vertical, 2)
        .accessibilityElement(children: .combine)
    }
}

/// One group's full list, with its own search.
private struct ProviderGroupList: View {
    let title: String
    let engines: [ProviderEngine]
    @State private var query = ""

    var body: some View {
        let q = query.trimmingCharacters(in: .whitespaces)
        let rows = q.isEmpty ? engines : ProviderSearch.matches(engines, query: q, limit: .max)
        List {
            if rows.isEmpty {
                Text(q.isEmpty ? "Nothing connected here yet." : "No match.")
                    .font(.subheadline)
                    .foregroundColor(Color("TextSecondary"))
            }
            ForEach(rows) { EngineRow(engine: $0) }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .searchable(text: $query, prompt: "Search \(title.lowercased())")
        .navigationTitle(title)
        .navigationBarTitleDisplayMode(.inline)
    }
}

/// The rest of the runtime catalog: search only, first 8 matches.
private struct OtherProvidersList: View {
    let engines: [ProviderEngine]
    let connected: [ProviderEngine]
    @State private var query = ""

    var body: some View {
        let q = query.trimmingCharacters(in: .whitespaces)
        let hits = ProviderSearch.matches(engines, query: q)
        let already = q.isEmpty ? [] : ProviderSearch.matches(connected, query: q, limit: .max)
        List {
            Section {
                ForEach(hits) { EngineRow(engine: $0) }
                if !already.isEmpty {
                    Text("Already connected: " + already.map(\.name).joined(separator: ", "))
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
                if hits.isEmpty {
                    Text(q.isEmpty ? "Type a provider name." : "No other provider matches.")
                        .font(.subheadline)
                        .foregroundColor(Color("TextSecondary"))
                }
            } footer: {
                Text("\(engines.count) providers. Connect one on your computer in Settings → Account → Models & providers.")
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .searchable(text: $query, prompt: "Search \(engines.count) providers")
        .navigationTitle("Other providers")
        .navigationBarTitleDisplayMode(.inline)
    }
}

// MARK: - Engine row

private struct EngineRow: View {
    let engine: ProviderEngine

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: iconName)
                .font(.system(size: 18, weight: .medium))
                .foregroundColor(Color("TextSecondary"))
                .frame(width: 40, height: 40)
                .background(Color("BgPanel"))
                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))

            VStack(alignment: .leading, spacing: 2) {
                Text(engine.name)
                    .font(.system(size: 15, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))

                HStack(spacing: 6) {
                    Text(engine.kind)
                        .font(.caption)
                    if engine.modelCount > 0 {
                        Text("·")
                            .font(.caption)
                        Text("\(engine.modelCount) models")
                            .font(.caption)
                    }
                }
                .foregroundColor(Color("TextSecondary"))
            }

            Spacer()

            Text(engine.status.uppercased())
                .font(.system(size: 10, weight: .bold))
                .foregroundColor(engine.isReady ? Theme.statusSuccess : Theme.statusError)
                .padding(.horizontal, 8)
                .padding(.vertical, 4)
                .background(engine.isReady ? Theme.statusSuccess.opacity(0.1) : Theme.statusError.opacity(0.1))
                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusSM))
        }
        .padding(.vertical, 4)
    }

    private var iconName: String {
        switch engine.id {
        case "openai", "azure-openai": return "circle.hexagongrid"
        case "anthropic": return "a.circle"
        case "ollama": return "cpu"
        default: return "terminal"
        }
    }
}

// MARK: - Store

@MainActor
final class ProviderManagementStore: ObservableObject {
    @Published private(set) var engines: [ProviderEngine] = []
    @Published private(set) var isLoading = false
    @Published var error: String? = nil

    private let client = ProviderManagementClient.shared

    func load() async {
        isLoading = true
        error = nil
        defer { isLoading = false }

        do {
            async let providersTask = client.listProviders()
            async let authTask = client.listAuthStatus()
            async let ollamaTask = client.ollamaLiveStatus()

            let (providers, authStatuses, ollama) = try await (providersTask, authTask, ollamaTask)

            let authMap = Dictionary(uniqueKeysWithValues: authStatuses.map { ($0.providerId, $0) })

            engines = providers.map { provider in
                let auth = authMap[provider.id]
                let isOllama = provider.id == "ollama"
                let ready: Bool
                let modelCount: Int
                if isOllama {
                    ready = ollama.running
                    modelCount = ollama.models.count
                } else if let auth {
                    ready = auth.authenticated || auth.status == "not_required" || provider.status == "active"
                    modelCount = provider.models.count
                } else {
                    ready = provider.status == "active" || provider.status == "online"
                    modelCount = provider.models.count
                }

                // An installed CLI that isn't signed in is in use and needs you;
                // the rest of the catalog is only found by search.
                let installedCLI = provider.isSubprocess && auth != nil && auth?.status != "not_installed"
                return ProviderEngine(
                    id: provider.id,
                    name: provider.name,
                    kind: provider.kindLabel,
                    status: ready ? "ready" : (provider.status == "missing_key" ? "missing" : "unknown"),
                    modelCount: modelCount,
                    isReady: ready,
                    group: provider.isSubprocess ? .cli : (provider.isLocal ? .local : .api),
                    inUse: ready || installedCLI
                )
            }
        } catch {
            self.error = error.localizedDescription
        }
    }
}

/// Name/id search over providers: names that start with the query first, at most `limit`.
enum ProviderSearch {
    static let limit = 8
    static let needsYouLimit = 3

    static func matches(_ engines: [ProviderEngine], query: String, limit: Int = ProviderSearch.limit) -> [ProviderEngine] {
        let q = query.trimmingCharacters(in: .whitespaces).lowercased()
        guard !q.isEmpty else { return [] }
        let hits = engines.filter { $0.name.lowercased().contains(q) || $0.id.lowercased().contains(q) }
        func starts(_ e: ProviderEngine) -> Bool { e.name.lowercased().hasPrefix(q) || e.id.lowercased().hasPrefix(q) }
        return Array(hits.sorted {
            starts($0) != starts($1) ? starts($0) : $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending
        }.prefix(limit))
    }
}

enum ProviderGroup: String, CaseIterable, Identifiable, Sendable {
    case cli, api, local

    var id: String { rawValue }

    var title: String {
        switch self {
        case .cli: return "Coding tools"
        case .api: return "API providers"
        case .local: return "On this computer"
        }
    }

    var icon: String {
        switch self {
        case .cli: return "terminal"
        case .api: return "key"
        case .local: return "cpu"
        }
    }
}

struct ProviderEngine: Identifiable, Sendable {
    let id: String
    let name: String
    let kind: String
    let status: String
    let modelCount: Int
    let isReady: Bool
    let group: ProviderGroup
    /// Ready, or an installed CLI waiting for sign-in. Everything else is catalog.
    let inUse: Bool

    var needsYou: Bool { inUse && !isReady }
}
