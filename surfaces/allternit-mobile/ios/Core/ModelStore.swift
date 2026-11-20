import SwiftUI

/// App-wide runtime-model selection behind the composer's model pill: the
/// catalog (`GET /api/v1/models`) plus the persisted selection. Shared —
/// Home and Code threads hold separate ChatViewModels but must send the
/// same model.
///
/// nil selection = backend default: /api/agent-chat falls back to its
/// env-configured default model when `runtimeModelId` is absent
/// (v1_routes.rs:182-200). The store auto-selects the first catalog entry
/// after the initial fetch so a fresh install never sends with no model —
/// a backend without a configured default rejects the send outright.
@MainActor
final class ModelStore: ObservableObject {
    static let shared = ModelStore()

    @Published var selectedModelId: String? {
        didSet {
            defaults.set(selectedModelId, forKey: Keys.selectedModelId)
            if Self.cloudAlias(selectedModelId) != nil { lastCloudModelId = selectedModelId }
        }
    }

    /// The person's most recent Allternit Cloud model (persisted): where
    /// "Use Allternit credits for this message" re-sends.
    private(set) var lastCloudModelId: String? {
        didSet { defaults.set(lastCloudModelId, forKey: Keys.lastCloudModelId) }
    }

    /// The Cloud model the credits button uses, or nil to hide the button.
    var creditsFallbackModelId: String? {
        CreditsFallback.modelId(recent: lastCloudModelId, listed: cloudModels.map(\.id))
    }

    /// The picked thinking level (persisted). Each model offers its own levels;
    /// `level(for:)` falls back to the model's default when it doesn't take this one.
    @Published var selectedEffort: String {
        didSet { defaults.set(selectedEffort, forKey: Keys.selectedEffort) }
    }

    /// Fast (Codex priority tier). Sent only for models that offer it; the
    /// server checks again (model-thinking.json `fast`).
    @Published var fastMode: Bool {
        didSet { defaults.set(fastMode, forKey: Keys.fastMode) }
    }

    /// The computer's catalog. Bots and routines pick from
    /// `automationModels`, never from the person models below.
    @Published private(set) var models: [RuntimeModel] = []
    /// Models that answer only the person's own chat messages: plan and
    /// Copilot rows that are connected, and ChatGPT signed in on this phone
    /// (`PersonModels`). Kept apart from `models` so no bot or routine
    /// picker can offer them.
    @Published private(set) var personModels: [RuntimeModel] = []
    @Published private(set) var isLoading = false
    @Published private(set) var loadError: String? = nil

    private let defaults: UserDefaults
    private let chatClient: AgentChatClient
    private var fetchTask: Task<Void, Never>? = nil

    private enum Keys {
        static let selectedModelId = "allternit-runtime-model-id"
        static let lastCloudModelId = "allternit-last-cloud-model-id"
        static let selectedEffort = "allternit-model-effort"
        static let fastMode = "allternit-model-fast"
    }

    init(defaults: UserDefaults = .standard, chatClient: AgentChatClient = AgentChatClient()) {
        self.defaults = defaults
        self.chatClient = chatClient
        self.lastCloudModelId = defaults.string(forKey: Keys.lastCloudModelId)
        self.selectedModelId = defaults.string(forKey: Keys.selectedModelId)
        if lastCloudModelId == nil, Self.cloudAlias(selectedModelId) != nil { self.lastCloudModelId = selectedModelId }
        self.selectedEffort = defaults.string(forKey: Keys.selectedEffort) ?? "medium"
        self.fastMode = defaults.bool(forKey: Keys.fastMode)
    }

    /// Composer pill label: the selected model's short name (derived from
    /// the persisted id, so it renders before the catalog loads) plus the
    /// effort when the model supports it ("claude-sonnet-4-6 Medium"),
    /// or "Pick a brain" when none is chosen (there is no default model).
    var pillLabel: String {
        guard let selectedModelId, !selectedModelId.isEmpty else { return "Pick a brain" }
        if let person = personModels.first(where: { $0.id == selectedModelId }) { return person.name }
        if let local = OnDeviceModelId.displayName(selectedModelId) ?? OtherDeviceModelId.displayName(selectedModelId) { return local }
        let short = selectedModelId.split(separator: "/", maxSplits: 1).last.map(String.init) ?? selectedModelId
        guard let thinking = models.first(where: { $0.id == selectedModelId })?.effectiveThinking else { return short }
        return "\(short) \(thinking.label(thinking.level(selectedEffort)))"
    }

