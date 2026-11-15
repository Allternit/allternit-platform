import Foundation

// -----------------------------------------------------------------------------
// The project page's REST surface: the endpoints ProjectHomeView, its panels and
// ProjectCoordinatorWorkspace call that BotModeClient / ProjectsClient don't
// cover. Paths mirror the web (lib/bots/coordinator.ts, delegate.ts,
// bot-projects.ts, lib/projects/project-status.ts, lib/factory/client.ts,
// lib/agents/memory-client.ts, lib/bots/templates/user-templates.ts,
// views/project/workspace/project-coordinator-api.ts).
// -----------------------------------------------------------------------------

// MARK: - Coordinator messages

struct ProjectReceiptThread: Decodable, Hashable, Sendable {
    let id: String
    let title: String
}

/// `payload` of a coordinator reply (CoordinatorPayload). `threads` is a list
/// on a fanout and a number on a synthesis, so it decodes by hand.
struct ProjectMessagePayload: Decodable, Hashable, Sendable {
    let kind: String?
    let threads: [ProjectReceiptThread]
    let threadCount: Int?
    let threadId: String?
    let threadTitle: String?

    private enum CodingKeys: String, CodingKey { case kind, threads, threadId, threadTitle }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        kind = try? c.decodeIfPresent(String.self, forKey: .kind)
        threads = (try? c.decodeIfPresent([ProjectReceiptThread].self, forKey: .threads)) ?? []
        threadCount = try? c.decodeIfPresent(Int.self, forKey: .threads)
        threadId = try? c.decodeIfPresent(String.self, forKey: .threadId)
        threadTitle = try? c.decodeIfPresent(String.self, forKey: .threadTitle)
    }
}

struct ProjectMessage: Decodable, Identifiable, Hashable, Sendable {
    let id: String
    /// "user" | "coordinator"
    let role: String
    let text: String
    var payload: ProjectMessagePayload? = nil
    let createdAt: String
}

// MARK: - Delegate board (delegate_routes.rs)

struct DelegateTask: Decodable, Identifiable, Hashable, Sendable {
    let id: String
    let title: String
    let notes: String?
    let estimateMin: Int?
}

struct DelegateLaneItem: Decodable, Hashable, Sendable {
    let threadId: String
    let taskId: String?
    let title: String
    let status: String
    let estimateMin: Int?
}

struct DelegateBot: Decodable, Identifiable, Hashable, Sendable {
    let id: String
    let name: String
    let running: Bool
    let costPerTaskCents: Double?
    let costSample: Int
    let busyMinutes: Int
    let unestimated: Int
    let queue: [DelegateLaneItem]
}

struct DelegateBoard: Decodable, Sendable {
    let projectId: String
    let auto: Bool
    let tasks: [DelegateTask]
    let bots: [DelegateBot]
    let queuedCostCents: Double?
    let now: String
    let autoError: String?
}

/// The numbers and words of lib/bots/delegate.ts.
enum DelegateWords {
    static func cents(_ cents: Double) -> String {
        if cents > 0 && cents < 1 { return "<$0.01" }
        return String(format: "$%.2f", cents / 100)
    }

    static func minutes(_ min: Int) -> String {
        if min < 60 { return "~\(min) min" }
        return min % 60 == 0 ? "~\(min / 60) h" : "~\(min / 60) h \(min % 60) min"
    }

    static func isActive(_ status: String) -> Bool {
        status == "working" || status == "planning" || status == "queued"
    }

    private static func clock(_ from: Date, plus minutes: Int) -> String {
        from.addingTimeInterval(Double(minutes) * 60).formatted(date: .omitted, time: .shortened)
    }

    /// "Free", "Busy until 1:31 PM" or "Busy, 2 in line".
    static func load(_ bot: DelegateBot, now: Date) -> String {
        guard bot.queue.contains(where: { isActive($0.status) }) else { return "Free" }
        if bot.unestimated == 0 && bot.busyMinutes > 0 { return "Busy until \(clock(now, plus: bot.busyMinutes))" }
        let waiting = bot.queue.filter { $0.status == "queued" }.count
        return waiting > 0 ? "Busy, \(waiting) in line" : "Busy"
    }

