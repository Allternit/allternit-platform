import Foundation

/// One tool call in an assistant reply, as the tool calls rail draws it
/// (port of the web ProcessBlock's tool step, ai #850). Built from the
/// session-worker's tool parts while streaming and from `metadata.parts`
/// on a reloaded chat.
struct ToolCallRecord: Identifiable, Equatable, Sendable {
    enum State: String, Equatable, Sendable {
        case running
        case done
        case failed
    }

    let id: String
    var toolName: String
    var state: State
    /// String-valued input fields (description, subagent_type, path, …).
    var input: [String: String] = [:]
    var title: String? = nil
    var error: String? = nil
    var startedAt: Date? = nil
    var finishedAt: Date? = nil

    /// How long the call ran; live calls count up to `now`.
    func elapsed(now: Date = Date()) -> TimeInterval? {
        guard let startedAt else { return nil }
        let end = finishedAt ?? (state == .running ? now : nil)
        return end.map { max(0, $0.timeIntervalSince(startedAt)) }
    }

    var summary: ToolDescription { ToolDescriber.describe(self) }

    /// A subagent spawn (Task / Agent / delegate tools) — web `isAgentStep`.
    var isAgentStep: Bool { ToolDescriber.isAgentTool(toolName) }

    /// Todo/plan tool calls stay off the rail (Progress lives elsewhere).
    var isTodo: Bool {
        let name = ToolDescriber.normalized(toolName)
        return name.contains("todo") || name == "planwrite"
    }
}

// MARK: - Building records

extension ToolCallRecord {
    /// History: `metadata.parts` tool parts → records.
    static func fromHistory(_ parts: [AgentSessionMessage.Part]) -> [ToolCallRecord] {
        parts.enumerated().compactMap { index, part in
            guard part.type == "tool", let name = part.tool, !name.isEmpty else { return nil }
            let state: State
            switch part.state?.status {
            case "error": state = .failed
            case "pending", "running": state = .running
            default: state = .done
            }
            return ToolCallRecord(
                id: part.id ?? "\(name)-\(index)",
                toolName: name,
                state: state,
                input: part.state?.input ?? [:],
                title: part.state?.title,
                error: part.state?.error,
                startedAt: part.state?.startedAt,
                finishedAt: part.state?.finishedAt
            )
        }
    }
}

extension MessageRecord {
    /// Inserts or updates one tool call from a stream frame. Keeps the
    /// first-seen start time when the runtime sends none.
    mutating func upsertToolCall(id: String, name: String, state: ToolCallRecord.State, detail: AgentChatEvent.ToolDetail?) {
        let now = Date()
        if let index = toolCalls.firstIndex(where: { $0.id == id }) {
            var call = toolCalls[index]
            call.toolName = name.isEmpty ? call.toolName : name
            call.state = state
            if let detail {
                if !detail.input.isEmpty { call.input = detail.input }
                call.title = detail.title ?? call.title
                call.error = detail.error ?? call.error
                call.startedAt = detail.startedAt ?? call.startedAt
                call.finishedAt = detail.finishedAt ?? call.finishedAt
            }
            if state != .running, call.finishedAt == nil { call.finishedAt = now }
            toolCalls[index] = call
        } else {
            toolCalls.append(ToolCallRecord(
                id: id,
                toolName: name,
                state: state,
                input: detail?.input ?? [:],
                title: detail?.title,
                error: detail?.error,
                startedAt: detail?.startedAt ?? now,
                finishedAt: detail?.finishedAt ?? (state == .running ? nil : now)
            ))
        }
    }
}

// MARK: - Describing a step

/// What a step's row says: the verb, what it acted on, and an SF Symbol.
struct ToolDescription: Equatable, Sendable {
    var verb: String
    var detail: String
    var symbol: String
    /// Subagent type slug for agent steps ("explore", "general-purpose").
    var subagent: String? = nil
}

/// Port of the web `describeTool` (describe-tool.ts), trimmed to what a
/// phone row shows.
enum ToolDescriber {
    static func normalized(_ name: String) -> String {
        var n = name.lowercased()
        if n.hasPrefix("tool-") { n.removeFirst(5) }
        if let range = n.range(of: "__", options: .backwards) { n = String(n[range.upperBound...]) }
        return n.replacingOccurrences(of: "_", with: "").replacingOccurrences(of: "-", with: "").replacingOccurrences(of: ".", with: "")
    }

    static func isAgentTool(_ name: String) -> Bool {
        let raw = name.lowercased().replacingOccurrences(of: "tool-", with: "")
        if raw == "task" || raw == "agent" || raw.contains("subagent") { return true }
        return ["delegate_task", "spawn_agent", "collab_spawn", "collab_agent_spawn"].contains(raw)
    }

