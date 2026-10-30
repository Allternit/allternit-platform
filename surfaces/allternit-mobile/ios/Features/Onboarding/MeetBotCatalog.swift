import Foundation
import SwiftUI

/// The web's built-in bot templates for "Meet your bots" (allternit-ai
/// `src/lib/bots/templates`): ids, names, routines (cron + prompt), first
/// jobs and the exact create body the web sends. Data lives in the
/// generated `MeetBotCatalogData.swift`; this file decodes it once.
///
/// The server's `/api/v1/agent-templates` catalog (AgentHubStore.templates)
/// is the pattern-crew catalog: it has no routines and none of these ids,
/// so the meet flow can't use it.
struct MeetBotTemplate: Decodable, Identifiable, Sendable, Hashable {
    struct FirstTask: Decodable, Sendable, Hashable {
        let prompt: String
        let expect: String
    }

    struct Routine: Decodable, Sendable, Hashable {
        let id: String
        let label: String
        /// "Weekdays 7:30 — morning note" → "Morning note" (team-slots routineTitle).
        let title: String
        let cron: String?
        let event: String?
        let prompt: String
        /// Plain-words schedule (routine-rule ruleLabel); nil when the web's
        /// editor can't draw the cron (those install paused, unseen).
        let scheduleLabel: String?

        /// Shown in the routines step (web `draftsForTemplate`).
        var isVisible: Bool { cron != nil && scheduleLabel != nil }
    }

    let id: String
    let version: String
    let name: String
    let tagline: String
    let category: String
    let color: String
    /// Gizzi YouTube cast id the bot wears, or nil for the plain avatar.
    let look: String?
    let welcome: String?
    let firstTask: FirstTask
    let starterPrompts: [String]
    /// Template ids this bot hands work to.
    let handoffs: [String]
    let routines: [Routine]
    /// The agent create body (snake_case wire) with `__MODEL__` /
    /// `__PROVIDER__` placeholders.
    let createBody: JSONValue?

    static func == (a: MeetBotTemplate, b: MeetBotTemplate) -> Bool { a.id == b.id }
    func hash(into hasher: inout Hasher) { hasher.combine(id) }

    var accent: Color { Color(hex: color) }

    /// The cast sprite sheet the template's bot wears.
    var spriteURL: URL? {
        guard let look else { return nil }
        return MeetBotCatalog.spriteURL(for: look)
    }

    var visibleRoutines: [Routine] { routines.filter(\.isVisible) }
    var hiddenRoutines: [Routine] { routines.filter { !$0.isVisible } }
}

/// One of the cast looks A:// can wear (web `ALLTERNIT_LOOKS` kind "cast").
struct MeetLook: Decodable, Identifiable, Sendable, Hashable {
    let id: String
    let label: String
    let field: String
    /// `lookAvatarConfig(id)` — saved as the bot's avatar.
    let avatar: JSONValue?

    static func == (a: MeetLook, b: MeetLook) -> Bool { a.id == b.id }
    func hash(into hasher: inout Hasher) { hasher.combine(id) }

    var spriteURL: URL? { MeetBotCatalog.spriteURL(for: id) }
}

struct MeetBotCatalog: Decodable, Sendable {
    let primary: MeetBotTemplate
    let templates: [MeetBotTemplate]
    let looks: [MeetLook]

    static let shared: MeetBotCatalog = {
        do {
            return try JSONDecoder().decode(MeetBotCatalog.self, from: Data(MeetBotCatalogData.json.utf8))
        } catch {
            assertionFailure("MeetBotCatalogData failed to decode: \(error)")
            return MeetBotCatalog(primary: nil)
        }
    }()

    private enum CodingKeys: String, CodingKey { case primary, templates, looks }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        primary = try c.decode(MeetBotTemplate.self, forKey: .primary)
        templates = try c.decode([MeetBotTemplate].self, forKey: .templates)
        looks = try c.decodeIfPresent([MeetLook].self, forKey: .looks) ?? []
    }

    /// Empty catalog (decode failure only; never shipped data).
    private init(primary: MeetBotTemplate?) {
        self.primary = primary ?? MeetBotTemplate.placeholderPrimary
        self.templates = []
        self.looks = []
    }

    func template(_ id: String) -> MeetBotTemplate? {
        if id == primary.id { return primary }
        return templates.first { $0.id == id }
    }

    /// "All" first, then each category once, in catalog order.
    var categories: [String] {
        var out: [String] = []
        for t in templates where !out.contains(t.category) { out.append(t.category) }
        return out
    }

    static func categoryLabel(_ category: String) -> String {
        switch category {
        case "ops": return "Ops"
        case "marketing": return "Marketing"
        case "research": return "Research"
        case "delivery": return "Delivery"
        case "finance": return "Finance"
        case "sales": return "Sales"
        case "engineering": return "Engineering"
        case "design": return "Design"
        default: return category.prefix(1).uppercased() + category.dropFirst()
        }
    }

    /// Served by the web app (public/avatar-packs/gizzi-youtube).
    static func spriteURL(for slug: String) -> URL? {
        URL(string: "/avatar-packs/gizzi-youtube/\(slug)-sheet.webp", relativeTo: BotAvatarSource.webOrigin)?.absoluteURL
    }

    /// "Works well with your team": bots the team's templates hand work to
    /// that aren't on it yet (team-slots handoffSuggestions).
    func handoffSuggestions(for members: [String], limit: Int = 4) -> [MeetBotTemplate] {
        var out: [MeetBotTemplate] = []
        for id in members {
            for to in template(id)?.handoffs ?? [] {
                guard let t = template(to), !members.contains(t.id), !out.contains(t) else { continue }
                out.append(t)
            }
        }
        return Array(out.prefix(limit))
    }
}

private extension MeetBotTemplate {
    static var placeholderPrimary: MeetBotTemplate {
        MeetBotTemplate(
            id: "primary-a", version: "1.0.0", name: "A://", tagline: "", category: "ops", color: "#D97757",
            look: nil, welcome: nil, firstTask: FirstTask(prompt: "", expect: ""), starterPrompts: [],
            handoffs: [], routines: [], createBody: nil
        )
    }
}

// MARK: - Avatar tile

/// A template's (or look's) tile: its colour field with the cast sprite.
struct MeetTemplateTile: View {
    let color: String
    let spriteURL: URL?
    var size: CGFloat = 40
    var live: Bool = false
    var label: String = ""

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: size * 0.28, style: .continuous)
                .fill(Color(hex: color))
            if let spriteURL {
                SpriteAvatar(url: spriteURL, size: size * 0.92, live: live)
            } else {
                AgentIdenticonView(seed: label.isEmpty ? color : label, size: size * 0.6)
            }
        }
        .frame(width: size, height: size)
        .clipShape(RoundedRectangle(cornerRadius: size * 0.28, style: .continuous))
        .accessibilityHidden(true)
    }
}

/// The A:// tile (the block mark on ink).
struct MeetPrimaryTile: View {
    var size: CGFloat = 40

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: size * 0.28, style: .continuous)
                .fill(Color(hex: "#141413"))
            Text("A://")
                .font(.system(size: size * 0.3, weight: .bold, design: .monospaced))
                .foregroundColor(.white)
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}