    /// What handing a task to the bot adds (delegate.ts dropPreview).
    static func dropPreview(_ bot: DelegateBot, estimate: Int?, now: Date) -> String {
        var parts: [String] = []
        if let cost = bot.costPerTaskCents { parts.append("+\(cents(cost))") }
        if let estimate {
            let time = "+\(estimate) min"
            parts.append(bot.unestimated == 0 ? "\(time), done ~\(clock(now, plus: bot.busyMinutes + estimate))" : time)
        }
        if parts.isEmpty { return bot.queue.contains { isActive($0.status) } ? "Goes next in line" : "Starts now" }
        return parts.joined(separator: " · ")
    }

    static func costLine(_ bot: DelegateBot) -> String {
        bot.costPerTaskCents.map { "≈ \(cents($0)) per task" } ?? "No cost history yet"
    }
}

// MARK: - Status page (metadata.status)

struct ProjectStatusSection: Decodable, Hashable, Sendable {
    let text: String
    let updatedBy: String
    let updatedAt: String

    private enum CodingKeys: String, CodingKey {
        case text
        case updatedBy = "updated_by"
        case updatedAt = "updated_at"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        text = try c.decodeIfPresent(String.self, forKey: .text) ?? ""
        updatedBy = try c.decodeIfPresent(String.self, forKey: .updatedBy) ?? ""
        updatedAt = try c.decodeIfPresent(String.self, forKey: .updatedAt) ?? ""
    }
}

struct ProjectStatusLogEntry: Decodable, Hashable, Sendable {
    let at: String
    let by: String
    let note: String
    let proofUrl: String?

    private enum CodingKeys: String, CodingKey {
        case at, by, note
        case proofUrl = "proof_url"
    }

    /// The proof as something a tap can open: bots write this field, so only
    /// web links count.
    var proofLink: URL? {
        guard let proofUrl, let url = URL(string: proofUrl.trimmingCharacters(in: .whitespacesAndNewlines)),
              let scheme = url.scheme?.lowercased(), scheme == "http" || scheme == "https" else { return nil }
        return url
    }
}

struct ProjectStatusRecord: Decodable, Sendable {
    let lifecycle: String
    let state: String
    let nextAction: String
    let sections: [String: ProjectStatusSection]
    let log: [ProjectStatusLogEntry]
    let archive: [ProjectStatusLogEntry]

    private enum CodingKeys: String, CodingKey {
        case lifecycle, state, sections, log, archive
        case nextAction = "next_action"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        lifecycle = try c.decodeIfPresent(String.self, forKey: .lifecycle) ?? "active"
        state = try c.decodeIfPresent(String.self, forKey: .state) ?? "on_track"
        nextAction = try c.decodeIfPresent(String.self, forKey: .nextAction) ?? ""
        sections = try c.decodeIfPresent([String: ProjectStatusSection].self, forKey: .sections) ?? [:]
        log = try c.decodeIfPresent([ProjectStatusLogEntry].self, forKey: .log) ?? []
        archive = try c.decodeIfPresent([ProjectStatusLogEntry].self, forKey: .archive) ?? []
    }

    /// STATUS_SECTION_KEYS with their labels, in the web's order.
    static let sectionOrder: [(key: String, label: String)] = [
        ("goal", "Goal"), ("status", "Status"), ("next_actions", "Next actions"), ("owners", "Owners"),
        ("deadlines", "Deadlines"), ("decisions", "Decisions"), ("blockers", "Blockers"),
        ("outcome", "Outcome"), ("lessons", "Lessons"),
    ]

    static let stateLabels: [(key: String, label: String)] = [
        ("on_track", "On track"), ("at_risk", "At risk"), ("blocked", "Blocked"),
        ("needs_decision", "Needs decision"), ("done", "Done"),
    ]

    static func label(forState state: String) -> String {
        stateLabels.first { $0.key == state }?.label ?? state
    }
}

// MARK: - Routines

/// A routine as the project sections need it. `workspace_id` and `metadata`
/// say which project it belongs to (useProjectRoutines).
struct ProjectRoutineRow: Decodable, Identifiable, Hashable, Sendable {
    let id: String
    let name: String
    let status: String
    let scheduleType: String
    let scheduleExpression: String
    let nextRunAt: String?
    private let workspaceId: String?
    private let metadataProjectId: String?

