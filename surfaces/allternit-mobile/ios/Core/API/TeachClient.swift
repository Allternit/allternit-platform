import Foundation

// MARK: - Models (lib/api-capture/api.ts)

/// One step of a taught skill (`TeachStep`).
struct TeachStep: Decodable, Sendable, Identifiable, Equatable {
    let id: String
    /// api | ui | shell | note
    let kind: String
    let text: String
    let target: String?

    private enum CodingKeys: String, CodingKey { case id, kind, text, target }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = ((try? c.decodeIfPresent(String.self, forKey: .id)) ?? nil) ?? UUID().uuidString
        kind = ((try? c.decodeIfPresent(String.self, forKey: .kind)) ?? nil) ?? "ui"
        text = ((try? c.decodeIfPresent(String.self, forKey: .text)) ?? nil) ?? ""
        target = (try? c.decodeIfPresent(String.self, forKey: .target)) ?? nil
    }
}

/// A value the skill asks for when it runs (`TeachInput`).
struct TeachInput: Decodable, Sendable, Identifiable, Equatable {
    let name: String
    let label: String
    let example: String
    let description: String?
    /// text | number | date | datetime | email | url | choice | file | folder | list | secret
    let type: String?
    let required: Bool?
    let choices: [String]?

    private enum CodingKeys: String, CodingKey { case name, label, example, description, type, required, choices }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        name = try c.decode(String.self, forKey: .name)
        label = ((try? c.decodeIfPresent(String.self, forKey: .label)) ?? nil) ?? ""
        example = ((try? c.decodeIfPresent(String.self, forKey: .example)) ?? nil) ?? ""
        description = (try? c.decodeIfPresent(String.self, forKey: .description)) ?? nil
        type = (try? c.decodeIfPresent(String.self, forKey: .type)) ?? nil
        required = (try? c.decodeIfPresent(Bool.self, forKey: .required)) ?? nil
        choices = (try? c.decodeIfPresent([String].self, forKey: .choices)) ?? nil
    }

    var id: String { name }
    var isSecret: Bool { type == "secret" }
}

/// A saved Teach skill (`TeachSkill`), read for display. `raw` is the row
/// as the server sent it, so a save (schedule, rename) sends back every
/// field, including the compiled `workflow` the phone doesn't draw.
struct TeachSkill: Identifiable, Sendable, Equatable {
    let raw: [String: JSONValue]
    let id: String
    let title: String
    let intent: String
    let steps: [TeachStep]
    let inputs: [TeachInput]
    /// model | rules
    let source: String
    let redactions: Int
    let domain: String?
    let skillPath: String?
    let routineId: String?
    /// Steps Run can play back without the model (Desktop runner).
    let workflowStepCount: Int
    let updatedAt: String

    private struct Wire: Decodable {
        let id: String
        let title: String?
        let intent: String?
        let steps: [TeachStep]?
        let inputs: [TeachInput]?
        let source: String?
        let redactions: Int?
        let domain: String?
        let skill_path: String?
        let routine_id: String?
        let workflow: Workflow?
        let updated_at: String?

        struct Workflow: Decodable { let steps: [JSONValue]? }
    }

    init?(raw: [String: JSONValue]) {
        guard let data = try? JSONEncoder().encode(raw),
              let wire = try? JSONDecoder().decode(Wire.self, from: data) else { return nil }
        self.raw = raw
        id = wire.id
        title = wire.title?.isEmpty == false ? wire.title! : "Untitled skill"
        intent = wire.intent ?? ""
        steps = wire.steps ?? []
        inputs = wire.inputs ?? []
        source = wire.source ?? "rules"
        redactions = wire.redactions ?? 0
        domain = wire.domain?.isEmpty == false ? wire.domain : nil
        skillPath = wire.skill_path
        routineId = wire.routine_id
        workflowStepCount = wire.workflow?.steps?.count ?? 0
        updatedAt = wire.updated_at ?? ""
    }

    /// The same row with fields changed, `updated_at` bumped.
    func with(_ changes: [String: JSONValue]) -> TeachSkill? {
        var next = raw
        for (key, value) in changes { next[key] = value }
        next["updated_at"] = .string(ISO8601DateFormatter().string(from: Date()))
        return TeachSkill(raw: next)
    }

