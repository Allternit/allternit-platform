import Foundation

/// Models that answer only messages the person sends in chat
/// (CONTRACT-connections-2026-11 §9.1, §9.5, §9.9):
///
/// - `plan/<vendor>/<model>`: the person's Claude, ChatGPT, Grok or Kimi plan
///   on an Allternit runner, sent to cloud-api `/v1/chat/completions`.
/// - `copilot/<model>`: GitHub Copilot, same route.
/// - `chatgpt/<model>`: ChatGPT signed in on this iPhone, sent from the
///   phone straight to OpenAI.
///
/// None of them may be picked for a bot, routine, schedule, mission or agent.
enum PersonModels {
    static let prefixes = ["plan/", "copilot/", "chatgpt/"]

    static func isPersonOnly(_ id: String?) -> Bool {
        guard let id else { return false }
        return prefixes.contains { id.hasPrefix($0) }
    }

    /// Sent from the phone to OpenAI, not through cloud-api.
    static func isOnDevice(_ id: String?) -> Bool { id?.hasPrefix("chatgpt/") ?? false }

    /// The vendor name for messages ("Claude", "GitHub Copilot", "ChatGPT").
    static func vendorName(for id: String) -> String {
        if id.hasPrefix("copilot/") { return "GitHub Copilot" }
        if id.hasPrefix("chatgpt/") { return "ChatGPT" }
        let parts = id.split(separator: "/")
        guard parts.count >= 2 else { return "your plan" }
        switch parts[1] {
        case "claude": return "Claude"
        case "chatgpt": return "ChatGPT"
        case "grok": return "Grok"
        case "kimi": return "Kimi"
        default: return String(parts[1]).capitalized
        }
    }

    /// The picker's group id for a person model.
    static func provider(for id: String) -> String {
        if id.hasPrefix("copilot/") { return "copilot" }
        if id.hasPrefix("chatgpt/") { return "chatgpt-phone" }
        let parts = id.split(separator: "/")
        return parts.count >= 2 ? "plan-\(parts[1])" : "plan"
    }

    static let providerNames: [String: String] = [
        "plan-claude": "Your Claude plan",
        "plan-chatgpt": "Your ChatGPT plan",
        "plan-grok": "Your Grok plan",
        "plan-kimi": "Your Kimi plan",
        "copilot": "GitHub Copilot",
        "chatgpt-phone": "ChatGPT · this iPhone",
    ]

    /// The vendor's own model name with its first letter raised:
    /// "plan/claude/sonnet" → "Sonnet".
    private static func displayName(_ id: String) -> String {
        let last = id.split(separator: "/").last.map(String.init) ?? id
        return last.prefix(1).uppercased() + last.dropFirst()
    }

    /// The chat picker's person models: plan and Copilot rows that are `on`
    /// on cloud-api, then ChatGPT signed in on this phone.
    static func build(rows: [ConnectionWire], chatgpt: [ChatGPTModel]) -> [RuntimeModel] {
        var out: [RuntimeModel] = []
        var seen = Set<String>()
        let order = ["claude", "chatgpt", "grok", "kimi", "copilot"]
        let sorted = rows.sorted { (order.firstIndex(of: $0.id) ?? 99) < (order.firstIndex(of: $1.id) ?? 99) }
        for row in sorted where row.state == .on && row.runtimeId == nil {
            for id in row.models where (id.hasPrefix("plan/") || id.hasPrefix("copilot/")) && seen.insert(id).inserted {
                out.append(RuntimeModel(id: id, name: displayName(id), provider: provider(for: id),
                                        description: "Answers only messages you send in chat"))
            }
        }
        for model in chatgpt where seen.insert(model.id).inserted {
            out.append(RuntimeModel(id: model.id, name: model.name, provider: "chatgpt-phone",
                                    description: "Runs on your ChatGPT plan, from this iPhone"))
        }
        return out
    }

    /// The models a bot, routine or schedule may use: never a person model.
    static func forAutomation(_ models: [RuntimeModel]) -> [RuntimeModel] {
        models.filter { !isPersonOnly($0.id) && providerNames[$0.provider] == nil }
    }
}
