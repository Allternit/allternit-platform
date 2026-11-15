import Foundation

/// One row of the flattened model catalog — `GET /api/v1/models`
/// (v1_routes.rs `list_available_models` → provider_routes.rs
/// `available_model_catalog`): `{id: "provider/model", name, provider,
/// description?, tier?, supports_effort?}`.
/// `id` is exactly what `POST /api/agent-chat` accepts as `runtimeModelId`
/// (v1_routes.rs:182-200 splits it back into provider + model).
struct RuntimeModel: Decodable, Sendable, Identifiable, Equatable {
    /// Picker grouping, mirroring the Claude app's sheet sections.
    /// `legacy` rows collapse under a "More models" disclosure.
    enum Tier: String, Decodable, Sendable, Equatable {
        case flagship, standard, fast, legacy
    }

    let id: String
    let name: String
    let provider: String
    /// One-line positioning copy ("Most efficient for everyday tasks").
    /// Optional: pre-metadata backends omit it and the row hides the subtitle.
    let description: String?
    let tier: Tier?
    /// Reasoning-effort selector eligibility (true when `thinking` is set).
    let supportsEffort: Bool
    /// The model's own thinking levels, word and default (model-thinking.json).
    let thinking: ModelThinking?
    /// The vendor's own default for this provider (server `default`; absent on
    /// older servers). Informational only: the app never auto-selects it.
    let isDefault: Bool
    /// Vendor order inside the provider, 0 = first (server `rank`; nil on
    /// older servers).
    let rank: Int?
    /// Retired Cloud alias (server `hidden`): still runs for a saved choice but
    /// is not offered in the picker.
    let isHidden: Bool

    /// The bare model id ("claude-sonnet-4.6" out of
    /// "anthropic/claude-sonnet-4.6") — the composer pill's compact label.
    var shortName: String {
        id.split(separator: "/", maxSplits: 1).last.map(String.init) ?? id
    }

    var resolvedTier: Tier { tier ?? .standard }

    private enum CodingKeys: String, CodingKey {
        case id, name, provider, description, tier
        case supportsEffort = "supports_effort"
        case thinking
        case isDefault = "default"
        case rank
        case isHidden = "hidden"
    }

    init(id: String, name: String, provider: String, description: String? = nil, tier: Tier? = nil, supportsEffort: Bool = false, thinking: ModelThinking? = nil, isDefault: Bool = false, rank: Int? = nil, isHidden: Bool = false) {
        self.id = id
        self.name = name
        self.provider = provider
        self.description = description
        self.tier = tier
        self.supportsEffort = supportsEffort || thinking != nil
        self.thinking = thinking
        self.isDefault = isDefault
        self.rank = rank
        self.isHidden = isHidden
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = try container.decode(String.self, forKey: .id)
        name = try container.decode(String.self, forKey: .name)
        provider = try container.decode(String.self, forKey: .provider)
        description = try container.decodeIfPresent(String.self, forKey: .description)
        tier = try container.decodeIfPresent(Tier.self, forKey: .tier)
        thinking = try? container.decodeIfPresent(ModelThinking.self, forKey: .thinking)
        isDefault = (try? container.decodeIfPresent(Bool.self, forKey: .isDefault)) ?? false
        rank = try? container.decodeIfPresent(Int.self, forKey: .rank)
        isHidden = (try? container.decodeIfPresent(Bool.self, forKey: .isHidden)) ?? false
        supportsEffort = (try container.decodeIfPresent(Bool.self, forKey: .supportsEffort) ?? false) || thinking != nil
    }

    /// The levels to offer: the model's own, or the legacy four for an older
    /// runtime that reports `supports_effort` without a `thinking` object.
    var effectiveThinking: ModelThinking? {
        thinking ?? (supportsEffort ? .legacy : nil)
    }
}

/// `thinking` on a catalog row: the vendor's word ("Effort", "Reasoning"),
/// its levels least to most, the default, and whether the account has a
/// faster priority tier (Codex).
struct ModelThinking: Decodable, Sendable, Equatable {
    struct Level: Decodable, Sendable, Equatable, Hashable {
        let id: String
        let label: String
    }

    let vendor: String
    let vendorName: String?
    let term: String
    let note: String?
    let toggle: Bool
    let levels: [Level]
    let `default`: String
    let fast: Bool

    private enum CodingKeys: String, CodingKey {
        case vendor, term, note, toggle, levels, fast
        case vendorName = "vendor_name"
        case `default`
    }

    init(vendor: String, vendorName: String?, term: String, note: String?, toggle: Bool, levels: [Level], default def: String, fast: Bool) {
        self.vendor = vendor
        self.vendorName = vendorName
        self.term = term
        self.note = note
        self.toggle = toggle
        self.levels = levels
        self.default = def
        self.fast = fast
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        vendor = try c.decode(String.self, forKey: .vendor)
        vendorName = try c.decodeIfPresent(String.self, forKey: .vendorName)
        term = try c.decodeIfPresent(String.self, forKey: .term) ?? "Thinking"
        note = try c.decodeIfPresent(String.self, forKey: .note)
        toggle = try c.decodeIfPresent(Bool.self, forKey: .toggle) ?? false
        levels = try c.decode([Level].self, forKey: .levels)
        `default` = try c.decode(String.self, forKey: .default)
        fast = try c.decodeIfPresent(Bool.self, forKey: .fast) ?? false
    }

    static let legacy = ModelThinking(
        vendor: "claude", vendorName: nil, term: "Effort", note: nil, toggle: false,
        levels: [.init(id: "low", label: "Low"), .init(id: "medium", label: "Medium"), .init(id: "high", label: "High")],
        default: "medium", fast: false
    )

    /// The level when this model takes it, else its default.
    func level(_ wanted: String?) -> String {
        guard let wanted, levels.contains(where: { $0.id == wanted }) else { return self.default }
        return wanted
    }

    func label(_ id: String) -> String { levels.first { $0.id == id }?.label ?? id }
}