    /// "harbor.example · 4 steps" (TeachSkillList).
    var subtitle: String {
        let count = "\(steps.count) step\(steps.count == 1 ? "" : "s")"
        return domain.map { "\($0) · \(count)" } ?? count
    }
}

/// A recording not analyzed into a skill yet (`SiteApiContract`).
struct TeachContract: Decodable, Sendable, Identifiable, Equatable {
    struct Endpoint: Decodable, Sendable, Equatable { let id: String }
    let id: String
    let domain: String
    let endpoints: [Endpoint]
}

// MARK: - Prompt (teach-prompt.ts / teach-format.ts)

enum TeachPrompt {
    /// Replace `{{name}}` with its value; unknown names stay as they are.
    static func fill(_ text: String, _ values: [String: String]) -> String {
        guard let regex = try? NSRegularExpression(pattern: #"\{\{\s*([a-zA-Z0-9_.-]+)\s*\}\}"#) else { return text }
        var out = text
        for match in regex.matches(in: text, range: NSRange(text.startIndex..., in: text)).reversed() {
            guard let whole = Range(match.range, in: text), let nameRange = Range(match.range(at: 1), in: text) else { continue }
            if let value = values[String(text[nameRange])], !value.isEmpty {
                out.replaceSubrange(whole, with: value)
            }
        }
        return out
    }

    /// `Run the skill "<title>" with inputs: … Steps: …` (renderTeachPrompt).
    static func render(_ skill: TeachSkill, values: [String: String]) -> String {
        let inputs = skill.inputs.isEmpty
            ? "none"
            : skill.inputs.map { "\($0.name) = \(values[$0.name] ?? $0.example)" }.joined(separator: ", ")
        var lines = ["Run the skill \"\(skill.title)\" with inputs: \(inputs)."]
        let goal = skill.intent.trimmingCharacters(in: .whitespacesAndNewlines)
        if !goal.isEmpty { lines.append("Goal: \(fill(goal, values))") }
        if let domain = skill.domain { lines.append("Site: \(domain)") }
        lines.append("Steps:")
        lines.append(skill.steps.enumerated().map { index, step in
            let text = fill(step.text.trimmingCharacters(in: .whitespacesAndNewlines), values)
            let target = step.kind == "api" && step.target != nil ? " (\(fill(step.target!, values)))" : ""
            return "\(index + 1). \(text)\(target)"
        }.joined(separator: "\n"))
        if skill.steps.contains(where: { $0.kind == "api" }) {
            lines.append("For API steps, call the API with the user's signed-in session instead of clicking through the page.")
        }
        lines.append("Ask for approval before any step that submits, pays, sends or deletes.")
        return lines.joined(separator: "\n")
    }

    /// The page a replay starts on: the first URL in the steps, else the
    /// skill's site (teach-replay.ts teachStartUrl).
    static func startURL(_ skill: TeachSkill) -> URL? {
        let detector = try? NSDataDetector(types: NSTextCheckingResult.CheckingType.link.rawValue)
        for step in skill.steps {
            for text in [step.target, step.text].compactMap({ $0 }) {
                if let match = detector?.firstMatch(in: text, range: NSRange(text.startIndex..., in: text)),
                   let url = match.url, url.scheme?.hasPrefix("http") == true {
                    return url
                }
            }
        }
        return skill.domain.flatMap { URL(string: "https://\($0)") }
    }
}

// MARK: - Client

/// Teach skills and recordings: `/api/har-derived-api/*` on allternit-api
/// (mounted under `/api`, not the `/api/v1` router).
struct TeachClient: Sendable {
    static let shared = TeachClient()

    /// The skills list is per account: 401 means signed out (SIGN_IN_TO_SAVE).
    static let signInToSave = "Sign in to save skills."

    private let client: APIClient

    init(client: APIClient = .shared) {
        self.client = client
    }

    private var root: URL {
        let base = client.baseURL.lastPathComponent == "v1" ? client.baseURL.deletingLastPathComponent() : client.baseURL
        return base.appendingPathComponent("har-derived-api")
    }

