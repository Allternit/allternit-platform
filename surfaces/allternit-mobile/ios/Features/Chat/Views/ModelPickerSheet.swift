import SwiftUI

/// "Choose a model" (web ModelPickerPopover, phone sheet): search and filter
/// chips on top, a provider rail (Suggested · Favorites · each provider ·
/// Connect account) beside the model list grouped by provider, and a detail
/// footer with the Effort control for the model in focus. Rows come from the
/// backend catalog (`GET /api/v1/models`).
struct ModelPickerSheet: View {
    @ObservedObject var modelStore: ModelStore
    @Environment(\.dismiss) private var dismiss

    private enum RailSelection: Hashable { case suggested, favorites, provider(String) }
    private enum Filter: String, CaseIterable { case reasoning = "Reasoning", local = "Local" }

    @State private var query = ""
    @State private var filters: Set<Filter> = []
    @State private var rail: RailSelection = .suggested
    /// Phone: the rail can collapse to icons for a wider list.
    @State private var isRailOpen = true
    @State private var favorites: [String] = ModelFavorites.load()
    /// The row last touched — the footer describes it (else the selection).
    @State private var focusId: String? = nil
    @State private var isConnectPresented = false

    private static let defaultVisible = 20
    private static let searchLimit = 60

    var body: some View {
        VStack(spacing: 0) {
            Capsule()
                .fill(Color("BorderStrong"))
                .frame(width: 36, height: 5)
                .padding(.top, 8)
                .padding(.bottom, 6)

            searchHeader

            HStack(spacing: 0) {
                providerRail
                    .frame(width: isRailOpen ? 148 : 48)
                Rectangle().fill(Color("BorderSubtle")).frame(width: 1)
                modelList
            }
            .frame(maxHeight: .infinity)

            Rectangle().fill(Color("BorderSubtle")).frame(height: 1)
            detailFooter
                .frame(height: 150, alignment: .top)
        }
        .background(Color("BgPrimary").ignoresSafeArea())
        .presentationDetents([.fraction(0.86), .large])
        .presentationCornerRadius(28)
        .presentationDragIndicator(.hidden)
        .onAppear { modelStore.fetchModelsIfNeeded() }
        .sheet(isPresented: $isConnectPresented) {
            NavigationStack {
                ModelManagementView()
                    .toolbar {
                        ToolbarItem(placement: .topBarLeading) {
                            Button("Done") { isConnectPresented = false }
                        }
                    }
            }
        }
    }

    // MARK: - Search + filters

