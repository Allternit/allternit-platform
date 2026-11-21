import Foundation

// The Workspace floor's Autopilot, digest and last-seen calls (platform
// floor_autopilot.rs: GET|PUT /floor/autopilot, GET /floor/digest,
// POST /floor/seen). Times are epoch milliseconds. Extends the same client
// that holds `floorState()` (OfficeFloorFeedV2.swift).

/// `GET|PUT /floor/autopilot`.
struct FloorAutopilot: Codable, Equatable {
    var enabled: Bool
    /// The bot Autopilot answers as; nil = the floor's main bot.
    var mainBotId: String?
    var rules: [FloorAutopilotRule]
    var updatedAt: Double?

    static let off = FloorAutopilot(enabled: false, mainBotId: nil, rules: [], updatedAt: nil)
}

/// One Autopilot rule: which bots (scope), which need, and how far Autopilot may go (level).
struct FloorAutopilotRule: Codable, Equatable, Identifiable {
    struct Scope: Codable, Equatable {
        /// "all" | "bot" | "pod"
        var type: String
        /// The bot or pod id; nil for "all".
        var id: String?
    }

    struct Limits: Codable, Equatable {
        var maxPerDay: Int?
        var allowedActions: [String]?
    }

    var id: String
    var scope: Scope
    /// "suggestion" | "permission" | "question"
    var need: String
    /// "draft" | "ask" | "tell" | "limits"
    var level: String
    var limits: Limits?
    var note: String?

    static let needs = ["suggestion", "permission", "question"]
    static let levels = ["draft", "ask", "tell", "limits"]

    /// A new rule: client ids are accepted by the server (≤128 chars).
    static func new(need: String = "suggestion") -> FloorAutopilotRule {
        FloorAutopilotRule(id: "rule-\(UUID().uuidString.lowercased())", scope: Scope(type: "all", id: nil),
                           need: need, level: need == "question" ? "draft" : "ask", limits: nil, note: nil)
    }

    /// Shared copy (LANE-BRIEF "Shared copy").
    static func levelName(_ level: String) -> String {
        switch level {
        case "draft": return "Draft only"
        case "tell": return "Send and tell me"
        case "limits": return "Act within limits"
        default: return "Ask first"
        }
    }

    static func levelHelp(_ level: String) -> String {
        switch level {
        case "draft": return "Your main bot writes a proposed answer. Nothing is sent."
        case "tell": return "Your main bot answers now, and the digest tells you."
        case "limits": return "Answers quietly up to the daily cap and allowed actions, then asks you."
        default: return "Waits for you."
        }
    }

    static func needName(_ need: String) -> String {
        switch need {
        case "permission": return "Permissions"
        case "question": return "Questions"
        default: return "Suggestions"
        }
    }
}

/// `GET /floor/digest`: what happened between `since` and `until`.
struct FloorDigest: Decodable, Equatable {
    struct Item: Decodable, Equatable, Identifiable {
        let botId: String
        let threadId: String?
        let text: String
        let at: Double
        let ruleId: String?

        var id: String { "\(botId):\(threadId ?? ""):\(Int(at))" }
    }

    let since: Double
    let until: Double
    let answered: [Item]
    let waiting: [Item]
    let finished: [Item]
    let suggestions: [Item]

    var count: Int { answered.count + waiting.count + finished.count + suggestions.count }
}

/// `POST /floor/seen`.
struct FloorSeen: Decodable {
    let previous: Double?
    let now: Double
}

/// A refused `PUT /floor/autopilot` (400 `{error, rule}`), in plain words.
struct FloorAutopilotRejection: LocalizedError, Equatable {
    let code: String
    /// Index of the rule at fault, when the server named one.
    let rule: Int?

    var errorDescription: String? {
        let lead = rule.map { "Rule \($0 + 1): " } ?? ""
        return lead + Self.message(code)
    }

    static func message(_ code: String) -> String {
        switch code {
        case "duplicate_rule": return "You already have a rule for that need and those bots."
        case "duplicate_rule_id": return "Two rules are the same. Remove one and try again."
        case "unknown_main_bot": return "Your main bot isn't one of your bots anymore. Pick your main bot again."
        case "too_many_rules": return "You can have up to 50 rules."
        case "bad_scope", "scope_id_required", "bad_scope_id": return "Pick which bot or pod this rule is for."
        case "bad_need", "bad_level", "bad_rule_id": return "This rule has an option the server doesn't accept. Update the app and try again."
        case "max_per_day_out_of_range": return "Answers per day must be between 1 and 200."
        case "too_many_actions": return "You can allow up to 20 actions."
        case "bad_action": return "Each allowed action needs a name of 40 characters or fewer."
        case "note_too_long": return "Notes can be up to 200 characters."
        default: return "Couldn't save Autopilot. Try again."
        }
    }
}

extension BotModeClient {
    func floorAutopilot() async throws -> FloorAutopilot {
        try await APIClient.shared.get(path: "floor/autopilot")
    }

    /// Saves the switch and rules; a 400 throws `FloorAutopilotRejection` with the server's code.
    func saveFloorAutopilot(_ autopilot: FloorAutopilot) async throws -> FloorAutopilot {
        struct Body: Encodable {
            let enabled: Bool
            let mainBotId: String?
            let rules: [FloorAutopilotRule]
        }
        let api = APIClient.shared
        var request = try await api.authorizedRequest(path: "floor/autopilot", method: "PUT")
        request.httpBody = try JSONEncoder().encode(Body(enabled: autopilot.enabled, mainBotId: autopilot.mainBotId, rules: autopilot.rules))
        let (data, response) = try await api.send(request)
        if (response as? HTTPURLResponse)?.statusCode == 400,
           let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
           let code = object["error"] as? String {
            throw FloorAutopilotRejection(code: code, rule: object["rule"] as? Int)
        }
        try api.validate(response, data: data, method: "PUT")
        do {
            return try JSONDecoder().decode(FloorAutopilot.self, from: data)
        } catch {
            throw APIError.decoding(error)
        }
    }

    /// The digest since `since` (epoch ms); nil = since the server's last-seen time (24 h when never seen).
    func floorDigest(since: Int? = nil) async throws -> FloorDigest {
        try await APIClient.shared.get(path: since.map { "floor/digest?since=\($0)" } ?? "floor/digest")
    }

    /// Stores now as the last-seen time; returns the previous one.
    @discardableResult
    func markFloorSeen() async throws -> FloorSeen {
        try await APIClient.shared.post(path: "floor/seen", body: [String: String]())
    }
}

/// True when an error means the phone has no network.
func isOfflineError(_ error: Error) -> Bool {
    let offline: Set<URLError.Code> = [.notConnectedToInternet, .networkConnectionLost, .dataNotAllowed]
    if let url = error as? URLError { return offline.contains(url.code) }
    if case .transport(let inner)? = error as? APIError, let url = inner as? URLError { return offline.contains(url.code) }
    return false
}
