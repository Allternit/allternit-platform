import Foundation

/// The person's subagent rows (custom types, and overrides of a built-in
/// keyed by the same slug), allternit-api
/// cmd/allternit-api/src/subagent_type_routes.rs (snake_case):
///
/// - `GET    /api/v1/subagent-types` → `{ types: [row] }`
/// - `POST   /api/v1/subagent-types` `{ slug, name, … }` → 201 `{ type }` (409 for a built-in or taken slug)
/// - `PATCH  /api/v1/subagent-types/:slug` → `{ type }` (upserts a built-in override)
/// - `DELETE /api/v1/subagent-types/:slug` → 204 (deletes a custom type, resets a built-in)
struct SubagentTypeRow: Codable, Equatable, Sendable {
    let slug: String
    let name: String?
    let description: String?
    let prompt: String?
    let modelClass: String?
    let tools: [String]?
    let maxSteps: Int?
    let look: SubagentLook?
    let isCustom: Bool

    enum CodingKeys: String, CodingKey {
        case slug, name, description, prompt, tools, look
        case modelClass = "model_class"
        case maxSteps = "max_steps"
        case isCustom = "is_custom"
    }
}

/// The fields Settings sends (built-ins never send `prompt`: theirs is managed by Allternit).
struct SubagentTypeFields: Encodable, Sendable {
    var slug: String? = nil
    let name: String
    let description: String
    let modelClass: String
    let tools: [String]
    let maxSteps: Int
    let look: SubagentLook
    var prompt: String? = nil

    enum CodingKeys: String, CodingKey {
        case slug, name, description, tools, look, prompt
        case modelClass = "model_class"
        case maxSteps = "max_steps"
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encodeIfPresent(slug, forKey: .slug)
        try c.encode(name, forKey: .name)
        try c.encode(description, forKey: .description)
        try c.encode(modelClass, forKey: .modelClass)
        try c.encode(tools, forKey: .tools)
        try c.encode(maxSteps, forKey: .maxSteps)
        try c.encode(look, forKey: .look)
        try c.encodeIfPresent(prompt, forKey: .prompt)
    }
}

final class SubagentTypesClient: @unchecked Sendable {
    static let shared = SubagentTypesClient()
    private let client: APIClient

    init(client: APIClient = .shared) { self.client = client }

    private struct ListBody: Decodable { let types: [SubagentTypeRow]? }
    private struct RowBody: Decodable { let type: SubagentTypeRow }

    func list() async throws -> [SubagentTypeRow] {
        let body: ListBody = try await client.get(path: "subagent-types")
        return body.types ?? []
    }

    func create(_ fields: SubagentTypeFields) async throws -> SubagentTypeRow {
        let body: RowBody = try await client.post(path: "subagent-types", body: fields)
        return body.type
    }

    func save(slug: String, _ fields: SubagentTypeFields) async throws -> SubagentTypeRow {
        var request = try await client.authorizedRequest(path: "subagent-types/\(settingsPathSegment(slug))", method: "PATCH")
        request.httpBody = try JSONEncoder().encode(fields)
        let (data, response) = try await client.send(request)
        try client.validate(response, data: data)
        do { return try JSONDecoder().decode(RowBody.self, from: data).type } catch { throw APIError.decoding(error) }
    }

    func remove(slug: String) async throws {
        try await client.delete(path: "subagent-types/\(settingsPathSegment(slug))")
    }
}

/// One subagent type as Settings shows it: a built-in (maybe edited) or one
/// of the person's own (web lib/subagents/catalog.ts `mergeCatalog`).
struct SubagentTypeEntry: Identifiable, Equatable, Sendable {
    let slug: String
    var name: String
    var description: String
    var modelClass: String
    var tools: [String]
    var maxSteps: Int?
    var prompt: String?
    var look: SubagentLook
    let builtin: Bool
    var overridden = false

    var id: String { slug }

    struct ModelClass: Identifiable, Sendable {
        let id: String
        let label: String
        let hint: String
    }

    static let modelClasses: [ModelClass] = [
        ModelClass(id: "small", label: "Small", hint: "Cheap and fast: search, tests, logs, docs, summaries."),
        ModelClass(id: "standard", label: "Standard", hint: "Everyday work: edits, reviews, browsing."),
        ModelClass(id: "deep", label: "Deep", hint: "Hard thinking: plans, security reviews."),
    ]

    /// Tools a person can grant (SubagentsPanel.tsx `SUBAGENT_TOOL_OPTIONS`).
    static let toolOptions = ["read", "grep", "glob", "edit", "bash", "web search", "fetch", "browser", "computer", "git", "gh", "python"]

    static func label(forClass id: String) -> String {
        modelClasses.first { $0.id == id }?.label ?? id.capitalized
    }

    private static func b(_ slug: String, _ name: String, _ modelClass: String, _ face: String, _ tools: [String], _ description: String, _ maxSteps: Int) -> SubagentTypeEntry {
        SubagentTypeEntry(slug: slug, name: name, description: description, modelClass: modelClass, tools: tools,
                          maxSteps: maxSteps, prompt: nil, look: SubagentLook(pack: SubagentCatalog.pack, face: face), builtin: true)
    }

