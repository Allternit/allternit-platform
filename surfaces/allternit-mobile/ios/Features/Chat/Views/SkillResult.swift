import Foundation

/// A loaded skill's result text, as `load_skill` sends it (contract "Amendments
/// after Wave 0"): line 1 `Skill <id> from <from>. This text comes from that server.`,
/// the SKILL.md body, an optional `ARGUMENTS:` block, then the todo reminder.
/// Mirrors the web parser; steps are top-level `1.` / `1)` lines, max 20, plain text.
struct SkillResult: Equatable, Sendable {
    let id: String
    let from: String
    let steps: [String]

    static let maxSteps = 20

    static func parse(_ text: String) -> SkillResult? {
        let lines = text.components(separatedBy: .newlines)
        guard let first = lines.first,
              let head = first.range(of: #"^Skill (\S+) from (.+?)\.( This text comes from that server\.)?\s*$"#, options: .regularExpression)
        else { return nil }
        let header = String(first[head])
        let afterSkill = header.dropFirst("Skill ".count)
        guard let sep = afterSkill.range(of: " from ") else { return nil }
        let id = String(afterSkill[..<sep.lowerBound])
        var from = String(afterSkill[sep.upperBound...])
        if let tail = from.range(of: " This text comes from that server.") { from = String(from[..<tail.lowerBound]) }
        if from.hasSuffix(".") { from.removeLast() }

        var steps: [String] = []
        for line in lines.dropFirst() {
            if line.hasPrefix("ARGUMENTS:") { break }
            guard steps.count < maxSteps,
                  let r = line.range(of: #"^\d+[.)]\s+(.+)$"#, options: .regularExpression) else { continue }
            let body = String(line[r]).replacingOccurrences(of: #"^\d+[.)]\s+"#, with: "", options: .regularExpression)
            let plain = stripMarkdown(body)
            if !plain.isEmpty { steps.append(plain) }
        }
        return SkillResult(id: id, from: from, steps: steps)
    }

    static func stripMarkdown(_ s: String) -> String {
        var t = s
        t = t.replacingOccurrences(of: #"\[([^\]]+)\]\([^)]*\)"#, with: "$1", options: .regularExpression)
        t = t.replacingOccurrences(of: #"[*_`~]+"#, with: "", options: .regularExpression)
        return t.trimmingCharacters(in: .whitespaces)
    }

    /// A `load_skill` tool (plain or MCP-prefixed, e.g. `nimbrook_load_skill`).
    static func isLoadSkill(_ toolName: String) -> Bool {
        toolName.lowercased().range(of: #"(^|_)load_skill$"#, options: .regularExpression) != nil
    }

    static func reason(code: String, from: String) -> String {
        switch code {
        case "skill_not_found": return "This skill is no longer on the server."
        case "skill_disabled": return "This skill is turned off in Settings."
        case "connector_unavailable": return "The server didn't answer."
        case "connector_sign_in_required": return "Sign in to \(from) again in Connections."
        case "skill_too_large": return "This skill is too large to load."
        case "skill_read_timeout": return "The server took too long to send this skill."
        default: return "Something went wrong loading this skill."
        }
    }

    /// The error code inside a failed call's text, if any.
    static func code(in text: String?) -> String? {
        guard let text else { return nil }
        for c in ["skill_not_found", "skill_disabled", "connector_unavailable", "connector_sign_in_required", "skill_too_large", "skill_read_timeout"]
        where text.contains(c) { return c }
        return nil
    }

    /// Which steps are ticked, from the turn's todo calls: the last todo snapshot
    /// wins; a step ticks when a completed todo's text matches it (case-insensitive,
    /// either contains the other). nil when the turn has no todo call (plain list).
    static func ticks(steps: [String], calls: [ToolCallRecord]) -> [Bool]? {
        guard let todo = calls.last(where: { $0.isTodo }) else { return nil }
        var items: [(String, Bool)] = []
        for raw in [todo.input["todos"], todo.output].compactMap({ $0 }) {
            guard let data = raw.data(using: .utf8),
                  let obj = try? JSONSerialization.jsonObject(with: data) else { continue }
            var arr = obj as? [[String: Any]]
            if arr == nil, let d = obj as? [String: Any] { arr = (d["todos"] as? [[String: Any]]) ?? ((d["metadata"] as? [String: Any])?["todos"] as? [[String: Any]]) }
            guard let arr else { continue }
            items = arr.map { (($0["content"] ?? $0["text"] ?? $0["title"]) as? String ?? "", ($0["status"] as? String) == "completed") }
            break
        }
        let done = items.filter { $0.1 }.map { $0.0.lowercased() }
        return steps.map { step in
            let s = step.lowercased()
            return done.contains { !$0.isEmpty && ($0.contains(s) || s.contains($0)) }
        }
    }
}

/// Skill id to display name, so a step row (built off the main actor) can say
/// "Loaded skill, <name>". Filled by ServerSkillsStore when the list loads.
enum SkillNames {
    private static let lock = NSLock()
    nonisolated(unsafe) private static var names: [String: String] = [:]
    static func set(_ skills: [ServerSkill]) {
        lock.lock(); defer { lock.unlock() }
        names = Dictionary(skills.map { ($0.id, $0.name) }, uniquingKeysWith: { a, _ in a })
    }
    static func name(for id: String) -> String {
        lock.lock(); defer { lock.unlock() }
        return names[id] ?? id
    }
}
