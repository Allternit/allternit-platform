import Foundation

// Port of allternit-ai src/components/agent-run-tree/model.ts: one task, the
// agents it handed work to, and theirs. Fed by the reply's agent steps joined
// to the gizzi session tasks (`GET /api/v1/agent-sessions/:id/tasks`).

enum RunStatus: String, Sendable, Equatable {
    case queued, running, done, failed, stopped

    var label: String {
        switch self {
        case .queued: return "Queued"
        case .running: return "Running"
        case .done: return "Done"
        case .failed: return "Failed"
        case .stopped: return "Stopped"
        }
    }

    var isLive: Bool { self == .running || self == .queued }
}

struct RunNode: Identifiable, Equatable, Sendable {
    var id: String
    var name: String
    var slug: String? = nil
    var model: String? = nil
    var status: RunStatus
    var task: String
    var cost: Double? = nil
    var tokens: Int = 0
    var startedAt: Date? = nil
    var finishedAt: Date? = nil
    var error: String? = nil
    var childSessionId: String? = nil
    var children: [RunNode] = []

    func elapsed(now: Date) -> TimeInterval? {
        guard let startedAt else { return nil }
        let end = finishedAt ?? (status.isLive ? now : nil)
        return end.map { max(0, $0.timeIntervalSince(startedAt)) }
    }

    func walk(_ visit: (RunNode, Int) -> Void, depth: Int = 0) {
        visit(self, depth)
        for child in children { child.walk(visit, depth: depth + 1) }
    }

    var anyLive: Bool {
        var live = false
        walk { node, _ in if node.status.isLive { live = true } }
        return live
    }

    /// Spend including everything under this node.
    var subtreeSpend: (cost: Double?, tokens: Int) {
        var cost: Double? = nil
        var tokens = 0
        walk { node, _ in
            if let c = node.cost { cost = (cost ?? 0) + c }
            tokens += node.tokens
        }
        return (cost, tokens)
    }

    var descendantCount: Int {
        var n = 0
        walk { _, depth in if depth > 0 { n += 1 } }
        return n
    }
}

struct RunTotals: Equatable {
    var agents = 0
    var running = 0
    var failed = 0
    var cost: Double? = nil
    var tokens = 0

    init(root: RunNode) {
        var agents = 0, running = 0, failed = 0, tokens = 0
        var cost: Double? = nil
        root.walk { node, depth in
            if depth > 0 {
                agents += 1
                if node.status.isLive { running += 1 }
                if node.status == .failed { failed += 1 }
            }
            if let c = node.cost { cost = (cost ?? 0) + c }
            tokens += node.tokens
        }
        self.agents = agents
        self.running = running
        self.failed = failed
        self.cost = cost
        self.tokens = tokens
    }
}

enum RunFormat {
    static func cost(_ cost: Double) -> String {
        if cost <= 0 { return "$0" }
        if cost < 0.01 { return "<$0.01" }
        return String(format: "$%.2f", cost)
    }

    static func tokens(_ n: Int) -> String {
        if n < 1000 { return "\(n) tok" }
        if n < 1_000_000 { return n < 10_000 ? String(format: "%.1fk tok", Double(n) / 1000) : "\(n / 1000)k tok" }
        return String(format: "%.1fM tok", Double(n) / 1_000_000)
    }

    /// Dollars when the brain bills per token, tokens for $0 subscription
    /// brains, nothing when neither is known (web `spendLabel`).
    static func spend(cost: Double?, tokens: Int) -> String? {
        if let cost, cost > 0 { return self.cost(cost) }
        if tokens > 0 { return self.tokens(tokens) }
        if cost == 0 { return "$0" }
        return nil
    }

    /// "14s", "2m 05s", "1h 3m" (web `formatElapsed`).
    static func elapsed(_ seconds: TimeInterval) -> String {
        let s = max(0, Int(seconds.rounded()))
        if s < 60 { return "\(s)s" }
        let m = s / 60
        if m < 60 { return String(format: "%dm %02ds", m, s % 60) }
        return "\(m / 60)h \(m % 60)m"
    }