    private func call(_ path: String, method: String = "GET", body: Data? = nil) async throws -> Data {
        var request = try await client.authorizedRequest(url: root.appendingPathComponent(path), method: method)
        request.httpBody = body
        let (data, response) = try await client.send(request)
        try client.validate(response, data: data)
        return data
    }

    func listSkills() async throws -> [TeachSkill] {
        struct Response: Decodable { let skills: [[String: JSONValue]]? }
        let response = try JSONDecoder().decode(Response.self, from: try await call("teach/skills"))
        return (response.skills ?? []).compactMap(TeachSkill.init(raw:))
    }

    @discardableResult
    func saveSkill(_ skill: TeachSkill) async throws -> TeachSkill {
        let escaped = skill.id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed.subtracting(CharacterSet(charactersIn: "/"))) ?? skill.id
        let data = try await call("teach/skills/\(escaped)", method: "PUT", body: try JSONEncoder().encode(skill.raw))
        // Some upserts answer 204 / {}: keep what we sent then.
        if let raw = try? JSONDecoder().decode([String: JSONValue].self, from: data), raw["id"] != nil,
           let saved = TeachSkill(raw: raw) {
            return saved
        }
        return skill
    }

    func deleteSkill(id: String) async throws {
        let escaped = id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed.subtracting(CharacterSet(charactersIn: "/"))) ?? id
        _ = try await call("teach/skills/\(escaped)", method: "DELETE")
    }

    func listContracts() async throws -> [TeachContract] {
        struct Response: Decodable { let contracts: [TeachContract]? }
        return (try JSONDecoder().decode(Response.self, from: try await call("contracts")).contracts) ?? []
    }

    /// An imported .har → a contract (`POST /ingest`), then the model's
    /// draft (`POST /teach/distill`), saved as a new skill.
    func analyze(har: String?, contractId: String?, note: String) async throws -> TeachSkill {
        var contract = contractId
        var domain: String? = nil
        if contract == nil, let har {
            struct Ingest: Encodable { let har: String; let source: String }
            struct Ingested: Decodable { let contract_id: String; let domain: String? }
            let ingested = try JSONDecoder().decode(
                Ingested.self,
                from: try await call("ingest", method: "POST", body: try JSONEncoder().encode(Ingest(har: har, source: "upload")))
            )
            contract = ingested.contract_id
            domain = ingested.domain
        }
        var body: [String: JSONValue] = ["events": .array([])]
        if let contract { body["contract_id"] = .string(contract) }
        let trimmed = note.trimmingCharacters(in: .whitespacesAndNewlines)
        if !trimmed.isEmpty { body["note"] = .string(trimmed) }
        guard case .object(var draft)? = try? JSONDecoder().decode(
            JSONValue.self,
            from: try await call("teach/distill", method: "POST", body: try JSONEncoder().encode(body))
        ) else { throw APIError.invalidResponse }

        // teachSkillFromDraft: ids for the skill and its steps, timestamps.
        let now = ISO8601DateFormatter().string(from: Date())
        if case .array(let steps)? = draft["steps"] {
            draft["steps"] = .array(steps.map { step in
                guard case .object(var fields) = step else { return step }
                if case .string(let id)? = fields["id"], !id.isEmpty { return step }
                fields["id"] = .string(Self.newId("step"))
                return .object(fields)
            })
        }
        draft["id"] = .string(Self.newId("teach"))
        if let contract { draft["contract_id"] = .string(contract) }
        if let domain, !domain.isEmpty { draft["domain"] = .string(domain) }
        if draft["inputs"] == nil { draft["inputs"] = .array([]) }
        draft["created_at"] = .string(now)
        draft["updated_at"] = .string(now)
        guard let skill = TeachSkill(raw: draft) else { throw APIError.invalidResponse }
        return skill
    }

    /// newTeachId: `<prefix>-<12 hex>`.
    static func newId(_ prefix: String) -> String {
        "\(prefix)-\(UUID().uuidString.replacingOccurrences(of: "-", with: "").prefix(12).lowercased())"
    }
}