    private enum CodingKeys: String, CodingKey {
        case id, name, status, metadata
        case scheduleType = "schedule_type"
        case scheduleExpression = "schedule_expression"
        case nextRunAt = "next_run_at"
        case workspaceId = "workspace_id"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        name = try c.decodeIfPresent(String.self, forKey: .name) ?? "Routine"
        status = try c.decodeIfPresent(String.self, forKey: .status) ?? "active"
        scheduleType = try c.decodeIfPresent(String.self, forKey: .scheduleType) ?? "cron"
        scheduleExpression = try c.decodeIfPresent(String.self, forKey: .scheduleExpression) ?? ""
        nextRunAt = try? c.decodeIfPresent(String.self, forKey: .nextRunAt)
        workspaceId = try? c.decodeIfPresent(String.self, forKey: .workspaceId)
        var projectId: String? = nil
        switch try? c.decodeIfPresent(JSONValue.self, forKey: .metadata) {
        case .object(let bag)?:
            if case .string(let value)? = bag["projectId"] { projectId = value }
        case .string(let raw)?:
            if let bag = try? JSONDecoder().decode([String: JSONValue].self, from: Data(raw.utf8)),
               case .string(let value)? = bag["projectId"] { projectId = value }
        default:
            break
        }
        metadataProjectId = projectId
    }

    func belongs(to projectId: String) -> Bool {
        workspaceId == projectId || metadataProjectId == projectId
    }

    /// "Every day at 9:00 AM · Active"
    var scheduleLine: String {
        let schedule = CronWords.describe(type: scheduleType, expression: scheduleExpression)
        let word = status == "active" ? "Active" : (status == "paused" ? "Paused" : status.capitalized)
        return "\(schedule) · \(word)"
    }
}

// MARK: - Factory (lib/factory/types.ts, the fields these screens show)

struct FactoryCampaign: Decodable, Identifiable, Hashable, Sendable {
    let id: String
    let projectId: String?
    let title: String
    let proven: Int
    let total: Int
    let needsYou: Int

    /// hooks.ts campaignForProject: a linked campaign first, then one sharing
    /// the project's id; lowest id wins.
    static func forProject(_ campaigns: [FactoryCampaign], projectId: String) -> FactoryCampaign? {
        let sorted = campaigns.sorted { $0.id < $1.id }
        return sorted.first { $0.projectId == projectId } ?? sorted.first { $0.id == projectId }
    }
}

struct FactoryProof: Decodable, Hashable, Sendable {
    let proven: Int
    let total: Int
}

struct FactoryNodeRef: Decodable, Hashable, Sendable {
    let dagId: String
    let nodeId: String
    let title: String
}

struct FactoryGate: Decodable, Hashable, Sendable {
    let kind: String
}

struct FactoryNodeCard: Decodable, Identifiable, Hashable, Sendable {
    let dagId: String
    let nodeId: String
    let title: String
    let status: String
    let assignee: String?
    let bindingType: String?
    let proof: FactoryProof?
    let blockedBy: [String]?
    let needsYou: Bool?
    let depth: Int?
    let gate: FactoryGate?

    var id: String { "\(dagId):\(nodeId)" }

    var statusLabel: String {
        switch status {
        case "new": return "New"
        case "ready": return "Ready"
        case "working": return "Working"
        case "checking": return "Checking"
        case "needs_you": return "Needs you"
        case "done": return "Done"
        case "failed": return "Failed"
        case "blocked": return "Blocked"
        default: return status.capitalized
        }
    }
}

struct FactoryBoard: Decodable, Sendable {
    struct Campaign: Decodable, Sendable {
        let intent: String?
    }

    /// The judge's count, the only source of PROVEN and the progress bar.
    struct Proven: Decodable, Sendable {
        let k: Int
        let n: Int
    }

    struct Summary: Decodable, Sendable {
        let now: [FactoryNodeCard]
        let next: [FactoryNodeCard]
        let proven: Proven?
        let needsYou: [FactoryNodeCard]
    }

    struct Wave: Decodable, Identifiable, Sendable {
        let depth: Int
        let nodes: [FactoryNodeCard]
        var id: Int { depth }
    }

    let campaign: Campaign?
    let summary: Summary
    let waves: [Wave]

    /// layout.ts boardWaves: depth order, each sorted by node id, no empty waves.
    var sortedWaves: [(depth: Int, nodes: [FactoryNodeCard])] {
        let merged = Dictionary(grouping: waves, by: \.depth).mapValues { $0.flatMap(\.nodes) }
        return merged.keys.sorted().compactMap { depth in
            guard let nodes = merged[depth], !nodes.isEmpty else { return nil }
            return (depth: depth, nodes: nodes.sorted { ($0.nodeId, $0.dagId) < ($1.nodeId, $1.dagId) })
        }
    }

