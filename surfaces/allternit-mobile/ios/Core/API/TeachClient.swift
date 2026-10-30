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
    /// date: 'YYYY-MM-DD' | 'MM/DD/YYYY' …; number: a locale like 'de-DE'.
    let format: String?
    /// file/folder: the allowed root, may be '{{profile.key}}'.
    let root: String?
    /// secret: the saved login's credential id; the value is never stored.
    let secretRef: String?

    private enum CodingKeys: String, CodingKey {
        case name, label, example, description, type, required, choices, format, root
        case secretRef = "secret_ref"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        name = try c.decode(String.self, forKey: .name)
        label = ((try? c.decodeIfPresent(String.self, forKey: .label)) ?? nil) ?? ""
        example = ((try? c.decodeIfPresent(String.self, forKey: .example)) ?? nil) ?? ""
        description = (try? c.decodeIfPresent(String.self, forKey: .description)) ?? nil
        type = (try? c.decodeIfPresent(String.self, forKey: .type)) ?? nil
        required = (try? c.decodeIfPresent(Bool.self, forKey: .required)) ?? nil
        choices = (try? c.decodeIfPresent([String].self, forKey: .choices)) ?? nil
        format = (try? c.decodeIfPresent(String.self, forKey: .format)) ?? nil
        root = (try? c.decodeIfPresent(String.self, forKey: .root)) ?? nil
        secretRef = (try? c.decodeIfPresent(String.self, forKey: .secretRef)) ?? nil
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

// MARK: - Editing (TeachEditor)

/// A skill being edited on the phone (TeachEditor's local draft). Edits stay
/// here until "Save changes"; `apply` writes them back into the skill's raw
/// row, keeping every field the phone doesn't edit (a step's or input's own
/// extra fields included).
struct TeachDraft: Equatable {
    struct Step: Identifiable, Equatable {
        var id: String
        /// api | ui | shell | note
        var kind: String
        var text: String
        var target: String
        var raw: [String: JSONValue]
    }

    struct Input: Identifiable, Equatable {
        /// Row identity only; renaming an input keeps its row.
        let id: UUID
        var name: String
        var label: String
        var example: String
        /// text | number | … | secret (TEACH_INPUT_TYPES)
        var type: String
        /// Default true.
        var required: Bool
        /// date / datetime / number only.
        var format: String
        /// file / folder only.
        var root: String
        /// secret only: the saved login's credential id.
        var secretRef: String
        /// choice only, one per line as typed.
        var choices: String
        var raw: [String: JSONValue]

        var isSecret: Bool { type == "secret" }
        var hasFormat: Bool { type == "date" || type == "datetime" || type == "number" }
        var hasRoot: Bool { type == "file" || type == "folder" }

        /// TeachEditor's type change: a secret drops its example, and only
        /// dates, times and numbers keep a format.
        mutating func setType(_ next: String) {
            type = next
            if next == "secret" { example = "" }
            if !hasFormat { format = "" }
        }
    }

    var title: String
    var intent: String
    var steps: [Step]
    var inputs: [Input]

    static let stepKinds = ["api", "ui", "shell", "note"]
    /// TeachEditor KIND_LABEL.
    static func stepKindLabel(_ kind: String) -> String {
        switch kind {
        case "api": return "API"
        case "ui": return "Page"
        case "shell": return "Shell"
        default: return "Note"
        }
    }

    /// TeachEditor TYPE_LABEL, in its order.
    static let inputTypes: [(type: String, label: String)] = [
        ("text", "Text"), ("number", "Number"), ("date", "Date"), ("datetime", "Date and time"),
        ("email", "Email"), ("url", "Web address"), ("choice", "Choice"), ("file", "File"),
        ("folder", "Folder"), ("list", "List"), ("secret", "Secret"),
    ]

    static func inputTypeLabel(_ type: String) -> String {
        inputTypes.first { $0.type == type }?.label ?? "Text"
    }

    init(_ skill: TeachSkill) {
        if case .string(let t)? = skill.raw["title"] { title = t } else { title = "" }
        intent = skill.intent
        let rawSteps: [[String: JSONValue]] = {
            guard case .array(let items)? = skill.raw["steps"] else { return [] }
            return items.compactMap { item -> [String: JSONValue]? in
                if case .object(let o) = item { return o }
                return nil
            }
        }()
        steps = skill.steps.enumerated().map { index, step in
            let raw = rawSteps.first { $0["id"] == .string(step.id) } ?? (index < rawSteps.count ? rawSteps[index] : [:])
            return Step(id: step.id, kind: step.kind, text: step.text, target: step.target ?? "", raw: raw)
        }
        let rawInputs: [[String: JSONValue]] = {
            guard case .array(let items)? = skill.raw["inputs"] else { return [] }
            return items.compactMap { item -> [String: JSONValue]? in
                if case .object(let o) = item { return o }
                return nil
            }
        }()
        inputs = skill.inputs.map { input in
            Input(id: UUID(), name: input.name, label: input.label, example: input.example,
                  type: input.type ?? "text", required: input.required != false,
                  format: input.format ?? "", root: input.root ?? "", secretRef: input.secretRef ?? "",
                  choices: (input.choices ?? []).joined(separator: "\n"),
                  raw: rawInputs.first { $0["name"] == .string(input.name) } ?? [:])
        }
    }

    /// A new, empty step on the page (TeachEditor "Add step").
    mutating func addStep() -> String {
        let id = TeachClient.newId("step")
        steps.append(Step(id: id, kind: "ui", text: "", target: "", raw: [:]))
        return id
    }

    /// `input_N`, the first free N (InputsTable "Add input").
    mutating func addInput() {
        var n = inputs.count + 1
        while inputs.contains(where: { $0.name == "input_\(n)" }) { n += 1 }
        inputs.append(Input(id: UUID(), name: "input_\(n)", label: "", example: "", type: "text",
                            required: true, format: "", root: "", secretRef: "", choices: "", raw: [:]))
    }

    mutating func moveStep(at index: Int, by delta: Int) {
        let to = index + delta
        guard steps.indices.contains(index), steps.indices.contains(to) else { return }
        steps.swapAt(index, to)
    }

    /// The skill with this draft written in. Touching an input clears its
    /// `needs_review`, as on the web.
    func apply(to skill: TeachSkill) -> TeachSkill? {
        let stepValues: [JSONValue] = steps.map { step in
            var o = step.raw
            o["id"] = .string(step.id)
            o["kind"] = .string(step.kind)
            o["text"] = .string(step.text)
            let target = step.target.trimmingCharacters(in: .whitespacesAndNewlines)
            if step.kind == "api", !target.isEmpty { o["target"] = .string(target) } else { o.removeValue(forKey: "target") }
            return .object(o)
        }
        let inputValues: [JSONValue] = inputs.map { input in
            var o = input.raw
            o["name"] = .string(input.name.trimmingCharacters(in: .whitespacesAndNewlines))
            o["label"] = .string(input.label)
            // Secrets never carry an example (they're never saved in the skill).
            o["example"] = .string(input.isSecret ? "" : input.example)
            if input.type == "text" && o["type"] == nil { o.removeValue(forKey: "type") } else { o["type"] = .string(input.type) }
            // Required is the default: only `false` is written.
            if input.required { o.removeValue(forKey: "required") } else { o["required"] = .bool(false) }
            func set(_ key: String, _ value: String, when on: Bool) {
                let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
                if on && !trimmed.isEmpty { o[key] = .string(trimmed) } else { o.removeValue(forKey: key) }
            }
            set("format", input.format, when: input.hasFormat)
            set("root", input.root, when: input.hasRoot)
            set("secret_ref", input.secretRef, when: input.isSecret)
            let choices = input.choices.split(separator: "\n").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
            if input.type == "choice" { o["choices"] = .array(choices.map { .string($0) }) } else { o.removeValue(forKey: "choices") }
            o.removeValue(forKey: "needs_review")
            return .object(o)
        }
        return skill.with([
            "title": .string(title),
            "intent": .string(intent),
            "steps": .array(stepValues),
            "inputs": .array(inputValues),
        ])
    }

    /// The first problem that blocks a save (teach-validate, the parts the
    /// phone edits): an empty skill name is fine, but inputs need a unique,
    /// valid name.
    var blockingProblem: String? {
        var seen = Set<String>()
        for input in inputs {
            let name = input.name.trimmingCharacters(in: .whitespacesAndNewlines)
            if name.isEmpty { return "Every input needs a name." }
            if name.range(of: #"^[a-zA-Z0-9_.-]+$"#, options: .regularExpression) == nil {
                return "Input names use letters, numbers, _ . and - only (\(name))."
            }
            if !seen.insert(name).inserted { return "Two inputs are named \(name)." }
        }
        return nil
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

// MARK: - Business profile (TeachProfile)

/// Values set once and used in any skill as `{{profile.key}}`. One per
/// person, `PUT /teach/profile`.
struct TeachProfile: Equatable {
    struct Row: Identifiable, Equatable {
        let id: UUID
        var key: String
        var label: String
        var value: String
        /// text | url | email | folder | number
        var type: String
    }

    var rows: [Row]

    /// TeachProfileSheet PROFILE_TYPES.
    static let types: [(type: String, label: String)] = [
        ("text", "Text"), ("url", "Web address"), ("email", "Email"), ("folder", "Folder"), ("number", "Number"),
    ]

    init(rows: [Row] = []) { self.rows = rows }

    init(values: [String: JSONValue]) {
        rows = values.keys.sorted().compactMap { key in
            guard case .object(let entry)? = values[key] else { return nil }
            func text(_ field: String) -> String {
                if case .string(let s)? = entry[field] { return s }
                return ""
            }
            let type = text("type")
            return Row(id: UUID(), key: key, label: text("label"), value: text("value"), type: type.isEmpty ? "text" : type)
        }
    }

    /// The saved shape: `{ values: { key: { value, label?, type? } } }`.
    var values: [String: JSONValue] {
        var out: [String: JSONValue] = [:]
        for row in rows {
            var entry: [String: JSONValue] = ["value": .string(row.value)]
            let label = row.label.trimmingCharacters(in: .whitespacesAndNewlines)
            if !label.isEmpty { entry["label"] = .string(label) }
            if row.type != "text" { entry["type"] = .string(row.type) }
            out[row.key.trimmingCharacters(in: .whitespacesAndNewlines)] = .object(entry)
        }
        return out
    }

    /// profileRowProblems: each row's problem, by index.
    var problems: [Int: String] {
        var problems: [Int: String] = [:]
        var seen = Set<String>()
        for (index, row) in rows.enumerated() {
            let key = row.key.trimmingCharacters(in: .whitespacesAndNewlines)
            if key.isEmpty { problems[index] = "Give it a key, like erp_url." }
            else if key.range(of: #"^[a-zA-Z0-9_-]+$"#, options: .regularExpression) == nil {
                problems[index] = "Use letters, numbers, _ or - in the key."
            } else if seen.contains(key) { problems[index] = "Two rows use the key \(key)." }
            seen.insert(key)
        }
        return problems
    }
}

// MARK: - Recorded steps (teach-workflow-types.ts)

/// One step of the compiled workflow Run plays back, with the runner's
/// words for it (teach-runner.ts stepLabel). Loop bodies come after their
/// `for_each`, one level in.
struct TeachRecordedStep: Identifiable, Equatable {
    let id: String
    let kind: String
    let label: String
    let depth: Int
    /// Submits, pays, sends or deletes: Run asks first.
    let unsafe: Bool

    static func list(_ skill: TeachSkill) -> [TeachRecordedStep] {
        guard case .object(let workflow)? = skill.raw["workflow"], case .array(let steps)? = workflow["steps"] else { return [] }
        return flatten(steps, depth: 0)
    }

    private static func flatten(_ steps: [JSONValue], depth: Int) -> [TeachRecordedStep] {
        var out: [TeachRecordedStep] = []
        for (index, value) in steps.enumerated() {
            guard case .object(let step) = value else { continue }
            let id: String = { if case .string(let s)? = step["id"] { return s }; return "step-\(depth)-\(index)" }()
            let kind: String = { if case .string(let s)? = step["kind"] { return s }; return "" }()
            let unsafe: Bool = { if case .bool(let b)? = step["unsafe_to_repeat"] { return b }; return false }()
            out.append(TeachRecordedStep(id: id, kind: kind, label: label(step), depth: depth, unsafe: unsafe))
            if kind == "for_each", case .array(let children)? = step["steps"] {
                out.append(contentsOf: flatten(children, depth: depth + 1))
            }
        }
        return out
    }

    private static func label(_ step: [String: JSONValue]) -> String {
        func text(_ key: String) -> String? {
            if case .string(let s)? = step[key], !s.isEmpty { return s }
            return nil
        }
        func target(_ key: String = "target") -> String {
            guard case .object(let t)? = step[key] else { return "the element" }
            for field in ["label", "text", "name", "id", "css"] {
                if case .string(let s)? = t[field], !s.isEmpty { return s }
            }
            return "the element"
        }
        if let note = text("note") { return note }
        switch text("kind") ?? "" {
        case "goto": return "Open \(text("url") ?? "")"
        case "click": return "Click “\(target())”"
        case "fill": return "Type \(text("value") ?? "") into “\(target())”"
        case "select": return "Choose \(text("value") ?? "") in “\(target())”"
        case "press": return "Press \(text("key") ?? "")"
        case "wait_for": return "Wait for \(text("url") ?? text("text") ?? target())"
        case "extract": return "Read “\(target())” as \(text("var") ?? "")"
        case "upload": return "Upload \(text("file") ?? "") to “\(target())”"
        case "expect_download": return "Wait for the download \(text("pattern") ?? "")"
        case "for_each": return "For each \(text("as") ?? "") in \(text("list") ?? "")"
        default: return "Step"
        }
    }
}

// MARK: - Skill file (teach-skill-md.ts)

/// The SKILL.md for a skill: the same text the web previews and saves.
enum TeachSkillFile {
    /// capabilityWriter's slugify, so `name:` matches the folder.
    static func slug(_ title: String) -> String {
        let lowered = title.lowercased()
        let dashed = lowered.replacingOccurrences(of: "[^a-z0-9]+", with: "-", options: .regularExpression)
        let trimmed = dashed.trimmingCharacters(in: CharacterSet(charactersIn: "-"))
        return trimmed.isEmpty ? "item" : String(trimmed.prefix(48))
    }

    /// Where `createSkill` writes it, relative to home.
    static func relPath(_ title: String) -> String { ".allternit/skills/\(slug(title))/SKILL.md" }

    /// Quote a one-line YAML value only when it needs it.
    static func yamlScalar(_ value: String) -> String {
        guard value.range(of: #": |\s#|^[-?:,\[\]{}&*!|>'"%@`#]"#, options: .regularExpression) != nil else { return value }
        let data = (try? JSONEncoder().encode(value)) ?? Data()
        return String(data: data, encoding: .utf8) ?? value
    }

    static func render(_ skill: TeachSkill, date: Date = Date()) -> String {
        func oneLine(_ text: String) -> String {
            text.replacingOccurrences(of: #"\s+"#, with: " ", options: .regularExpression).trimmingCharacters(in: .whitespaces)
        }
        var rawTitle = ""
        if case .string(let s)? = skill.raw["title"] { rawTitle = s }
        let title = oneLine(rawTitle).nilIfBlank ?? "Taught skill"
        let intent = skill.intent.trimmingCharacters(in: .whitespacesAndNewlines)
        var lines = ["---", "name: \(slug(title))", "description: \(yamlScalar(oneLine(intent).nilIfBlank ?? title))", "---", "# \(title)"]
        if !intent.isEmpty { lines += ["", intent] }
        lines += ["", "## Inputs"]
        if skill.inputs.isEmpty {
            lines.append("None.")
        } else {
            for input in skill.inputs {
                let label = oneLine(input.label.isEmpty ? input.name : input.label)
                let example = input.example.trimmingCharacters(in: .whitespaces).isEmpty ? "" : " (e.g. \(oneLine(input.example)))"
                lines.append("- `\(input.name)` — \(label)\(example)")
            }
        }
        lines += ["", "## Steps"]
        if skill.steps.isEmpty {
            lines.append("No steps yet.")
        } else {
            for (index, step) in skill.steps.enumerated() {
                let text = oneLine(step.text).nilIfBlank ?? "(empty step)"
                let target = step.kind == "api" ? step.target?.trimmingCharacters(in: .whitespaces).nilIfBlank : nil
                let suffix = target.map { text.contains($0) ? "" : " (`\($0)`)" } ?? ""
                lines.append("\(index + 1). \(text)\(suffix)")
            }
            if skill.steps.contains(where: { $0.kind == "api" }) {
                lines += ["", "For API steps, prefer calling the API with the user's signed-in session over clicking through the page."]
            }
        }
        let day = ISO8601DateFormatter().string(from: date).prefix(10)
        lines += ["", "## Notes",
                  "Taught in Allternit Teach from one recording on \(day). Ask for approval before any step that submits, pays, sends or deletes.",
                  ""]
        return lines.joined(separator: "\n")
    }
}

private extension String {
    var nilIfBlank: String? { trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? nil : self }
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

    /// `GET /teach/profile`. No profile yet (or an older runtime without
    /// the route) starts empty.
    func profile() async throws -> TeachProfile {
        struct Response: Decodable { let values: [String: JSONValue]? }
        do {
            let response = try JSONDecoder().decode(Response.self, from: try await call("teach/profile"))
            return TeachProfile(values: response.values ?? [:])
        } catch APIError.httpError(let status, _) where status == 404 {
            return TeachProfile()
        }
    }

    /// `PUT /teach/profile`; keeps what was sent when the answer is empty.
    @discardableResult
    func saveProfile(_ profile: TeachProfile) async throws -> TeachProfile {
        let body: [String: JSONValue] = [
            "values": .object(profile.values),
            "updated_at": .string(ISO8601DateFormatter().string(from: Date())),
        ]
        let data = try await call("teach/profile", method: "PUT", body: try JSONEncoder().encode(body))
        struct Response: Decodable { let values: [String: JSONValue]? }
        if let values = (try? JSONDecoder().decode(Response.self, from: data))?.values { return TeachProfile(values: values) }
        return profile
    }

    func listContracts() async throws -> [TeachContract] {
        struct Response: Decodable { let contracts: [TeachContract]? }
        return (try JSONDecoder().decode(Response.self, from: try await call("contracts")).contracts) ?? []
    }

    /// An imported .har → a contract (`POST /ingest`), then the model's
    /// draft (`POST /teach/distill`), saved as a new skill.
    func analyze(har: String?, contractId: String?, events: [JSONValue] = [], note: String) async throws -> TeachSkill {
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
        var body: [String: JSONValue] = ["events": .array(events)]
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
