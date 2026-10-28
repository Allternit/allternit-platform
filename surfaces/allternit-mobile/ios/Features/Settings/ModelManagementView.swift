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
    @ObservedObject private var modelStore = ModelStore.shared
    @StateObject private var store = ProviderManagementStore()

    var body: some View {
        ModelManagementListContent(modelStore: modelStore, store: store)
            .navigationTitle("Models & Engines")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button(action: { Task { await store.load() } }) {
                        Image(systemName: "arrow.clockwise")
                            .font(.subheadline)
                    }
                    .accessibilityLabel("Refresh")
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

    var body: some View {
        List {
            selectedModelSection
            enginesSection
            Section {
                NavigationLink("Benchmark") {
                    BenchmarkView()
                }
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .task {
            modelStore.fetchModelsIfNeeded()
            await store.load()
        }
        .refreshable {
            modelStore.fetchModelsIfNeeded(force: true)
            await store.load()
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
                    message: FriendlyErrorMessage.from(error),
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

                Picker("Reasoning effort", selection: $modelStore.selectedEffort) {
                    ForEach(ModelEffort.allCases, id: \.self) { effort in
                        Text(effort.label).tag(effort)
                    }
                }
                .font(.subheadline)
                .disabled(modelStore.selectedModel?.supportsEffort != true)
            }
        } header: {
            Text("Default Model")
        }
    }

    // MARK: - Engines

    @ViewBuilder
    private var enginesSection: some View {
        Section {
            if store.isLoading && store.engines.isEmpty {
                ProgressView()
                    .frame(maxWidth: .infinity, alignment: .center)
            } else if let error = store.error {
                FriendlyInlineStateView(
                    style: .offline,
                    icon: "wifi.slash",
                    title: "Couldn't load engines",
                    message: FriendlyErrorMessage.from(error),
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
                ForEach(store.engines) { engine in
                    EngineRow(engine: engine)
                }
            }
        } header: {
            Text("Engines")
        }
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

                return ProviderEngine(
                    id: provider.id,
                    name: provider.name,
                    kind: provider.kindLabel,
                    status: ready ? "ready" : (provider.status == "missing_key" ? "missing" : "unknown"),
                    modelCount: modelCount,
                    isReady: ready
                )
            }
        } catch {
            self.error = error.localizedDescription
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
}