    /// Every node on the board (connected.tsx boardNodes).
    var allNodes: [FactoryNodeCard] { waves.flatMap(\.nodes) }

    /// The campaign's DAGs, sorted; the Flow picker shows when there are several.
    var dagIds: [String] { Array(Set(allNodes.map(\.dagId))).sorted() }
}

struct FactoryFlow: Decodable, Sendable {
    struct Edge: Decodable, Hashable, Sendable {
        let from: String
        let to: String
        /// "blocked_by" or "on_fail".
        let type: String?
    }

    let dagId: String
    let title: String
    let nodes: [FactoryNodeCard]
    let edges: [Edge]
}

struct FactoryAgentBinding: Decodable, Hashable, Sendable {
    let type: String?
    let harness: String?
    let vendor: String?
    let lane: String?

    /// parts.tsx bindingLabel: "Hosted · Gizzi", "Terminal · Claude Code", "Vendor · ChatGPT · official".
    var label: String {
        switch type {
        case "hosted":
            return "Hosted · Gizzi"
        case "terminal":
            let harnessName = harness.map { Self.harnessNames[$0] ?? $0 } ?? "terminal"
            return "Terminal · \(harnessName)"
        default:
            let vendorName = vendor.map { Self.vendorNames[$0.lowercased()] ?? $0 } ?? "vendor"
            let laneName = lane.map { Self.laneNames[$0] ?? $0 }
            return ["Vendor", vendorName, laneName].compactMap { $0 }.joined(separator: " · ")
        }
    }

    private static let harnessNames = ["gizzi": "Gizzi", "claude": "Claude Code", "codex": "Codex",
                                       "kimi": "Kimi", "grok": "Grok", "agy": "Antigravity"]
    private static let vendorNames = ["chatgpt": "ChatGPT", "claude": "Claude", "gemini": "Gemini",
                                      "grok": "Grok", "perplexity": "Perplexity"]
    private static let laneNames = ["official": "official", "channel": "channel", "ui_bridge": "UI bridge", "local": "local"]
}

struct FactoryAgent: Decodable, Identifiable, Hashable, Sendable {
    let id: String
    let name: String
    /// "slug@team", or the slug; nodes name their assignee by this.
    let address: String
    let role: String?
    let state: String?
    let binding: FactoryAgentBinding?
    let proof: FactoryProof?
    let currentNode: FactoryNodeRef?
    /// Terminal bots only.
    let pane: Pane?
    let reach: [String]

    struct Pane: Decodable, Hashable, Sendable {
        let id: String
        let attachable: Bool
    }

    private enum CodingKeys: String, CodingKey { case id, name, address, role, state, binding, proof, currentNode, pane, reach }

    /// A Terminal bot whose pane is running can be taken over (TeamLanes NodeDetail).
    var attachablePane: Pane? {
        binding?.type == "terminal" && pane?.attachable == true ? pane : nil
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        name = try c.decodeIfPresent(String.self, forKey: .name) ?? "Bot"
        address = (try? c.decodeIfPresent(String.self, forKey: .address)) ?? id
        role = try? c.decodeIfPresent(String.self, forKey: .role)
        state = try? c.decodeIfPresent(String.self, forKey: .state)
        binding = try? c.decodeIfPresent(FactoryAgentBinding.self, forKey: .binding)
        proof = try? c.decodeIfPresent(FactoryProof.self, forKey: .proof)
        currentNode = try? c.decodeIfPresent(FactoryNodeRef.self, forKey: .currentNode)
        pane = try? c.decodeIfPresent(Pane.self, forKey: .pane)
        reach = (try? c.decodeIfPresent([String].self, forKey: .reach)) ?? []
    }
}

struct FactoryTeam: Decodable, Sendable {
    struct Edge: Decodable, Hashable, Sendable {
        let kind: String
        let from: String
        let to: String
    }

    let name: String
    let edges: [Edge]
}

struct FactoryNodePage: Decodable, Sendable {
    struct ProofLine: Decodable, Hashable, Sendable {
        let line: String
        let checkedBy: String?
    }