    /// The 11 built-ins (catalog.ts `BUILTIN_SUBAGENTS`).
    static let builtins: [SubagentTypeEntry] = [
        b("explorer", "Explorer", "small", "kest", ["read", "grep", "glob"], "Searches code and files. Read-only.", 40),
        b("builder", "Builder", "standard", "daniel", ["read", "edit", "bash"], "Makes one scoped change in its own worktree.", 80),
        b("code-reviewer", "Code Reviewer", "standard", "maya", ["read", "grep", "git diff"], "Finds bugs in a diff. Read-only.", 40),
        b("verifier", "Verifier", "small", "ray", ["bash", "read"], "Runs typecheck, tests and lint. Reports the failures.", 30),
        b("web-researcher", "Web Researcher", "standard", "saanvi", ["web search", "fetch"], "Searches the web. Writes a cited summary.", 40),
        b("browser-operator", "Browser Operator", "standard", "li-wei", ["browser"], "Uses websites. Stops before it pays, posts or sends.", 60),
        b("computer-operator", "Computer Operator", "standard", "mara", ["computer"], "Uses desktop apps on a computer.", 60),
        b("data-analyst", "Data Analyst", "standard", "amir", ["read", "python", "charts"], "Reads data. Makes charts and tables.", 40),
        b("ui-designer", "UI Designer", "standard", "kira", ["read", "edit", "screenshots"], "Makes mockups from the real CSS.", 60),
        b("release-engineer", "Release Engineer", "standard", "grant", ["bash", "git", "gh"], "Builds, release notes and PRs. Never deploys without a go.", 60),
        b("security-reviewer", "Security Reviewer", "deep", "jonah", ["read", "grep"], "Checks for leaked secrets, auth holes and injection. Read-only.", 40),
    ]

    /// Old names that map onto a built-in (catalog.ts `SUBAGENT_ALIASES`): not usable as new slugs.
    static let reservedAliases: Set<String> = ["general", "general-purpose", "worker", "default", "explore", "verification"]

    @MainActor static func merge(_ rows: [SubagentTypeRow]) -> [SubagentTypeEntry] {
        let bySlug = Dictionary(rows.map { ($0.slug, $0) }, uniquingKeysWith: { _, last in last })
        var out = builtins.map { base -> SubagentTypeEntry in
            guard let r = bySlug[base.slug] else { return base }
            var t = base
            if let name = r.name, !name.isEmpty { t.name = name }
            if let description = r.description, !description.isEmpty { t.description = description }
            if let modelClass = r.modelClass, !modelClass.isEmpty { t.modelClass = modelClass }
            if let tools = r.tools, !tools.isEmpty { t.tools = tools }
            t.maxSteps = r.maxSteps ?? base.maxSteps
            t.prompt = r.prompt
            if let look = r.look { t.look = look }
            t.overridden = true
            return t
        }
        let builtinSlugs = Set(builtins.map(\.slug))
        for r in rows where r.isCustom && !builtinSlugs.contains(r.slug) {
            out.append(SubagentTypeEntry(
                slug: r.slug,
                name: (r.name?.isEmpty == false ? r.name : nil) ?? r.slug,
                description: r.description ?? "",
                modelClass: r.modelClass ?? "small",
                tools: r.tools ?? [],
                maxSteps: r.maxSteps,
                prompt: r.prompt,
                look: r.look ?? SubagentCatalog.shared.look(for: r.slug),
                builtin: false
            ))
        }
        return out
    }

    /// "Release Notes Writer!" → "release-notes-writer" (subagent-slug.ts).
    static func deriveSlug(_ name: String) -> String {
        let folded = name.folding(options: [.diacriticInsensitive, .widthInsensitive], locale: nil).lowercased()
        var out = ""
        var lastDash = false
        for scalar in folded.unicodeScalars {
            let isAlnum = (scalar >= "a" && scalar <= "z") || (scalar >= "0" && scalar <= "9")
            if isAlnum {
                out.unicodeScalars.append(scalar)
                lastDash = false
            } else if !lastDash {
                out.append("-")
                lastDash = true
            }
        }
        while out.hasPrefix("-") { out.removeFirst() }
        out = String(out.prefix(40))
        while out.hasSuffix("-") { out.removeLast() }
        return out
    }

    /// Why a slug can't be used for a new custom type, or nil (subagent-slug.ts `subagentSlugProblem`).
    static func slugProblem(_ slug: String, taken: [String]) -> String? {
        if slug.isEmpty { return "Give it a name with at least two letters or numbers." }
        let valid = slug.range(of: "^[a-z0-9][a-z0-9-]{1,39}$", options: .regularExpression) != nil
        if !valid { return "Use 2–40 lowercase letters, numbers or dashes, starting with a letter or number." }
        if builtins.contains(where: { $0.slug == slug }) || reservedAliases.contains(slug) {
            return "That name belongs to a built-in subagent. Open the built-in to change it, or pick another name."
        }
        if taken.contains(slug) { return "You already have a subagent with this name." }
        return nil
    }
}
