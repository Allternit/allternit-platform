import Foundation

// -----------------------------------------------------------------------------
// Bot packaging layer — Bot = Agent with `isBot: true` + a `botProfile` blob
// of UX metadata. Contract: surfaces/ai.allternit.com/src/lib/bots/BOT_AGENT_CONTRACT.md;
// helper semantics mirror surfaces/ai.allternit.com/src/lib/bots/bot-profile.ts
// (isBot/getBotDisplayName/getBotTagline/getBotAccentColor, BOT_CATEGORIES).
// The backend has no dedicated columns: `is_bot`/`bot_profile` are merged into
// the agent's `config` JSON as camelCase keys `isBot`/`botProfile`
// (cmd/allternit-api/src/agent_routes.rs merge_autonomous_primitives_into_config),
// so everything here reads out of AgentRecord.config — no new wire fields.
// -----------------------------------------------------------------------------

/// UX/packaging metadata of a packaged bot (`config.botProfile`).
/// Tolerant decoding per repo model convention: every field is optional at
/// the wire level even though the contract requires `displayName`
/// (BOT_AGENT_CONTRACT.md "Validation rules" #2) — `botDisplayName` falls
/// back to the agent's `name` when it's absent/empty.
struct BotProfile: Codable, Sendable, Equatable, Hashable {
    /// Required user-facing name shown in cards and headers.
    var displayName: String
    var tagline: String?
    var welcomeMessage: String?
    /// Quick-start prompts (contract caps at 5 — a creation-time validation,
    /// not enforced at decode).
    var starterPrompts: [String]
    /// Hex color ("#8b5cf6") for card theming; render via `Color(hex:)`.
    var accentColor: String?
    /// Defaults to `true` for bots (BOT_AGENT_CONTRACT.md "Authoring").
    var groupChatEnabled: Bool
    var defaultPresetId: String?
    var botCategory: BotCategory?

    enum CodingKeys: String, CodingKey {
        case displayName, tagline, welcomeMessage, starterPrompts, accentColor
        case groupChatEnabled, defaultPresetId, botCategory
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        displayName = try container.decodeIfPresent(String.self, forKey: .displayName) ?? ""
        tagline = try container.decodeIfPresent(String.self, forKey: .tagline)
        welcomeMessage = try container.decodeIfPresent(String.self, forKey: .welcomeMessage)
        starterPrompts = try container.decodeIfPresent([String].self, forKey: .starterPrompts) ?? []
        accentColor = try container.decodeIfPresent(String.self, forKey: .accentColor)
        groupChatEnabled = (try? container.decodeIfPresent(Bool.self, forKey: .groupChatEnabled)) ?? true
        defaultPresetId = try container.decodeIfPresent(String.self, forKey: .defaultPresetId)
        // Unknown category strings decode to nil rather than failing the row.
        if let raw = try container.decodeIfPresent(String.self, forKey: .botCategory) {
            botCategory = BotCategory(rawValue: raw)
        } else {
            botCategory = nil
        }
    }

    /// Memberwise init for fixtures and local edits.
    init(displayName: String, tagline: String? = nil, welcomeMessage: String? = nil,
         starterPrompts: [String] = [], accentColor: String? = nil,
         groupChatEnabled: Bool = true, defaultPresetId: String? = nil,
         botCategory: BotCategory? = nil) {
        self.displayName = displayName
        self.tagline = tagline
        self.welcomeMessage = welcomeMessage
        self.starterPrompts = starterPrompts
        self.accentColor = accentColor
        self.groupChatEnabled = groupChatEnabled
        self.defaultPresetId = defaultPresetId
        self.botCategory = botCategory
    }
}

/// Bot categories — keys and copy mirror `BOT_CATEGORIES` in
/// lib/bots/bot-profile.ts:202-235; `allCases` preserves the same order as
/// the TS record's `Object.keys` (used for the hub's filter chips).
enum BotCategory: String, Codable, Sendable, CaseIterable, Identifiable {
    case research, code, writing, data, sales, design, ops, custom

    var id: String { rawValue }

    var label: String {
        switch self {
        case .research: return "Research"
        case .code: return "Code"
        case .writing: return "Writing"
        case .data: return "Data"
        case .sales: return "Sales"
        case .design: return "Design"
        case .ops: return "Operations"
        case .custom: return "Custom"
        }
    }

    var description: String {
        switch self {
        case .research: return "Information gathering and analysis"
        case .code: return "Software development and engineering"
        case .writing: return "Content creation and editing"
        case .data: return "Data analysis and visualization"
        case .sales: return "Outreach and lead generation"
        case .design: return "UI/UX and visual design"
        case .ops: return "Process automation and workflows"
        case .custom: return "Specialized or user-defined bots"
        }
    }
}

/// Category catalog in filter-chip order, mirroring the `BOT_CATEGORIES`
/// constant in lib/bots/bot-profile.ts.
let BOT_CATEGORIES: [BotCategory] = BotCategory.allCases