    struct Spec: Decodable, Sendable {
        let intent: String?
        let miniRequirements: [String]?
        let proofContract: [ProofLine]?
    }

    struct File: Decodable, Hashable, Sendable {
        let path: String
    }

    struct Delivery: Decodable, Hashable, Sendable {
        let id: String?
    }

    let card: FactoryNodeCard
    let spec: Spec?
    let progressMd: String?
    let files: [File]?
    let deliveries: [Delivery]?
}

// MARK: - Client

final class BotProjectClient: @unchecked Sendable {
    static let shared = BotProjectClient()

    private let client: APIClient
    private let factoryBase: URL

    init(client: APIClient = .shared, factoryBase: URL = AppConfig.factoryBaseURL) {
        self.client = client
        self.factoryBase = factoryBase
    }

    private static func escape(_ id: String) -> String {
        var allowed = CharacterSet.urlPathAllowed
        allowed.remove(charactersIn: "/?#")
        return id.addingPercentEncoding(withAllowedCharacters: allowed) ?? id
    }

    private struct EmptyBody: Encodable {}

    // MARK: Coordinator

    func messages(projectId: String) async throws -> [ProjectMessage] {
        struct Envelope: Decodable { let messages: [ProjectMessage] }
        let envelope: Envelope = try await client.get(path: "projects/\(Self.escape(projectId))/messages")
        return envelope.messages
    }

    // MARK: Threads

    func thread(id: String) async throws -> BotThread {
        struct Envelope: Decodable { let thread: BotThread }
        let envelope: Envelope = try await client.get(path: "threads/\(Self.escape(id))")
        return envelope.thread
    }

    /// A session made for a project's bot becomes a server thread in the
    /// project (bot-threads.ts `createBotThread`: `threadApi.create`).
    func adoptThread(botId: String, title: String, sessionId: String, projectId: String) async throws {
        struct Body: Encodable {
            let botId: String
            let title: String
            let sessionId: String
            let createdBy = "user"
            let projectId: String
        }
        try await client.post(path: "threads", body: Body(botId: botId, title: title, sessionId: sessionId, projectId: projectId))
    }

    /// ThreadResolvedBanner.tsx `reopenThread`.
    func reopenThread(_ thread: BotThread) async throws {
        struct Patch: Encodable { let status = "idle" }
        if thread.archived == true || thread.archivedAt != nil {
            try await client.post(path: "threads/\(Self.escape(thread.id))/unarchive", body: EmptyBody())
        }
        try await client.patch(path: "threads/\(Self.escape(thread.id))", body: Patch())
    }

    // MARK: Project record

    private struct ProjectRecord: Decodable {
        let instructions: String?
        let metadata: String?
    }

    private struct ProjectEnvelope: Decodable { let project: ProjectRecord }

    func instructions(projectId: String) async throws -> String {
        let envelope: ProjectEnvelope = try await client.get(path: "cowork/projects/\(Self.escape(projectId))")
        return envelope.project.instructions ?? ""
    }

    func saveInstructions(projectId: String, instructions: String) async throws {
        struct Body: Encodable {
            let instructions: String?
            func encode(to encoder: Encoder) throws {
                var c = encoder.container(keyedBy: CodingKeys.self)
                try c.encode(instructions, forKey: .instructions)
            }
            enum CodingKeys: String, CodingKey { case instructions }
        }
        let trimmed = instructions.trimmingCharacters(in: .whitespacesAndNewlines)
        try await client.patch(path: "cowork/projects/\(Self.escape(projectId))",
                               body: Body(instructions: trimmed.isEmpty ? nil : instructions))
    }

    func saveDetails(projectId: String, title: String, description: String) async throws {
        struct Body: Encodable {
            let title: String
            let description: String?
            func encode(to encoder: Encoder) throws {
                var c = encoder.container(keyedBy: CodingKeys.self)
                try c.encode(title, forKey: .title)
                try c.encode(description, forKey: .description)
            }
            enum CodingKeys: String, CodingKey { case title, description }
        }
        let trimmed = description.trimmingCharacters(in: .whitespacesAndNewlines)
        try await client.patch(path: "cowork/projects/\(Self.escape(projectId))",
                               body: Body(title: title, description: trimmed.isEmpty ? nil : description))
    }