    /// The currently selected catalog entry, when the catalog is loaded.
    var selectedModel: RuntimeModel? {
        chatModels.first { $0.id == selectedModelId }
    }

    /// What the chat composer's picker offers: the person's own plans first,
    /// then the computer's catalog, or with no computer the Allternit Cloud
    /// catalog as `allternit/<alias>` (sent straight to cloud-api, D7/D8).
    var chatModels: [RuntimeModel] { personModels + (models.isEmpty ? cloudModels : models) }

    // MARK: - Allternit Cloud with no computer (CONTRACT-cloud-credits-2026-11-15)

    nonisolated static let cloudPrefix = "allternit/"

    /// The public Allternit Cloud catalog (`{cloud}/v1/models`): the
    /// no-computer rows, and each Cloud row's provider (for the lock) and price.
    @Published private(set) var cloudCatalog: [SetupModelRow] = []
    private var cloudFetchTask: Task<Void, Never>? = nil

    /// "allternit/glm-5.3" → "glm-5.3"; nil for any other model.
    nonisolated static func cloudAlias(_ id: String?) -> String? {
        guard let id, id.hasPrefix(cloudPrefix) else { return nil }
        let alias = String(id.dropFirst(cloudPrefix.count))
        return alias.isEmpty ? nil : alias
    }

    /// The rows offered with no computer: listed, standard-lane models. The
    /// open lane has no terms screen here, so the server would refuse it (D14).
    nonisolated static func cloudChatModels(_ catalog: [SetupModelRow]) -> [RuntimeModel] {
        catalog.filter { !$0.isHidden && $0.lane != "open" }.map { row in
            RuntimeModel(id: cloudPrefix + row.id, name: row.name, provider: "allternit",
                         description: "Allternit Cloud · no computer needed", isDefault: row.isDefault)
        }
    }

    var cloudModels: [RuntimeModel] { Self.cloudChatModels(cloudCatalog) }

    func cloudRow(for id: String?) -> SetupModelRow? {
        guard let alias = Self.cloudAlias(id) else { return nil }
        return cloudCatalog.first { $0.id == alias }
    }

    /// An `allternit/…` model with no computer catalog listing it: the turn
    /// goes straight to cloud-api. With a computer, Cloud models keep the
    /// computer path (D8).
    func routesDirect(_ id: String?) -> Bool {
        guard let id, Self.cloudAlias(id) != nil else { return false }
        return !models.contains { $0.id == id }
    }

    /// A Cloud row the account can't run yet: not allowed and not served by
    /// one of the person's own keys (D5). Unknown entitlement = unlocked (D10).
    func isCloudLocked(_ id: String?) -> Bool {
        guard Self.cloudAlias(id) != nil else { return false }
        return CloudModelsEntitlement.isLocked(AccountPlanStore.shared.cloudModels, provider: cloudRow(for: id)?.provider)
    }

    func fetchCloudCatalogIfNeeded(force: Bool = false) {
        guard force || cloudCatalog.isEmpty, cloudFetchTask == nil else { return }
        cloudFetchTask = Task { [weak self] in
            let rows = await SetupAPI.cloudModelCatalog()
            guard let self else { return }
            if !rows.isEmpty { self.cloudCatalog = rows }
            self.cloudFetchTask = nil
        }
    }

    #if DEBUG
    /// Fixtures for the debug gallery (`-cloud-credits-gallery`).
    func setFixtures(models: [RuntimeModel], cloudCatalog: [SetupModelRow]) {
        self.models = models
        self.cloudCatalog = cloudCatalog
    }
    #endif

    /// What a bot, routine or schedule may use: the catalog without any
    /// person-only model (contract §9.1 `person_only`).
    var automationModels: [RuntimeModel] { PersonModels.forAutomation(models) }

    /// `automationModels` grouped by provider, in catalog order.
    var automationProviders: [(provider: String, models: [RuntimeModel])] {
        Self.grouped(automationModels)
    }

    func setPersonModels(_ list: [RuntimeModel]) {
        guard list != personModels else { return }
        personModels = list
    }

    /// Effort to attach to the next send — nil unless the selected model
    /// actually supports it (the runtime rejects unknown parameters on some
    /// providers, so we never send it speculatively).
    var effortForSend: String? {
        selectedModel?.effectiveThinking.map { $0.level(selectedEffort) }
    }

    /// The level shown for `model`: the picked one when it takes it, else its default.
    func level(for model: RuntimeModel) -> String? {
        model.effectiveThinking.map { $0.level(selectedEffort) }
    }