    /// Row durations: "820ms", "1.2s", "2m 5s".
    static func duration(_ seconds: TimeInterval) -> String {
        if seconds < 1 { return String(format: "%.0fms", seconds * 1000) }
        if seconds < 10 { return String(format: "%.1fs", seconds) }
        return elapsed(seconds)
    }

    /// "anthropic/claude-sonnet-5-5" → "Sonnet 5.5"; "openai/gpt-6-mini" → "GPT-6 Mini".
    static func modelLabel(_ id: String?) -> String? {
        guard let id, !id.isEmpty else { return nil }
        var name = String(id.split(separator: "/").last ?? Substring(id))
        name = name.replacingOccurrences(of: "::", with: "-")
        if name.hasPrefix("claude-") { name.removeFirst(7) }
        if name.hasSuffix("-latest") { name.removeLast(7) }
        if let r = name.range(of: #"-\d{8}$"#, options: .regularExpression) { name.removeSubrange(r) }
        let upper: Set<String> = ["gpt", "glm", "llm"]
        var out: [String] = []
        for part in name.split(whereSeparator: { "-_ ".contains($0) }).map(String.init) {
            let prev = out.last
            if part.allSatisfy(\.isNumber), let prev, prev.last?.isNumber == true {
                out[out.count - 1] = "\(prev).\(part)"
            } else if part.first?.isNumber == true, let prev, upper.contains(prev.lowercased()) {
                out[out.count - 1] = "\(prev)-\(part)"
            } else if upper.contains(part.lowercased()) {
                out.append(part.uppercased())
            } else {
                out.append(part.prefix(1).uppercased() + part.dropFirst())
            }
        }
        return out.isEmpty ? nil : out.joined(separator: " ")
    }
}

// MARK: - gizzi session tasks

/// One piece of work a session delegated (gizzi `SessionTasks.Item`), with
/// the agent-run-tree fields newer runtimes add (model, cost, tokens,
/// children). Older runtimes omit those; decoding tolerates it.
struct SessionTask: Decodable, Sendable, Equatable, Identifiable {
    struct Time: Decodable, Sendable, Equatable {
        let created: Double
        let finished: Double?
    }
    struct Tokens: Decodable, Sendable, Equatable {
        let input: Int
        let output: Int
    }

    let id: String
    let kind: String
    let status: String
    let description: String
    let agent: String?
    let childSessionID: String?
    let error: String?
    /// A finished shell's output (the Tasks pane).
    let output: String?
    let time: Time
    let model: String?
    let cost: Double?
    let tokens: Tokens?
    let children: [SessionTask]

    private enum CodingKeys: String, CodingKey {
        case id, kind, status, description, agent, childSessionID, error, output, time, model, cost, tokens, children
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        kind = ((try? c.decodeIfPresent(String.self, forKey: .kind)) ?? nil) ?? "subagent"
        status = ((try? c.decodeIfPresent(String.self, forKey: .status)) ?? nil) ?? "running"
        description = ((try? c.decodeIfPresent(String.self, forKey: .description)) ?? nil) ?? ""
        agent = (try? c.decodeIfPresent(String.self, forKey: .agent)) ?? nil
        childSessionID = (try? c.decodeIfPresent(String.self, forKey: .childSessionID)) ?? nil
        error = (try? c.decodeIfPresent(String.self, forKey: .error)) ?? nil
        output = (try? c.decodeIfPresent(String.self, forKey: .output)) ?? nil
        time = ((try? c.decodeIfPresent(Time.self, forKey: .time)) ?? nil) ?? Time(created: 0, finished: nil)
        model = (try? c.decodeIfPresent(String.self, forKey: .model)) ?? nil
        cost = (try? c.decodeIfPresent(Double.self, forKey: .cost)) ?? nil
        tokens = (try? c.decodeIfPresent(Tokens.self, forKey: .tokens)) ?? nil
        children = ((try? c.decodeIfPresent([SessionTask].self, forKey: .children)) ?? nil) ?? []
    }

    var runStatus: RunStatus {
        switch status {
        case "queued": return .queued
        case "running": return .running
        case "completed": return .done
        case "cancelled": return .stopped
        default: return .failed
        }
    }