    /// bot-projects.ts `setBotProjectFlags`: read the metadata, merge the two
    /// keys, write it back whole so migrated files and the guide survive.
    func setFlags(projectId: String, favorite: Bool? = nil, archived: Bool? = nil) async throws {
        struct Body: Encodable { let metadata: String }
        let path = "cowork/projects/\(Self.escape(projectId))"
        let envelope: ProjectEnvelope = try await client.get(path: path)
        var metadata: [String: JSONValue] = [:]
        if let raw = envelope.project.metadata?.data(using: .utf8),
           let parsed = try? JSONDecoder().decode([String: JSONValue].self, from: raw) {
            metadata = parsed
        }
        if let favorite { metadata["favorite"] = .bool(favorite) }
        if let archived { metadata["archived"] = .bool(archived) }
        let encoded = String(decoding: try JSONEncoder().encode(metadata), as: UTF8.self)
        try await client.patch(path: path, body: Body(metadata: encoded))
    }

    // MARK: Delegate

    private func delegatePath(_ projectId: String) -> String { "projects/\(Self.escape(projectId))/delegate" }

    func delegateBoard(projectId: String) async throws -> DelegateBoard {
        try await client.get(path: delegatePath(projectId))
    }

    func addDelegateTask(projectId: String, title: String, estimateMin: Int?) async throws -> DelegateBoard {
        struct Body: Encodable {
            let title: String
            let estimateMin: Int?
        }
        return try await client.post(path: "\(delegatePath(projectId))/tasks", body: Body(title: title, estimateMin: estimateMin))
    }

    func removeDelegateTask(projectId: String, taskId: String) async throws {
        try await client.delete(path: "\(delegatePath(projectId))/tasks/\(Self.escape(taskId))")
    }

    func assignDelegateTask(projectId: String, taskId: String, botId: String) async throws {
        struct Body: Encodable { let botId: String }
        try await client.post(path: "\(delegatePath(projectId))/tasks/\(Self.escape(taskId))/assign", body: Body(botId: botId))
    }

    func setDelegateAuto(projectId: String, auto: Bool) async throws {
        struct Body: Encodable { let auto: Bool }
        try await client.put(path: "\(delegatePath(projectId))/auto", body: Body(auto: auto))
    }

    // MARK: Status

    private func statusPath(_ projectId: String) -> String { "cowork/projects/\(Self.escape(projectId))/status" }

    func status(projectId: String) async throws -> ProjectStatusRecord {
        struct Envelope: Decodable { let status: ProjectStatusRecord }
        let envelope: Envelope = try await client.get(path: statusPath(projectId))
        return envelope.status
    }

    /// People edit the page as "you" (project-status.ts). Answers with the
    /// page as it now stands.
    func patchStatus(projectId: String, section: String? = nil, text: String? = nil, state: String? = nil,
                     done: Bool? = nil, proofUrl: String? = nil) async throws -> ProjectStatusRecord {
        struct Body: Encodable {
            let by = "you"
            let section: String?
            let text: String?
            let state: String?
            let done: Bool?
            let proof_url: String?
        }
        struct Envelope: Decodable { let status: ProjectStatusRecord }
        var request = try await client.authorizedRequest(path: statusPath(projectId), method: "PATCH")
        request.httpBody = try JSONEncoder().encode(Body(section: section, text: text, state: state, done: done, proof_url: proofUrl))
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        let (data, response) = try await client.send(request)
        try client.validate(response, data: data)
        return try JSONDecoder().decode(Envelope.self, from: data).status
    }

    /// `POST cowork/projects/:id/close`: closes the project, or reopens it
    /// with `reopen`. Closing pauses the project's routines; reopening restarts them.
    func closeProject(projectId: String, outcome: String, lessons: String, reopen: Bool) async throws -> ProjectStatusRecord {
        struct Body: Encodable {
            let by = "you"
            let outcome: String
            let lessons: String
            let reopen: Bool
        }
        struct Envelope: Decodable { let status: ProjectStatusRecord }
        let envelope: Envelope = try await client.post(
            path: "cowork/projects/\(Self.escape(projectId))/close",
            body: Body(outcome: outcome, lessons: lessons, reopen: reopen)
        )
        return envelope.status
    }

    // MARK: Routines

    func routines(projectId: String) async throws -> [ProjectRoutineRow] {
        let all: [ProjectRoutineRow] = try await client.get(path: "automation/routines")
        return all.filter { $0.belongs(to: projectId) }
    }

    // MARK: Memory

