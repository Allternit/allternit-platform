import SwiftUI

/// The model sheet (web ModelPickerPopover, phone layout): search, the
/// provider overview strip with each account's limit ring, the current model
/// with its own thinking slider, then every model grouped by provider with no
/// cap. Rows come from the catalog (`GET /api/v1/models`), each carrying its
/// thinking levels (model-thinking.json).
struct ModelPickerSheet: View {
    @ObservedObject var modelStore: ModelStore
    @ObservedObject private var quotas = ProviderQuotaStore.shared
    @ObservedObject private var usage = UsageStore.shared
    /// Connected plans (plan/…, copilot/…) and ChatGPT on this phone feed
    /// `modelStore.personModels`; observed so new rows show at once.
    @ObservedObject private var connections = ConnectionsStore.shared
    @ObservedObject private var chatgpt = ChatGPTDeviceStore.shared
    @Environment(\.dismiss) private var dismiss

    private enum Scope: Hashable { case all, favorites, provider(String) }

    @State private var query = ""
    @State private var scope: Scope = .all
    @State private var favorites: [String] = ModelFavorites.load()
    @State private var isConnectPresented = false

    var body: some View {
        VStack(spacing: 0) {
            Capsule()
                .fill(Color("BorderStrong"))
                .frame(width: 36, height: 5)
                .padding(.top, 8)
                .padding(.bottom, 6)
            header
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    if !searching, scope == .all, let current = modelStore.selectedModel {
                        currentModel(current)
                            .padding(.bottom, 6)
                    }
                    modelList
                }
                .padding(.horizontal, 10)
                .padding(.bottom, 24)
            }
        }
        .background(Color("BgPrimary").ignoresSafeArea())
        .presentationDetents([.fraction(0.86), .large])
        .presentationCornerRadius(28)
        .presentationDragIndicator(.hidden)
        .onAppear {
            modelStore.fetchModelsIfNeeded()
            usage.fetchUsageIfNeeded()
            quotas.load(providerIds)
            // Plans connect on cloud-api, so they show with no computer at all.
            Task {
                if !connections.loaded { await connections.load() }
                await chatgpt.loadModels()
            }
        }
        .onChange(of: modelStore.models) { _, _ in quotas.load(providerIds) }
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

    // MARK: - Header: search + provider overview

    private var header: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                Image(systemName: "magnifyingglass")
                    .font(.system(size: 15))
                    .foregroundColor(Color("TextSecondary"))
                TextField("Search every model", text: $query)
                    .font(.system(size: 16))
                    .foregroundColor(Color("TextPrimary"))
                    .autocorrectionDisabled()
                    .textInputAutocapitalization(.never)
                    .accessibilityLabel("Search models")
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 9)
            .background(Color("FillNeutral"), in: Capsule())
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 7) {
                    overviewChip(icon: AnyView(Image(systemName: "square.grid.2x2").font(.system(size: 14))), title: "All", detail: "\(offered.count) models", active: scope == .all) { scope = .all }
                    overviewChip(icon: AnyView(Image(systemName: "star.fill").font(.system(size: 13)).foregroundColor(Color("AccentPrimary"))), title: "Favorites", detail: "\(favorites.count)", active: scope == .favorites) { scope = .favorites }
                    ForEach(providerIds, id: \.self) { pid in
                        let worst = quotas.worst(pid)
                        let count = offered.filter { $0.provider == pid }.count
                        overviewChip(
                            icon: AnyView(LimitRingView(window: worst, size: 28) { ModelProviderMark(providerId: pid, size: 15) }),
                            title: ModelProviderNames.name(pid),
                            detail: worst.map { $0.usedRatio >= 0.95 ? "limit reached" : "\(count) · \(Int(((1 - $0.usedRatio) * 100).rounded()))% left" } ?? "\(count) models",
                            active: scope == .provider(pid)
                        ) { scope = .provider(pid) }
                    }
                    overviewChip(icon: AnyView(Image(systemName: "plus").font(.system(size: 13))), title: "Connect", detail: "account", active: false) { isConnectPresented = true }
                }
            }
        }
        .padding(.horizontal, 12)
        .padding(.bottom, 10)
        .overlay(alignment: .bottom) { Rectangle().fill(Color("BorderSubtle")).frame(height: 1) }
    }

    private func overviewChip(icon: AnyView, title: String, detail: String, active: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 8) {
                icon.frame(width: 28, height: 28).foregroundColor(Color("TextPrimary"))
                VStack(alignment: .leading, spacing: 0) {
                    Text(title).font(.system(size: 12, weight: .semibold)).foregroundColor(Color("TextPrimary"))
                    Text(detail).font(.system(size: 10.5)).foregroundColor(Color("TextTertiary"))
                }
            }
            .padding(.leading, 6)
            .padding(.trailing, 10)
            .padding(.vertical, 6)
            .background(Color("FillNeutral"), in: RoundedRectangle(cornerRadius: 14))
            .overlay(RoundedRectangle(cornerRadius: 14).stroke(active ? Color("AccentPrimary") : Color("BorderSubtle"), lineWidth: 1))
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(active ? [.isSelected] : [])
    }

    // MARK: - Current model + its thinking levels

    private func currentModel(_ model: RuntimeModel) -> some View {
        let windows = model.provider == "allternit" ? [ProviderQuotaStore.cloudWindow()].compactMap { $0 } : (quotas.byProvider[model.provider]?.windows ?? [])
        return VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .top, spacing: 12) {
                LimitRingView(window: windows.max { $0.usedRatio < $1.usedRatio }, size: 52) {
                    ModelProviderMark(providerId: model.provider, size: 22)
                }
                VStack(alignment: .leading, spacing: 4) {
                    Text(model.name).font(.system(size: 15, weight: .semibold)).foregroundColor(Color("TextPrimary"))
                    Text(accountSummary(model.provider)).font(.system(size: 11.5)).foregroundColor(Color("TextTertiary")).lineLimit(2)
                    ForEach(windows) { w in
                        VStack(alignment: .leading, spacing: 3) {
                            HStack {
                                Text(w.displayLabel).font(.system(size: 11)).foregroundColor(Color("TextSecondary"))
                                Spacer()
                                Text("\(Int((w.usedRatio * 100).rounded()))%\(w.resetDate.map { " · resets " + $0.formatted(.dateTime.weekday().hour().minute()) } ?? "")")
                                    .font(.system(size: 11)).foregroundColor(Color("TextPrimary")).monospacedDigit()
                            }
                            ProgressView(value: w.usedRatio).tint(LimitRingView<EmptyView>.color(w.usedRatio))
                        }
                    }
                }
            }
            if let thinking = model.effectiveThinking {
                ThinkingSliderView(thinking: thinking, value: thinking.level(modelStore.selectedEffort)) { level in
                    modelStore.selectedEffort = level
                }
                if thinking.fast {
                    Toggle(isOn: $modelStore.fastMode) {
                        HStack(spacing: 10) {
                            Image(systemName: "bolt.fill")
                                .font(.system(size: 13))
                                .foregroundColor(modelStore.fastMode ? .white : Color("TextSecondary"))
                                .frame(width: 28, height: 28)
                                .background(modelStore.fastMode ? Color(red: 0.851, green: 0.467, blue: 0.341) : Color("FillNeutral"), in: Circle())
                            VStack(alignment: .leading, spacing: 1) {
                                Text("Fast").font(.system(size: 13, weight: .semibold)).foregroundColor(Color("TextPrimary"))
                                Text(modelStore.fastMode ? "Priority · faster replies, more usage" : "Standard speed")
                                    .font(.system(size: 11)).foregroundColor(Color("TextTertiary"))
                            }
                        }
                    }
                    .tint(Color(red: 0.851, green: 0.467, blue: 0.341))
                    .padding(10)
                    .background(Color("FillNeutral"), in: RoundedRectangle(cornerRadius: 14))
                    .overlay(RoundedRectangle(cornerRadius: 14).stroke(Color("BorderSubtle"), lineWidth: 1))
                }
            } else {
                Text("This model has no thinking setting.")
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextTertiary"))
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(12)
                    .overlay(RoundedRectangle(cornerRadius: 14).stroke(Color("BorderSubtle"), lineWidth: 1))
            }
        }
        .padding(12)
        .background(Color("FillNeutral").opacity(0.5), in: RoundedRectangle(cornerRadius: 18))
        .overlay(RoundedRectangle(cornerRadius: 18).stroke(Color("BorderSubtle"), lineWidth: 1))
        .padding(.top, 10)
    }

    // MARK: - Every model, grouped by provider

    private var providerIds: [String] {
        var ids: [String] = []
        for model in modelStore.chatModels where !ids.contains(model.provider) { ids.append(model.provider) }
        return ids.sorted { a, b in
            let ra = ModelProviderNames.rank(a), rb = ModelProviderNames.rank(b)
            return ra != rb ? ra < rb : ModelProviderNames.name(a) < ModelProviderNames.name(b)
        }
    }

    /// Catalog rows the picker offers: hidden (retired) ones only when current.
    private var offered: [RuntimeModel] {
        modelStore.chatModels.filter { !$0.isHidden || $0.id == modelStore.selectedModelId }
    }

    private var searching: Bool { !query.trimmingCharacters(in: .whitespaces).isEmpty }

    /// Every model in scope that matches the search. Nothing is hidden.
    private var matches: [RuntimeModel] {
        let q = query.trimmingCharacters(in: .whitespaces).lowercased()
        return offered.filter { model in
            switch scope {
            case .favorites: if !favorites.contains(model.id) { return false }
            case .provider(let pid): if model.provider != pid { return false }
            case .all: break
            }
            guard !q.isEmpty else { return true }
            let hay = "\(model.name) \(model.id) \(ModelProviderNames.name(model.provider)) \(model.thinking?.term ?? "")".lowercased()
            return hay.contains(q)
        }
    }

    private var groups: [(provider: String, rows: [RuntimeModel])] {
        let rows = Dictionary(grouping: matches, by: \.provider)
        return providerIds.compactMap { pid in rows[pid].map { (pid, Self.rankOrdered($0)) } }
    }

    /// Ranked models first by ascending rank; unranked keep their order after.
    private static func rankOrdered(_ rows: [RuntimeModel]) -> [RuntimeModel] {
        let indexed = Array(rows.enumerated())
        return indexed.sorted { a, b in
            switch (a.element.rank, b.element.rank) {
            case let (x?, y?): return x != y ? x < y : a.offset < b.offset
            case (_?, nil): return true
            case (nil, _?): return false
            default: return a.offset < b.offset
            }
        }.map(\.element)
    }

    @ViewBuilder
    private var modelList: some View {
        if modelStore.isLoading && modelStore.chatModels.isEmpty {
            ProgressView().frame(maxWidth: .infinity).padding(.top, 32)
        } else if modelStore.chatModels.isEmpty, let loadError = modelStore.loadError {
            FriendlyStateView(
                style: .offline,
                icon: "wifi.slash",
                title: "Couldn't load models",
                message: FriendlyErrorMessage.from(loadError),
                actionTitle: "Retry",
                action: { modelStore.fetchModelsIfNeeded(force: true) }
            )
        } else if groups.isEmpty {
            Text(scope == .favorites ? "Star a model to keep it here." : "No models match")
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
                    Spacer()
                    Text("\(group.rows.count)").font(.system(size: 11)).foregroundColor(Color("TextTertiary"))
                }
                .padding(.horizontal, 6)
                .padding(.top, 14)
                .padding(.bottom, 4)
                VStack(spacing: 0) {
                    ForEach(group.rows) { model in
                        modelRow(model)
                        if model.id != group.rows.last?.id {
                            Rectangle().fill(Color("BorderSubtle")).frame(height: 1).padding(.leading, 44)
                        }
                    }
                }
                .background(Color("FillNeutral").opacity(0.5), in: RoundedRectangle(cornerRadius: 14))
            }
        }
    }

    private func groupTitle(_ pid: String) -> String {
        pid == "allternit" ? "Allternit · via Allternit Cloud" : ModelProviderNames.name(pid)
    }

    private func modelRow(_ model: RuntimeModel) -> some View {
        let selected = modelStore.selectedModelId == model.id
        let favorite = favorites.contains(model.id)
        return HStack(spacing: 10) {
            ModelProviderMark(providerId: model.provider, size: 24)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(model.name)
                        .font(.system(size: 14))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    if model.isDefault {
                        Text("Default")
                            .font(.system(size: 10.5))
                            .foregroundColor(Color("TextTertiary"))
                            .accessibilityHidden(true)
                    }
                }
                HStack(spacing: 6) {
                    Text(model.shortName)
                        .font(.system(size: 10.5, design: .monospaced))
                        .foregroundColor(Color("TextTertiary"))
                        .lineLimit(1)
                    if let thinking = model.thinking { ThinkingChip(thinking: thinking) }
                }
            }
            Spacer(minLength: 4)
            if selected {
                Image(systemName: "checkmark").font(.system(size: 13, weight: .semibold)).foregroundColor(Color("AccentPrimary"))
            }
            Button(action: { toggleFavorite(model.id) }) {
                Image(systemName: favorite ? "star.fill" : "star")
                    .font(.system(size: 14))
                    .foregroundColor(favorite ? Color("AccentPrimary") : Color("TextTertiary"))
                    .frame(width: 30, height: 30)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(favorite ? "Unfavorite \(model.name)" : "Favorite \(model.name)")
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 8)
        .contentShape(Rectangle())
        .onTapGesture { choose(model) }
        .accessibilityElement(children: .combine)
        .accessibilityLabel(model.isDefault ? "\(model.name), default" : model.name)
        .accessibilityAddTraits(selected ? [.isButton, .isSelected] : [.isButton])
    }

    private func choose(_ model: RuntimeModel) {
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
        modelStore.userSelectedModel(model.id)
        // Stay open on a pick from the full list so the level can be set too.
        scope = .all
        query = ""
    }

    private func toggleFavorite(_ id: String) {
        if let index = favorites.firstIndex(of: id) { favorites.remove(at: index) } else { favorites.append(id) }
        ModelFavorites.save(favorites)
    }

    private func accountSummary(_ pid: String) -> String {
        if PersonModels.providerNames[pid] != nil {
            return pid == "chatgpt-phone"
                ? "Your ChatGPT plan, from this iPhone · only messages you send"
                : "Your own plan · only messages you send in chat"
        }
        if pid == "allternit" {
            let left = usage.percentUsed.map { "\(max(0, 100 - Int($0.rounded())))% of allowance left" }
            let parts = ["Allternit Cloud", usage.planLabel, left].compactMap { $0 }
            return parts.joined(separator: " · ")
        }
        if ModelProviderNames.rank(pid) == 2 { return "Runs on this computer · no limits" }
        let plan = quotas.byProvider[pid]?.plan
        let name = ModelProviderNames.name(pid)
        if quotas.byProvider[pid] == nil { return "\(name) · limits not reported" }
        return [name, plan].compactMap { $0 }.joined(separator: " · ")
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
    ].merging(PersonModels.providerNames) { a, _ in a }
    private static let local: Set<String> = ["ollama", "omlx", "allternit-sidecar", "local-brain"]

    static func name(_ pid: String) -> String { names[pid] ?? pid.capitalized }
    /// The person's own plans first, then Allternit Cloud, providers, local.
    static func rank(_ pid: String) -> Int {
        if PersonModels.providerNames[pid] != nil { return -1 }
        return pid == "allternit" ? 0 : local.contains(pid) ? 2 : 1
    }

    /// The letter on a provider's tile: the vendor's, not "Your …".
    static func mark(_ pid: String) -> String {
        switch pid {
        case "plan-claude": return "C"
        case "plan-chatgpt", "chatgpt-phone": return "G"
        case "plan-grok": return "X"
        case "plan-kimi": return "K"
        case "copilot": return "G"
        default: return String(name(pid).prefix(1))
        }
    }
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
                Text(ModelProviderNames.mark(providerId))
                    .font(.system(size: size * 0.55, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
            }
        }
        .frame(width: size, height: size)
        .background(Color("FillNeutral"), in: RoundedRectangle(cornerRadius: size * 0.28))
        .accessibilityHidden(true)
    }
}