    /// True once the user picks a model in the picker this launch. Until
    /// then a selected agent's own model may win the send (plan Phase 6.3:
    /// a manual composer choice always overrides the agent's model). Not
    /// persisted — every launch starts agent-deferential again.
    @Published private(set) var didManuallySelectModel = false

    /// The picker sheet's selection path — marks the choice as manual so
    /// per-agent model defaults stop applying this launch.
    func userSelectedModel(_ id: String?) {
        selectedModelId = id
        didManuallySelectModel = true
    }

    /// Catalog rows grouped by provider, preserving catalog order — the
    /// selector menu renders one section per provider.
    var providers: [(provider: String, models: [RuntimeModel])] { Self.grouped(models) }

    private static func grouped(_ models: [RuntimeModel]) -> [(provider: String, models: [RuntimeModel])] {
        var order: [String] = []
        var byProvider: [String: [RuntimeModel]] = [:]
        for model in models {
            if byProvider[model.provider] == nil { order.append(model.provider) }
            byProvider[model.provider, default: []].append(model)
        }
        return order.map { ($0, byProvider[$0] ?? []) }
    }

    /// Catalog rows grouped by tier (Flagship → Standard → Fast), with
    /// `legacy` split out for the picker's "More models" disclosure.
    var tiers: [(tier: RuntimeModel.Tier, models: [RuntimeModel])] {
        var grouped: [RuntimeModel.Tier: [RuntimeModel]] = [:]
        for model in models {
            grouped[model.resolvedTier, default: []].append(model)
        }
        return [RuntimeModel.Tier.flagship, .standard, .fast]
            .compactMap { tier in grouped[tier].map { (tier, $0) } }
    }

    var legacyModels: [RuntimeModel] {
        models.filter { $0.resolvedTier == .legacy }
    }

    /// Fetches the catalog once per launch unless forced; concurrent callers
    /// share the in-flight request (same idiom as AgentModeStore's registry
    /// fetch). A fetch failure is a plain error — the picker shows its
    /// error/retry state; there is no client-side substitute catalog.
    func fetchModelsIfNeeded(force: Bool = false) {
        fetchCloudCatalogIfNeeded()
        guard force || models.isEmpty, fetchTask == nil else { return }
        isLoading = true
        loadError = nil
        fetchTask = Task { [weak self] in
            guard let self else { return }
            defer {
                self.isLoading = false
                self.fetchTask = nil
            }
            do {
                self.models = try await self.chatClient.listModels()
            } catch is CancellationError {
                // View went away mid-flight — keep current state.
                return
            } catch {
                self.loadError = error.localizedDescription
                return
            }
            // Never pick a model for the user (there is no default brain).
            // A persisted id the catalog no longer lists (e.g. a bare id from
            // before the catalog carried provider-prefixed ids) is a dead end:
            // clear it so the composer shows "Pick a brain" and the next send
            // opens the picker.
            // A person model (plan/…, copilot/…, chatgpt/…) isn't in the
            // computer's catalog; its row in Connections decides if it works.
            let selectionIsStale = self.selectedModelId != nil
                && !PersonModels.isPersonOnly(self.selectedModelId)
                && !self.models.contains(where: { $0.id == self.selectedModelId })
            if selectionIsStale, !self.models.isEmpty {
                // A CLI model the vendor retired: carry the user's choice to
                // its successor when the server lists it, else clear.
                if let stale = self.selectedModelId,
                   let renamed = Self.renamedId(stale),
                   self.models.contains(where: { $0.id == renamed }) {
                    self.selectedModelId = renamed
                } else {
                    self.selectedModelId = nil
                }
            }
        }
    }

    /// Retired CLI model ids and their successors (model id part only).
    private static let retiredModelIds: [String: String] = [
        "claude-fable-5-1": "claude-fable-5-2",
        "claude-opus-5": "claude-opus-5-5",
        "claude-sonnet-5": "claude-sonnet-5-5",
        "gemini-3.7-flash-high": "gemini-3.8-flash-high",
        "kimi-k2.7-code": "kimi-for-coding",
    ]

    /// "provider/old" -> "provider/new", keeping the provider; nil if not retired.
    private static func renamedId(_ id: String) -> String? {
        guard let slash = id.firstIndex(of: "/") else { return retiredModelIds[id] }
        let provider = id[..<slash]
        let model = String(id[id.index(after: slash)...])
        return retiredModelIds[model].map { "\(provider)/\($0)" }
    }
}