    /// useProjectMemory: facts whose source observation came from one of the
    /// project's sessions.
    func memoryFacts(sessionIds: Set<String>) async throws -> [String] {
        guard !sessionIds.isEmpty else { return [] }
        struct Observation: Decodable {
            let id: String
            let session_id: String?
        }
        struct Fact: Decodable {
            let fact: String
            let source_observation_id: String?
        }
        struct Observations: Decodable { let observations: [Observation]? }
        struct Facts: Decodable { let facts: [Fact]? }
        let observations: Observations = try await client.get(path: "memory/v2/observations?limit=500")
        let facts: Facts = try await client.get(path: "memory/v2/facts?limit=500")
        let own = Set((observations.observations ?? []).filter { $0.session_id.map(sessionIds.contains) ?? false }.map(\.id))
        return (facts.facts ?? []).filter { $0.source_observation_id.map(own.contains) ?? false }.map(\.fact)
    }

    // MARK: Templates

    func saveTeamTemplate(visibility: String, template: [String: JSONValue]) async throws {
        struct Body: Encodable {
            let kind = "team"
            let visibility: String
            let template: [String: JSONValue]
        }
        try await client.post(path: "templates", body: Body(visibility: visibility, template: template))
    }

    // MARK: Project workspaces (a Chat project's coordinator)

    private struct Registration: Decodable { let projectId: String }

    /// The coordinator project behind a Chat project, once registered; nil
    /// until the first send registers it (`project workspace not found`).
    func workspaceProjectId(key: String) async throws -> String? {
        do {
            let registration: Registration = try await client.get(path: "project-workspaces/\(Self.escape(key))")
            return registration.projectId
        } catch let error where error.localizedDescription.localizedCaseInsensitiveContains("not found") {
            return nil
        }
    }

    func registerWorkspace(key: String, nativeId: String, mode: String, title: String, description: String?,
                           directory: String? = nil) async throws -> String {
        struct Settings: Encodable {
            let context: String
            let directory: String?
        }
        struct Body: Encodable {
            let nativeId: String
            let mode: String
            let title: String
            let description: String?
            let settings: Settings
        }
        var request = try await client.authorizedRequest(path: "project-workspaces/\(Self.escape(key))", method: "PUT")
        request.httpBody = try JSONEncoder().encode(Body(
            nativeId: nativeId, mode: mode, title: title, description: description,
            settings: Settings(context: description ?? "", directory: directory)
        ))
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        let (data, response) = try await client.send(request)
        try client.validate(response, data: data)
        return try JSONDecoder().decode(Registration.self, from: data).projectId
    }

    /// Moves a registered workspace to a working folder, or clears it. The PUT
    /// replaces the settings whole, so what's stored (models, permission modes)
    /// is read first and kept. Worker sessions take the folder from here.
    func setWorkspaceDirectory(key: String, nativeId: String, mode: String, title: String, description: String?,
                               directory: String?) async throws {
        struct Stored: Decodable { let settings: [String: JSONValue]? }
        struct Body: Encodable {
            let nativeId: String
            let mode: String
            let title: String
            let description: String?
            let settings: [String: JSONValue]
        }
        let path = "project-workspaces/\(Self.escape(key))"
        let stored: Stored = try await client.get(path: path)
        var settings = stored.settings ?? [:]
        if let directory {
            settings["directory"] = .string(directory)
        } else {
            settings.removeValue(forKey: "directory")
        }
        var request = try await client.authorizedRequest(path: path, method: "PUT")
        request.httpBody = try JSONEncoder().encode(Body(nativeId: nativeId, mode: mode, title: title, description: description, settings: settings))
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        let (data, response) = try await client.send(request)
        try client.validate(response, data: data)
    }

    // MARK: Factory (lives on the user's allternit-api, like Agent Activity)

    private func factory<T: Decodable>(_ path: String) async throws -> T {
        let request = try await client.authorizedRequest(url: factoryBase.appendingPathComponent(path))
        let (data, response) = try await InstanceConnection.runtimeAPIData(for: request, session: client.session)
        try client.validate(response, data: data)
        return try JSONDecoder().decode(T.self, from: data)
    }

    func campaign(projectId: String) async throws -> FactoryCampaign? {
        struct Envelope: Decodable { let campaigns: [FactoryCampaign] }
        let envelope: Envelope = try await factory("campaigns")
        return FactoryCampaign.forProject(envelope.campaigns, projectId: projectId)
    }