    private var searchHeader: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Image(systemName: "magnifyingglass")
                    .font(.system(size: 15))
                    .foregroundColor(Color("TextSecondary"))
                TextField("Search models, providers, capabilities", text: $query)
                    .font(.system(size: 16))
                    .foregroundColor(Color("TextPrimary"))
                    .autocorrectionDisabled()
                    .textInputAutocapitalization(.never)
                    .accessibilityLabel("Search models")
            }
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 8) {
                    ForEach(Filter.allCases, id: \.self) { filter in
                        let on = filters.contains(filter)
                        Button(action: {
                            if on { filters.remove(filter) } else { filters.insert(filter) }
                        }) {
                            Text(filter.rawValue)
                                .font(.system(size: 11))
                                .foregroundColor(on ? Color("TextPrimary") : Color("TextSecondary"))
                                .padding(.horizontal, 8)
                                .padding(.vertical, 3)
                                .background(on ? Color("FillNeutral") : Color.clear, in: Capsule())
                                .overlay(Capsule().stroke(on ? Color("BorderStrong") : Color("BorderSubtle"), lineWidth: 1))
                        }
                        .buttonStyle(.plain)
                        .accessibilityAddTraits(on ? [.isSelected] : [])
                    }
                }
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .overlay(alignment: .bottom) {
            Rectangle().fill(Color("BorderSubtle")).frame(height: 1)
        }
    }

    // MARK: - Provider rail

    private var providerRail: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 2) {
                railItem(
                    icon: AnyView(Image(systemName: "sidebar.left").font(.system(size: 15))),
                    label: isRailOpen ? "Hide provider names" : "Show provider names",
                    active: false
                ) {
                    withAnimation(.easeInOut(duration: 0.2)) { isRailOpen.toggle() }
                }
                railItem(icon: AnyView(Image(systemName: "square.grid.2x2").font(.system(size: 13))), label: "Suggested", active: rail == .suggested) {
                    rail = .suggested
                }
                railItem(icon: AnyView(Image(systemName: "star.fill").font(.system(size: 13))), label: "Favorites", active: rail == .favorites) {
                    rail = .favorites
                }
                railHeading("Providers")
                ForEach(providerIds, id: \.self) { pid in
                    railItem(icon: AnyView(ModelProviderMark(providerId: pid, size: 18)), label: ModelProviderNames.name(pid), active: rail == .provider(pid)) {
                        rail = .provider(pid)
                    }
                }
                railHeading("Manage")
                railItem(icon: AnyView(Image(systemName: "plus").font(.system(size: 13))), label: "Connect account", active: false) {
                    isConnectPresented = true
                }
            }
            .padding(isRailOpen ? 6 : 4)
        }
        .background(Color("FillNeutral").opacity(0.3))
    }

    @ViewBuilder
    private func railHeading(_ label: String) -> some View {
        if isRailOpen {
            Text(label.uppercased())
                .font(.system(size: 10.5, weight: .semibold))
                .tracking(0.8)
                .foregroundColor(Color("TextTertiary"))
                .padding(.horizontal, 8)
                .padding(.top, 10)
                .padding(.bottom, 4)
        } else {
            Rectangle().fill(Color("BorderSubtle")).frame(height: 1).padding(.horizontal, 8).padding(.vertical, 6)
        }
    }

    private func railItem(icon: AnyView, label: String, active: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 8) {
                icon
                    .foregroundColor(Color("TextPrimary"))
                    .frame(width: 18)
                if isRailOpen {
                    Text(label)
                        .font(.system(size: 12.5))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    Spacer(minLength: 0)
                }
            }
            .padding(.horizontal, isRailOpen ? 8 : 0)
            .frame(maxWidth: .infinity, minHeight: isRailOpen ? 32 : 40)
            .background(active ? Color("FillNeutral") : Color.clear, in: RoundedRectangle(cornerRadius: 8))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
    }

    // MARK: - Model list

    private var providerIds: [String] {
        var ids: [String] = []
        for model in modelStore.models where !ids.contains(model.provider) { ids.append(model.provider) }
        // Allternit Cloud first, local last.
        return ids.sorted { a, b in
            let ra = ModelProviderNames.rank(a), rb = ModelProviderNames.rank(b)
            return ra != rb ? ra < rb : ModelProviderNames.name(a) < ModelProviderNames.name(b)
        }
    }

    private var searching: Bool { !query.trimmingCharacters(in: .whitespaces).isEmpty }

    /// Visible rows and how many more a search would reach (web: the
    /// selection, favorites, then the catalog — Cloud first — capped).
    private var filtered: (visible: [RuntimeModel], hidden: Int) {
        let q = query.trimmingCharacters(in: .whitespaces).lowercased()
        let matches = modelStore.models.filter { model in
            switch rail {
            case .favorites: if !favorites.contains(model.id) { return false }
            case .provider(let pid): if model.provider != pid { return false }
            case .suggested: break
            }
            if filters.contains(.reasoning), !model.supportsEffort { return false }
            if filters.contains(.local), ModelProviderNames.rank(model.provider) != 2 { return false }
            guard !q.isEmpty else { return true }
            let hay = "\(model.name) \(model.id) \(ModelProviderNames.name(model.provider)) \(model.supportsEffort ? "reasoning" : "")".lowercased()
            return hay.contains(q)
        }
        if !q.isEmpty { return (Array(matches.prefix(Self.searchLimit)), max(0, matches.count - Self.searchLimit)) }
        if rail == .favorites { return (matches, 0) }
        var ordered: [RuntimeModel] = []
        func push(_ model: RuntimeModel?) {
            if let model, !ordered.contains(model) { ordered.append(model) }
        }
        push(matches.first { $0.id == modelStore.selectedModelId })
        favorites.forEach { id in push(matches.first { $0.id == id }) }
        matches.sorted { ModelProviderNames.rank($0.provider) < ModelProviderNames.rank($1.provider) }.forEach(push)
        return (Array(ordered.prefix(Self.defaultVisible)), max(0, ordered.count - Self.defaultVisible))
    }

    private var groups: [(provider: String, rows: [RuntimeModel])] {
        var order: [String] = []
        var rows: [String: [RuntimeModel]] = [:]
        for model in filtered.visible {
            if rows[model.provider] == nil { order.append(model.provider) }
            rows[model.provider, default: []].append(model)
        }
        return order
            .sorted { ModelProviderNames.rank($0) < ModelProviderNames.rank($1) }
            .map { ($0, rows[$0] ?? []) }
    }

    private var modelList: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 0) {
                if modelStore.isLoading && modelStore.models.isEmpty {
                    ProgressView()
                        .frame(maxWidth: .infinity)
                        .padding(.top, 32)
                } else if modelStore.models.isEmpty, let loadError = modelStore.loadError {
                    FriendlyStateView(
                        style: .offline,
                        icon: "wifi.slash",
                        title: "Couldn't load models",
                        message: FriendlyErrorMessage.from(loadError),
                        actionTitle: "Retry",
                        action: { modelStore.fetchModelsIfNeeded(force: true) }
                    )
                } else if groups.isEmpty {
                    Text("No models match")
                        .font(.system(size: 14))
                        .foregroundColor(Color("TextSecondary"))
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 24)
                } else {
                    ForEach(groups, id: \.provider) { group in
                        HStack(spacing: 6) {
                            ModelProviderMark(providerId: group.provider, size: 15)
                            Text(groupTitle(group.provider).uppercased())
                                .font(.system(size: 11, weight: .semibold))
                                .tracking(0.8)
                                .foregroundColor(Color("TextSecondary"))
                                .lineLimit(1)
                        }
                        .padding(.horizontal, 8)
                        .padding(.top, 10)
                        .padding(.bottom, 4)
                        ForEach(group.rows) { model in
                            modelRow(model)
                        }
                    }
                }
                if filtered.hidden > 0 {
                    Text(searching ? "More matches — keep typing to narrow the search" : "Type in the search bar to find more models")
                        .font(.system(size: 11.5))
                        .foregroundColor(Color("TextSecondary"))
                        .multilineTextAlignment(.center)
                        .frame(maxWidth: .infinity)
                        .padding(.top, 12)
                        .padding(.bottom, 8)
                }
            }
            .padding(.horizontal, 6)
            .padding(.vertical, 4)
        }
    }

    private func groupTitle(_ pid: String) -> String {
        pid == "allternit" ? "Allternit · via Allternit Cloud" : ModelProviderNames.name(pid)
    }

    private func modelRow(_ model: RuntimeModel) -> some View {
        let selected = modelStore.selectedModelId == model.id
        let favorite = favorites.contains(model.id)
        return HStack(spacing: 10) {
            ModelProviderMark(providerId: model.provider, size: 22)
            VStack(alignment: .leading, spacing: 1) {
                Text(model.name)
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                Text(subtitle(model))
                    .font(.system(size: 11))
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(1)
            }
            Spacer(minLength: 4)
            if model.supportsEffort {
                Text("Reasoning")
                    .font(.system(size: 10))
                    .foregroundColor(Color("TextSecondary"))
                    .padding(.horizontal, 4)
                    .overlay(RoundedRectangle(cornerRadius: 4).stroke(Color("BorderSubtle"), lineWidth: 1))
            }
            Button(action: { toggleFavorite(model.id) }) {
                Image(systemName: favorite ? "star.fill" : "star")
                    .font(.system(size: 13))
                    .foregroundColor(favorite ? Color("TextPrimary") : Color("TextTertiary"))
                    .frame(width: 28, height: 28)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(favorite ? "Unfavorite \(model.name)" : "Favorite \(model.name)")
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 6)
        .background(selected ? Color("FillNeutral") : Color.clear, in: RoundedRectangle(cornerRadius: 9))
        .overlay(alignment: .leading) {
            if selected {
                Rectangle().fill(Color("AccentPrimary")).frame(width: 2).padding(.vertical, 4)
            }
        }
        .contentShape(Rectangle())
        .onTapGesture { choose(model) }
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(selected ? [.isButton, .isSelected] : [.isButton])
    }

    private func subtitle(_ model: RuntimeModel) -> String {
        if let description = model.description, !description.isEmpty, description != "Allternit Cloud" { return description }
        return model.provider == "allternit" ? "via Allternit Cloud" : ModelProviderNames.name(model.provider)
    }

    private func choose(_ model: RuntimeModel) {
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
        focusId = model.id
        modelStore.userSelectedModel(model.id)
        dismiss()
    }

    private func toggleFavorite(_ id: String) {
        if let index = favorites.firstIndex(of: id) { favorites.remove(at: index) } else { favorites.append(id) }
        ModelFavorites.save(favorites)
    }

    // MARK: - Detail footer

    private var detailModel: RuntimeModel? {
        if let focusId, let model = modelStore.models.first(where: { $0.id == focusId }) { return model }
        return modelStore.selectedModel
    }

    @ViewBuilder
    private var detailFooter: some View {
        VStack(alignment: .leading, spacing: 8) {
            if let model = detailModel {
                HStack(spacing: 8) {
                    ModelProviderMark(providerId: model.provider, size: 18)
                    Text(model.name)
                        .font(.system(size: 11.5, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    Spacer(minLength: 8)
                    Text("Effort")
                    if model.supportsEffort {
                        HStack(spacing: 0) {
                            ForEach(ModelEffort.allCases, id: \.self) { effort in
                                let on = modelStore.selectedModelId == model.id && modelStore.selectedEffort == effort
                                Button(action: {
                                    modelStore.userSelectedModel(model.id)
                                    modelStore.selectedEffort = effort
                                }) {
                                    Text(effort.label)
                                        .font(.system(size: 11.5))
                                        .foregroundColor(on ? Color("TextPrimary") : Color("TextSecondary"))
                                        .padding(.horizontal, 8)
                                        .padding(.vertical, 3)
                                        .background(on ? Color("FillNeutral") : Color.clear)
                                }
                                .buttonStyle(.plain)
                                .accessibilityAddTraits(on ? [.isSelected] : [])
                            }
                        }
                        .clipShape(RoundedRectangle(cornerRadius: 8))
                        .overlay(RoundedRectangle(cornerRadius: 8).stroke(Color("BorderSubtle"), lineWidth: 1))
                        .accessibilityLabel("Thinking effort")
                    } else {
                        Text("Not adjustable")
                            .foregroundColor(Color("TextTertiary"))
                    }
                }
                Rectangle().fill(Color("BorderSubtle")).frame(height: 1)
                Text("ACCOUNT · \(ModelProviderNames.name(model.provider).uppercased())")
                    .font(.system(size: 11, weight: .semibold))
                    .tracking(0.8)
                    .lineLimit(1)
                Text(accountSummary(model.provider))
                    .lineLimit(2)
            } else {
                Text("Pick a model to see its details.")
            }
        }
        .font(.system(size: 11.5))
        .foregroundColor(Color("TextSecondary"))
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
    }

    private func accountSummary(_ pid: String) -> String {
        if pid == "allternit" {
            let usage = UsageStore.shared
            let left = usage.percentUsed.map { "\(max(0, 100 - Int($0.rounded())))% of allowance left" }
            let parts = [usage.planLabel, left].compactMap { $0 }
            return parts.isEmpty ? "Usage unavailable" : parts.joined(separator: " · ")
        }
        if ModelProviderNames.rank(pid) == 2 { return "No limits · runs on this computer" }
        return "Usage limits aren’t reported for this account yet"
    }
}

/// Starred models, shared with the web's key name.
private enum ModelFavorites {
    private static let key = "allternit:model-favorites"
    static func load() -> [String] { UserDefaults.standard.stringArray(forKey: key) ?? [] }
    static func save(_ ids: [String]) { UserDefaults.standard.set(ids, forKey: key) }
}

/// Provider display names and ordering (Allternit Cloud first, local last).
enum ModelProviderNames {
    private static let names: [String: String] = [
        "allternit": "Allternit Cloud",
        "anthropic": "Anthropic",
        "openai": "OpenAI",
        "google": "Google",
        "gemini": "Google",
        "moonshot": "Moonshot",
        "kimi": "Moonshot",
        "deepseek": "DeepSeek",
        "xai": "xAI",
        "mistral": "Mistral",
        "groq": "Groq",
        "openrouter": "OpenRouter",
        "ollama": "Ollama",
        "omlx": "oMLX",
        "local-brain": "Local Brain",
        "allternit-sidecar": "Local Brain",
    ]
    private static let local: Set<String> = ["ollama", "omlx", "allternit-sidecar", "local-brain"]

    static func name(_ pid: String) -> String { names[pid] ?? pid.capitalized }
    static func rank(_ pid: String) -> Int { pid == "allternit" ? 0 : local.contains(pid) ? 2 : 1 }
}

/// Provider mark: the A:// matrix for Allternit Cloud, else the provider's
/// initial on a neutral tile.
struct ModelProviderMark: View {
    let providerId: String
    var size: CGFloat = 18

    var body: some View {
        Group {
            if providerId == "allternit" {
                Image("MatrixLogo")
                    .resizable()
                    .scaledToFit()
                    .padding(size * 0.08)
            } else {
                Text(String(ModelProviderNames.name(providerId).prefix(1)))
                    .font(.system(size: size * 0.55, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
            }
        }
        .frame(width: size, height: size)
        .background(Color("FillNeutral"), in: RoundedRectangle(cornerRadius: size * 0.28))
        .accessibilityHidden(true)
    }
}