    static func describe(_ call: ToolCallRecord) -> ToolDescription {
        let running = call.state == .running
        let name = normalized(call.toolName)
        let inp = call.input
        func s(_ keys: String...) -> String {
            for key in keys { if let v = inp[key]?.trimmingCharacters(in: .whitespacesAndNewlines), !v.isEmpty { return firstLine(v) } }
            return ""
        }
        let path = s("file_path", "filePath", "path", "filename")
        let url = s("url", "href")
        let command = s("command", "cmd", "script")
        let query = s("query", "q", "search", "pattern")

        if isAgentTool(call.toolName) {
            let subagent = s("subagent_type", "agent", "subagent", "agent_role", "role", "agent_type")
            let detail = s("description").isEmpty ? String(s("prompt", "message", "task").prefix(80)) : s("description")
            return ToolDescription(verb: SubagentNames.title(for: subagent.isEmpty ? nil : subagent), detail: detail, symbol: "person.2.fill", subagent: subagent.isEmpty ? "agent" : subagent)
        }
        if name.contains("websearch") || name.contains("googlesearch") || name == "search" {
            return ToolDescription(verb: running ? "Searching the web" : "Searched the web", detail: query, symbol: "globe")
        }
        if name.contains("fetch") || name.contains("browseurl") || name.contains("readurl") || name.contains("webpage") || name.contains("scrape") || (!url.isEmpty && path.isEmpty && command.isEmpty) {
            return ToolDescription(verb: running ? "Reading" : "Read", detail: shortURL(url), symbol: "doc.richtext")
        }
        if name.contains("bash") || name.contains("shell") || name.contains("terminal") || name.contains("exec") || name.contains("runcode") {
            let description = s("description")
            if !description.isEmpty { return ToolDescription(verb: description, detail: "", symbol: "terminal") }
            return ToolDescription(verb: running ? "Running" : "Ran", detail: command, symbol: "terminal")
        }
        if name.contains("multiedit") || name.contains("strreplace") || name.contains("editfile") || name == "edit" || name == "patch" {
            return ToolDescription(verb: running ? "Editing" : "Edited", detail: basename(path), symbol: "pencil")
        }
        if name.contains("writefile") || name.contains("createfile") || name == "write" {
            return ToolDescription(verb: running ? "Creating" : "Created", detail: basename(path), symbol: "doc.badge.plus")
        }
        if name.contains("readfile") || name == "read" || name == "view" {
            return ToolDescription(verb: running ? "Reading" : "Read", detail: basename(path), symbol: "doc.text")
        }
        if name.contains("grep") || name.contains("searchcode") {
            return ToolDescription(verb: running ? "Searching code" : "Searched code", detail: query, symbol: "magnifyingglass")
        }
        if name.contains("glob") || name.contains("findfiles") || name.contains("listfiles") || name == "ls" || name == "list" {
            return ToolDescription(verb: running ? "Finding files" : "Found files", detail: query.isEmpty ? path : query, symbol: "folder")
        }
        if name.contains("memory") {
            return ToolDescription(verb: running ? "Using Memory Drive" : "Used Memory Drive", detail: s("query", "text"), symbol: "brain")
        }
        if name == "messageagent" {
            let target = s("target").replacingOccurrences(of: "@", with: "")
            return ToolDescription(verb: running ? "Handing to" : "Handed to", detail: target, symbol: "arrowshape.turn.up.right")
        }
        if name == "question" || name.contains("askuser") {
            return ToolDescription(verb: running ? "Asking you" : "Asked you", detail: s("question"), symbol: "questionmark.bubble")
        }
        let label = call.title.flatMap { $0.isEmpty ? nil : $0 } ?? humanize(call.toolName)
        let firstString = inp.values.sorted().first.map(firstLine) ?? ""
        return ToolDescription(verb: "\(running ? "Using" : "Used") \(label)", detail: String(firstString.prefix(120)), symbol: "wrench.and.screwdriver")
    }

    private static func firstLine(_ text: String) -> String {
        String(text.split(separator: "\n", omittingEmptySubsequences: true).first ?? "").trimmingCharacters(in: .whitespaces)
    }

    private static func basename(_ path: String) -> String {
        path.split(separator: "/").last.map(String.init) ?? path
    }

    private static func shortURL(_ url: String) -> String {
        guard let components = URLComponents(string: url), let host = components.host else { return url }
        let host2 = host.hasPrefix("www.") ? String(host.dropFirst(4)) : host
        return components.path.count > 1 ? "\(host2)\(components.path)" : host2
    }

    /// "mcp__github__list_issues" → "List issues"; "webfetch" → "Webfetch".
    static func humanize(_ raw: String) -> String {
        var name = raw
        if let range = name.range(of: "__", options: .backwards) { name = String(name[range.upperBound...]) }
        let words = name.replacingOccurrences(of: "_", with: " ").replacingOccurrences(of: "-", with: " ")
        guard let first = words.first else { return "a tool" }
        return first.uppercased() + words.dropFirst()
    }
}

/// Subagent type titles (web lib/subagents/catalog BUILTIN_SUBAGENTS names
/// for the common types; anything else is title-cased from its slug).
enum SubagentNames {
    private static let builtins: [String: String] = [
        "agent": "Agent",
        "general": "General",
        "general-purpose": "General",
        "explore": "Explorer",
        "explorer": "Explorer",
        "plan": "Planner",
        "planner": "Planner",
        "researcher": "Researcher",
        "research": "Researcher",
        "reviewer": "Reviewer",
        "code-reviewer": "Reviewer",
        "build": "Builder",
        "builder": "Builder",
        "tester": "Tester",
        "writer": "Writer",
        "designer": "Designer"
    ]

    static func slug(_ raw: String?) -> String {
        let s = (raw ?? "").trimmingCharacters(in: .whitespaces).lowercased()
            .replacingOccurrences(of: "@", with: "")
            .replacingOccurrences(of: " ", with: "-")
            .replacingOccurrences(of: "_", with: "-")
        return s.isEmpty ? "agent" : s
    }

    static func title(for raw: String?) -> String {
        let key = slug(raw)
        if let name = builtins[key] { return name }
        let words = key.split(separator: "-").map { $0.prefix(1).uppercased() + $0.dropFirst() }
        return words.isEmpty ? "Agent" : words.joined(separator: " ")
    }
}