    var isLive: Bool { status == "queued" || status == "running" }
}

enum RunTreeBuilder {
    static func subagentTasks(_ tasks: [SessionTask]) -> [SessionTask] {
        tasks.filter { $0.kind == "subagent" }.sorted { $0.time.created < $1.time.created }
    }

    static func node(from task: SessionTask) -> RunNode {
        RunNode(
            id: task.id,
            name: SubagentNames.title(for: task.agent),
            slug: SubagentNames.slug(task.agent),
            model: RunFormat.modelLabel(task.model),
            status: task.runStatus,
            task: task.description,
            cost: task.cost,
            tokens: (task.tokens?.input ?? 0) + (task.tokens?.output ?? 0),
            startedAt: task.time.created > 0 ? Date(timeIntervalSince1970: task.time.created / 1000) : nil,
            finishedAt: task.time.finished.map { Date(timeIntervalSince1970: $0 / 1000) },
            error: task.error,
            childSessionId: task.childSessionID,
            children: subagentTasks(task.children).map(node(from:))
        )
    }

    private static func normalize(_ s: String) -> String {
        s.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
            .split(whereSeparator: \.isWhitespace).joined(separator: " ")
    }

    /// A reply's agent steps → children of the root, each joined to its
    /// gizzi task (same description, and agent when both name one) for
    /// model, cost and nesting (web `treeFromSteps`). With no steps, the
    /// session's whole task tree (web `treeFromTasks`).
    static func tree(rootName: String, steps: [ToolCallRecord], tasks: [SessionTask]) -> RunNode {
        let pool = subagentTasks(tasks)
        let agentSteps = steps.filter(\.isAgentStep)
        var children: [RunNode]
        if agentSteps.isEmpty {
            children = pool.map(node(from:))
        } else {
            var used = Set<String>()
            children = agentSteps.map { step in
                let summary = step.summary
                let description = step.input["description"].flatMap { $0.isEmpty ? nil : $0 } ?? summary.detail
                let agent = summary.subagent == "agent" ? "" : (summary.subagent ?? "")
                if let match = pool.first(where: { t in
                    !used.contains(t.id) && normalize(t.description) == normalize(description)
                        && (agent.isEmpty || (t.agent ?? "").isEmpty || t.agent == agent)
                }) {
                    used.insert(match.id)
                    var n = node(from: match)
                    // The step is the live truth for this reply (the task list polls).
                    if step.state == .running && !n.status.isLive { n.status = .running }
                    if step.state == .failed && n.status == .done {
                        n.status = .failed
                        n.error = n.error ?? step.error
                    }
                    n.id = step.id
                    if !agent.isEmpty {
                        n.name = SubagentNames.title(for: agent)
                        n.slug = SubagentNames.slug(agent)
                    }
                    return n
                }
                let status: RunStatus
                switch step.state {
                case .running: status = .running
                case .failed: status = .failed
                case .done: status = .done
                }
                return RunNode(
                    id: step.id,
                    name: SubagentNames.title(for: agent.isEmpty ? nil : agent),
                    slug: SubagentNames.slug(agent.isEmpty ? nil : agent),
                    status: status,
                    task: description,
                    startedAt: step.startedAt,
                    finishedAt: step.finishedAt,
                    error: step.error
                )
            }
        }
        let live = children.contains { $0.status.isLive }
        let starts = children.compactMap(\.startedAt)
        let ends = children.compactMap(\.finishedAt)
        return RunNode(
            id: "root",
            name: rootName,
            status: live ? .running : .done,
            task: "",
            startedAt: starts.min(),
            finishedAt: live ? nil : ends.max(),
            children: children
        )
    }

    /// What Retry asks the parent agent; it shows in the chat like any message.
    static func retryPrompt(_ node: RunNode) -> String {
        let who = node.name.isEmpty || node.name == "Agent" ? "the agent" : "the \(node.name) agent"
        let why = node.error.map { " The last attempt failed: \($0)" } ?? ""
        return "Run \(who) again for this task: \"\(node.task)\".\(why)"
    }
}