    func board(campaignId: String) async throws -> FactoryBoard {
        try await factory("campaigns/\(campaignId)/board")
    }

    func flow(dagId: String) async throws -> FactoryFlow {
        try await factory("dags/\(dagId)")
    }

    func node(dagId: String, nodeId: String) async throws -> FactoryNodePage {
        try await factory("nodes/\(dagId)/\(nodeId)")
    }

    // MARK: Save a run as a template (POST /api/factory/templates)

    /// A saved run. `finished` is false when some of its steps hadn't finished.
    struct SavedRunTemplate: Decodable, Sendable {
        struct FromRun: Decodable, Sendable { let finished: Bool }

        let id: String
        let steps: Int
        let fromRun: FromRun
    }

    /// The Factory's refusal, in its own words (API.md §2).
    struct FactoryRefusal: Error, Sendable {
        let status: Int
        let code: String?
        let fact: String
        let action: String
    }

    func saveRunAsTemplate(fromRun: String, id: String, force: Bool) async throws -> SavedRunTemplate {
        struct Body: Encodable {
            let fromRun: String
            let id: String
            let force: Bool
        }
        struct Answer: Decodable { let saved: SavedRunTemplate }
        struct Refusal: Decodable {
            struct Detail: Decodable {
                let code: String?
                let fact: String?
                let action: String?
            }
            let error: Detail
        }
        var request = try await client.authorizedRequest(url: factoryBase.appendingPathComponent("templates"))
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONEncoder().encode(Body(fromRun: fromRun, id: id, force: force))
        let (data, response) = try await InstanceConnection.runtimeAPIData(for: request, session: client.session)
        if let http = response as? HTTPURLResponse, !(200...299).contains(http.statusCode),
           let refusal = try? JSONDecoder().decode(Refusal.self, from: data) {
            throw FactoryRefusal(status: http.statusCode, code: refusal.error.code,
                                 fact: refusal.error.fact ?? "Couldn't save the template.",
                                 action: refusal.error.action ?? "Try again.")
        }
        try client.validate(response, data: data)
        return try JSONDecoder().decode(Answer.self, from: data).saved
    }

    // MARK: A Terminal bot's pane (lib/factory/panes.ts)

    /// `GET agents/:id/stream`: SSE of `screen` events, then one `gone`.
    func paneStream(agentId: String) async throws -> URLSession.AsyncBytes {
        var request = try await client.authorizedRequest(url: factoryBase.appendingPathComponent("agents/\(agentId)/stream"))
        request.setValue("text/event-stream", forHTTPHeaderField: "Accept")
        request.timeoutInterval = 3600
        let (bytes, response) = try await InstanceConnection.runtimeAPIBytes(for: request, session: client.session)
        try client.validate(response)
        return bytes
    }

    /// `POST agents/:id/input`: keystrokes, not a send.
    func paneInput(agentId: String, text: String?, keys: [String]?) async throws {
        struct Body: Encodable {
            let text: String?
            let keys: [String]?
        }
        var request = try await client.authorizedRequest(url: factoryBase.appendingPathComponent("agents/\(agentId)/input"))
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONEncoder().encode(Body(text: text, keys: keys))
        let (data, response) = try await InstanceConnection.runtimeAPIData(for: request, session: client.session)
        try client.validate(response, data: data)
    }

    /// `useFactoryTeamFor`: the project's bots that are in the Factory, and
    /// the edges between them across every team.
    func team(botIds: [String]) async throws -> (agents: [FactoryAgent], edges: [FactoryTeam.Edge]) {
        struct Agents: Decodable { let agents: [FactoryAgent] }
        struct Teams: Decodable { let teams: [FactoryTeam] }
        let ids = Set(botIds)
        let agents: Agents = try await factory("agents")
        let members = agents.agents.filter { ids.contains($0.id) }
        let memberIds = Set(members.map(\.id))
        let teams: Teams? = try? await factory("teams")
        var seen = Set<FactoryTeam.Edge>()
        var edges: [FactoryTeam.Edge] = []
        for team in teams?.teams ?? [] {
            for edge in team.edges where memberIds.contains(edge.from) && memberIds.contains(edge.to) && !seen.contains(edge) {
                seen.insert(edge)
                edges.append(edge)
            }
        }
        return (members, edges)
    }
}
